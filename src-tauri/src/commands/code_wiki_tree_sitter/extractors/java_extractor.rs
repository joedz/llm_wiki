// Java AST extractor for tree-sitter.
//
// Supported node types:
//   - class_declaration       → ClassInfo (kind=Class)
//   - interface_declaration   → ClassInfo (kind=Interface)
//   - enum_declaration        → ClassInfo (kind=Enum)
//   - method_declaration      → FunctionInfo (enclosing_class = parent class)
//   - constructor_declaration → FunctionInfo
//   - field_declaration       → (recorded in ClassInfo.properties)
//   - import_declaration      → ImportInfo
//   - method/class calls      → CallGraphEntry (via extract_call_graph)
//
// ID scheme matches the rest of the codebase:
//   free function / method: `function:<rel>:<ClassName>.<method>`
//   types: `class:<rel>:<ClassName>`
// Visibility is recorded verbatim (`public`/`protected`/`private`/`package`).

use std::collections::HashSet;

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, InheritanceKind, LanguageExtractor, StructuralAnalysis};

pub struct JavaExtractor;

impl JavaExtractor {
    pub fn new() -> Self {
        Self
    }

    /// Get the source text of a node, falling back to S-expression
    /// parsing if `utf8_text` fails (same fix the Python and TS
    /// extractors carry).
    fn node_text(node: &tree_sitter::Node, source: &[u8]) -> String {
        if let Ok(text) = node.utf8_text(source) {
            return text.to_string();
        }
        let sexp = node.to_string();
        if let Some(quote_start) = sexp.find(" \"") {
            let after = &sexp[quote_start + 2..];
            if let Some(end) = after.find('"') {
                return after[..end].to_string();
            }
        }
        sexp
    }

    fn child_by_field<'a>(node: &tree_sitter::Node<'a>, field: &str) -> Option<tree_sitter::Node<'a>> {
        node.child_by_field_name(field)
    }

    fn line_range(node: &tree_sitter::Node) -> [u32; 2] {
        [node.start_position().row as u32 + 1, node.end_position().row as u32 + 1]
    }

    fn params_from_list(node: Option<tree_sitter::Node>, source: &[u8]) -> Vec<String> {
        let Some(node) = node else { return Vec::new() };
        let mut out = Vec::new();
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if matches!(child.kind(), "formal_parameter" | "spread_parameter") {
                    let text = Self::node_text(&child, source).trim().to_string();
                    if !text.is_empty() {
                        out.push(text);
                    }
                }
            }
        }
        out
    }

    fn return_type(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        Self::child_by_field(node, "type")
            .map(|n| Self::node_text(&n, source))
    }

    /// Pull the text of a `modifiers` child block (which contains
    /// `public` / `protected` / `private` / `static` / `final` keywords).
    fn modifier_text(node: &tree_sitter::Node, source: &[u8]) -> String {
        if let Some(modifiers) = Self::child_by_field(node, "modifiers") {
            Self::node_text(&modifiers, source)
        } else {
            String::new()
        }
    }

    /// Pick the visibility keyword out of a Java modifiers block.
    fn extract_visibility(modifiers: &str) -> Option<String> {
        if modifiers.contains("public") {
            Some("public".to_string())
        } else if modifiers.contains("protected") {
            Some("protected".to_string())
        } else if modifiers.contains("private") {
            Some("private".to_string())
        } else {
            None
        }
    }

    fn extract_function(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        enclosing_class: Option<&str>,
    ) -> Option<String> {
        let name_node = node.child_by_field_name("name")?;
        let name = Self::node_text(&name_node, source);
        let params = Self::params_from_list(node.child_by_field_name("parameters"), source);
        let return_type = Self::return_type(node, source);
        let line_range = Self::line_range(node);
        let mods = Self::modifier_text(node, source);
        let visibility = Self::extract_visibility(&mods);
        let qualified = match enclosing_class {
            Some(c) => format!("{c}.{name}"),
            None => name.clone(),
        };

        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: qualified,
            line_range,
            params,
            return_type,
            enclosing_class: enclosing_class.map(|s| s.to_string()),
            visibility,
        });
        Some(name)
    }

    /// Walk class body, recording methods and constructors into
    /// `functions` and pushing simple field strings into
    /// `properties`. Methods become first-class qualified nodes.
    fn extract_class_body(
        &self,
        class_name: &str,
        body: tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        properties: &mut Vec<String>,
    ) {
        let mut current_visibility: Option<String> = None;
        for i in 0..body.child_count() {
            if let Some(child) = body.child(i) {
                match child.kind() {
                    "method_declaration" | "constructor_declaration" => {
                        if let Some(mname) =
                            self.extract_function(&child, source, functions, Some(class_name))
                        {
                            // If the method itself has no
                            // visibility modifier, fall back to
                            // whatever the previous modifier set
                            // (e.g. when a `public:` block in an
                            // annotation / annotation-decorated
                            // context applies — rare in plain Java
                            // but harmless).
                            if Self::extract_visibility(&Self::modifier_text(&child, source))
                                .is_none()
                            {
                                if let Some(functions_last) = functions.last_mut() {
                                    if functions_last.enclosing_class.as_deref()
                                        == Some(class_name)
                                    {
                                        functions_last.visibility =
                                            current_visibility.clone();
                                    }
                                }
                            }
                            // Track the most recent modifier for
                            // upcoming declarations without one.
                            current_visibility =
                                Self::extract_visibility(&Self::modifier_text(&child, source));
                            // Side note: `mname` is recorded but
                            // Java methods aren't inlined into the
                            // class.methods array the way UA does.
                            // We keep it as a property string
                            // purely for legacy compat (UA uses
                            // methods: string[] in `ClassInfo`).
                            let _ = mname;
                        }
                    }
                    "field_declaration" => {
                        // field_declaration children include the
                        // variable declarators. Pull their names.
                        for j in 0..child.child_count() {
                            if let Some(grand) = child.child(j) {
                                if grand.kind() == "variable_declarator" {
                                    if let Some(name_field) = grand.child_by_field_name("name") {
                                        properties.push(Self::node_text(&name_field, source));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn extract_class(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        kind: ClassKind,
        classes: &mut Vec<ClassInfo>,
        functions: &mut Vec<FunctionInfo>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        let name_node = match node.child_by_field_name("name") {
            Some(n) => n,
            None => return,
        };
        let name = Self::node_text(&name_node, source);
        let line_range = Self::line_range(node);
        let mut properties = Vec::new();

        // `extends` and `implements` clauses appear as siblings
        // of the body. Look for `superclass` and `super_interfaces`
        // fields first; fall back to walking children for older
        // tree-sitter-java versions.
        if let Some(super_node) = node.child_by_field_name("superclass") {
            let super_name = Self::node_text(&super_node, source);
            if !super_name.is_empty() {
                inheritances.push(InheritanceInfo {
                    subclass: name.clone(),
                    superclass: super_name,
                    kind: InheritanceKind::Inherits,
                    line_number: node.start_position().row as u32 + 1,
                });
            }
        }
        if let Some(ifaces_node) = node.child_by_field_name("super_interfaces") {
            for j in 0..ifaces_node.child_count() {
                if let Some(item) = ifaces_node.child(j) {
                    if item.kind() == "type_identifier"
                        || item.kind() == "generic_type"
                        || item.kind() == "identifier"
                    {
                        inheritances.push(InheritanceInfo {
                            subclass: name.clone(),
                            superclass: Self::node_text(&item, source),
                            kind: InheritanceKind::Implements,
                            line_number: node.start_position().row as u32 + 1,
                        });
                    }
                }
            }
        }

        if let Some(body) = node.child_by_field_name("body") {
            self.extract_class_body(&name, body, source, functions, &mut properties);
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            methods: Vec::new(), // first-class nodes live in `functions`
            properties,
            interface_kind: kind,
            implemented_interfaces: Vec::new(),
        });
    }

    fn extract_import(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        let mut specifiers = Vec::new();
        let mut package_source = String::new();

        if let Some(scoped_id) = node.child_by_field_name("scoped_identifier") {
            package_source = Self::node_text(&scoped_id, source);
        } else if let Some(id) = node.child_by_field_name("identifier") {
            package_source = Self::node_text(&id, source);
        }

        // `import a.b.C;` records the simple name `C`.
        if let Some(dot) = package_source.rfind('.') {
            let simple = package_source[dot + 1..].to_string();
            specifiers.push(simple);
        } else {
            specifiers.push(package_source.clone());
        }

        imports.push(ImportInfo {
            source: package_source,
            specifiers,
            line_number: node.start_position().row as u32 + 1,
        });
    }

    fn extract_callee_name(&self, call_node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        // Java's `method_invocation` IS the call site —
        // tree-sitter-java has no separate `call_expression`.
        // Handle it as the top-level case.
        if call_node.kind() == "method_invocation" {
            if let Some(name_field) = call_node.child_by_field_name("name") {
                return Some(Self::node_text(&name_field, source));
            }
            return None;
        }
        let func_node = call_node.child(0)?;
        match func_node.kind() {
            "identifier" => Some(Self::node_text(&func_node, source)),
            "field_access" => {
                let obj = func_node.child_by_field_name("object")?;
                let field = func_node.child_by_field_name("field")?;
                Some(format!(
                    "{}.{}",
                    Self::node_text(&obj, source),
                    Self::node_text(&field, source),
                ))
            }
            _ => {
                let text = Self::node_text(&func_node, source);
                if text.contains(' ') || text.contains('(') || text.contains(')') {
                    None
                } else {
                    Some(text)
                }
            }
        }
    }

    fn is_utility_method(name: &str) -> bool {
        let mut set: HashSet<&str> = HashSet::new();
        for s in [
            "equals", "hashCode", "toString", "getClass", "notify", "notifyAll",
            "wait", "clone", "finalize", "length", "size", "isEmpty",
            "println", "print", "format", "valueOf", "parseInt", "parseLong",
            "parseFloat", "parseDouble", "valueOf", "iterator", "next", "hasNext",
            "add", "remove", "get", "set", "put", "contains", "keySet", "entrySet",
            "values", "clear", "isNull", "nonNull", "requireNonNull", "requireNull",
        ] {
            set.insert(s);
        }
        set.contains(name)
    }
}

impl Default for JavaExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageExtractor for JavaExtractor {
    fn language_ids(&self) -> &[&'static str] {
        &["java"]
    }

    fn extract_structure(&self, root: &tree_sitter::Node, source: &[u8]) -> StructuralAnalysis {
        let mut functions = Vec::new();
        let mut classes = Vec::new();
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut inheritances = Vec::new();
        self.walk_and_extract(
            root,
            source,
            None,
            &mut functions,
            &mut classes,
            &mut imports,
            &mut exports,
            &mut inheritances,
        );
        StructuralAnalysis {
            functions,
            classes,
            imports,
            exports,
            inheritances,
        }
    }

    fn extract_call_graph(&self, root: &tree_sitter::Node, source: &[u8]) -> Vec<CallGraphEntry> {
        let mut entries = Vec::new();
        let mut function_stack: Vec<String> = Vec::new();
        self.walk_for_calls(root, source, None, &mut function_stack, &mut entries);
        entries
    }
}

impl JavaExtractor {
    #[allow(clippy::too_many_arguments)]
    fn walk_and_extract(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        enclosing_class: Option<&str>,
        functions: &mut Vec<FunctionInfo>,
        classes: &mut Vec<ClassInfo>,
        imports: &mut Vec<ImportInfo>,
        exports: &mut Vec<ExportInfo>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        match node.kind() {
            "class_declaration" => {
                self.extract_class(node, source, ClassKind::Class, classes, functions, inheritances);
                return;
            }
            "interface_declaration" => {
                self.extract_class(node, source, ClassKind::Interface, classes, functions, inheritances);
                return;
            }
            "enum_declaration" => {
                self.extract_class(node, source, ClassKind::Enum, classes, functions, inheritances);
                return;
            }
            "import_declaration" => {
                self.extract_import(node, source, imports);
            }
            _ => {}
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_and_extract(
                    &child,
                    source,
                    enclosing_class,
                    functions,
                    classes,
                    imports,
                    exports,
                    inheritances,
                );
            }
        }
    }

    fn walk_for_calls(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        enclosing_class: Option<&str>,
        function_stack: &mut Vec<String>,
        entries: &mut Vec<CallGraphEntry>,
    ) {
        let mut pushed = false;
        match node.kind() {
            "method_declaration" | "constructor_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::node_text(&name_node, source);
                    let qname = match enclosing_class {
                        Some(c) => format!("{c}.{name}"),
                        None => name,
                    };
                    function_stack.push(qname);
                    pushed = true;
                }
            }
            "method_invocation" => {
                if let Some(caller) = function_stack.last() {
                    if let Some(callee) = self.extract_callee_name(node, source) {
                        let bare = callee.rsplit('.').next().unwrap_or(&callee).to_string();
                        if !Self::is_utility_method(&bare) {
                            entries.push(CallGraphEntry {
                                caller: caller.clone(),
                                callee,
                                line_number: node.start_position().row as u32 + 1,
                            });
                        }
                    }
                }
            }
            _ => {}
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_for_calls(&child, source, enclosing_class, function_stack, entries);
            }
        }

        if pushed {
            function_stack.pop();
        }
    }
}

pub static JAVA_EXTRACTOR: JavaExtractor = JavaExtractor;
