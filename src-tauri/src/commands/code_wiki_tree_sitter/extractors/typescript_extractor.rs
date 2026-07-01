// TypeScript/JavaScript AST extractor for tree-sitter.
// Handles: typescript, tsx, javascript, jsx
//
// Supported node types:
//   - function_declaration     → FunctionInfo
//   - class_declaration         → ClassInfo
//   - lexical_declaration      → FunctionInfo (arrow functions)
//   - variable_declaration     → FunctionInfo (arrow functions)
//   - import_statement         → ImportInfo
//   - export_statement          → ExportInfo
//   - call_expression          → CallGraphEntry (via extract_call_graph)

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, LanguageExtractor, StructuralAnalysis};

pub struct TypeScriptExtractor;

impl TypeScriptExtractor {
    pub fn new() -> Self {
        Self
    }

    fn child_by_field<'a>(&self, node: &tree_sitter::Node<'a>, field: &str) -> Option<tree_sitter::Node<'a>> {
        node.child_by_field_name(field)
    }

    fn children_by_type<'a>(&self, node: &tree_sitter::Node<'a>, ty: &str) -> Vec<tree_sitter::Node<'a>> {
        let mut result = Vec::new();
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == ty {
                    result.push(child);
                }
            }
        }
        result
    }

    /// Get the actual source text of a node. `Node::to_string()`
    /// returns the S-expression representation (e.g. `(identifier)`),
    /// NOT the source text — we must use `utf8_text(source)`.
    /// Same fix the Python and Rust extractors have.
    fn node_text(node: &tree_sitter::Node, source: &[u8]) -> String {
        Self::node_text_static(node, source)
    }

    /// Static form of `node_text`, callable from module-level
    /// helper functions (e.g. `collect_implements`).
    fn node_text_static(node: &tree_sitter::Node, source: &[u8]) -> String {
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

    fn extract_name(&self, node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        self.child_by_field(node, "name")
            .map(|n| Self::node_text(&n, source))
    }

    fn extract_line_range(node: &tree_sitter::Node) -> [u32; 2] {
        [node.start_position().row as u32 + 1, node.end_position().row as u32 + 1]
    }

    fn extract_params(&self, params_node: Option<tree_sitter::Node>, source: &[u8]) -> Vec<String> {
        let Some(params_node) = params_node else { return Vec::new() };
        let mut params = Vec::new();
        for i in 0..params_node.child_count() {
            if let Some(child) = params_node.child(i) {
                match child.kind() {
                    "required_parameter" | "optional_parameter" => {
                        if let Some(pattern) = child.child_by_field_name("pattern") {
                            params.push(Self::node_text(&pattern, source));
                        } else if let Some(name) = child.child_by_field_name("name") {
                            params.push(Self::node_text(&name, source));
                        }
                    }
                    "identifier" => {
                        params.push(Self::node_text(&child, source));
                    }
                    "rest_pattern" | "rest_element" => {
                        if let Some(pattern) = child.child_by_field_name("pattern") {
                            params.push(format!("...{}", Self::node_text(&pattern, source)));
                        } else {
                            params.push("...".to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        params
    }

    fn extract_return_type(&self, node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        self.child_by_field(node, "return_type")
            .map(|n| Self::node_text(&n, source))
    }

    fn extract_function(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        enclosing_class: Option<&str>,
    ) {
        let name = match self.extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let params = self.extract_params(self.child_by_field(node, "parameters"), source);
        let return_type = self.extract_return_type(node, source);
        let line_range = Self::extract_line_range(node);
        let qualified = match enclosing_class {
            Some(c) => format!("{c}.{name}"),
            None => name.clone(),
        };

        functions.push(FunctionInfo {
            name,
            qualified_name: qualified,
            line_range,
            params,
            return_type,
            enclosing_class: enclosing_class.map(|s| s.to_string()),
            visibility: None,
        });
    }

    fn extract_arrow_function(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        name: &str,
        functions: &mut Vec<FunctionInfo>,
        enclosing_class: Option<&str>,
    ) {
        let params = self.extract_params(self.child_by_field(node, "parameters"), source);
        let return_type = self.extract_return_type(node, source);
        let line_range = Self::extract_line_range(node);
        let qualified = match enclosing_class {
            Some(c) => format!("{c}.{name}"),
            None => name.to_string(),
        };

        functions.push(FunctionInfo {
            name: name.to_string(),
            qualified_name: qualified,
            line_range,
            params,
            return_type,
            enclosing_class: enclosing_class.map(|s| s.to_string()),
            visibility: None,
        });
    }

    /// Extract a `method_definition` (TS class member) as a first-class
    /// function node with the qualified name `<ClassName>.<methodName>`.
    fn extract_method(
        &self,
        class_name: &str,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
    ) -> Option<String> {
        let name_node = self.child_by_field(node, "name")?;
        let name = Self::node_text(&name_node, source);
        let params = self.extract_params(self.child_by_field(node, "parameters"), source);
        let return_type = self.extract_return_type(node, source);
        let line_range = Self::extract_line_range(node);
        let qualified = format!("{class_name}.{name}");

        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: qualified,
            line_range,
            params,
            return_type,
            enclosing_class: Some(class_name.to_string()),
            visibility: None,
        });
        Some(name)
    }

    fn extract_class(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        classes: &mut Vec<ClassInfo>,
        functions: &mut Vec<FunctionInfo>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        let name = match self.extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let line_range = Self::extract_line_range(node);
        let mut methods = Vec::new();
        let mut properties = Vec::new();
        let mut implemented_interfaces: Vec<String> = Vec::new();

        // `class Foo extends Bar` — concrete inheritance. In
        // tree-sitter-typescript this lives at field
        // `class_heritage` (or sometimes a direct `extends_clause`
        // child) — we walk children looking for the right kind.
        if let Some(extends_clause) = self.child_by_field(node, "extends_clause") {
            for i in 0..extends_clause.child_count() {
                if let Some(child) = extends_clause.child(i) {
                    if child.kind() == "type_identifier" || child.kind() == "identifier" {
                        inheritances.push(InheritanceInfo {
                            subclass: name.clone(),
                            superclass: Self::node_text(&child, source),
                            kind: super::InheritanceKind::Inherits,
                            line_number: node.start_position().row as u32 + 1,
                        });
                        break;
                    }
                }
            }
        }

        // `class Foo implements IFoo, IBar` — interface
        // conformance. Tree-sitter-typescript places this inside
        // a `class_heritage` node which is a child of the
        // `class_declaration`. Walk children to find either a
        // direct `implements_clause` or one nested under
        // `class_heritage`.
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                collect_implements(&child, source, &mut implemented_interfaces);
            }
        }

        if let Some(body) = self.child_by_field(node, "body") {
            for i in 0..body.child_count() {
                if let Some(child) = body.child(i) {
                    match child.kind() {
                        "method_definition" => {
                            if let Some(mname) =
                                self.extract_method(&name, &child, source, functions)
                            {
                                methods.push(mname);
                            }
                        }
                        "public_field_definition" | "private_field_definition"
                        | "field_definition" => {
                            if let Some(name_node) = self.child_by_field(&child, "name") {
                                properties.push(Self::node_text(&name_node, source));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            methods,
            properties,
            interface_kind: ClassKind::Class,
            implemented_interfaces: implemented_interfaces.clone(),
        });

        for iface in &implemented_interfaces {
            inheritances.push(InheritanceInfo {
                subclass: name.clone(),
                superclass: iface.clone(),
                kind: super::InheritanceKind::Implements,
                line_number: node.start_position().row as u32 + 1,
            });
        }
    }

    fn extract_interface(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        classes: &mut Vec<ClassInfo>,
        _functions: &mut Vec<FunctionInfo>,
    ) {
        let name = match self.extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let line_range = Self::extract_line_range(node);
        let mut methods = Vec::new();
        let mut properties = Vec::new();

        if let Some(body) = self.child_by_field(node, "body") {
            for i in 0..body.child_count() {
                if let Some(child) = body.child(i) {
                    match child.kind() {
                        "method_signature" | "abstract_method_signature" => {
                            if let Some(name_node) = self.child_by_field(&child, "name") {
                                methods.push(Self::node_text(&name_node, source));
                            }
                        }
                        "property_signature" => {
                            if let Some(name_node) = self.child_by_field(&child, "name") {
                                properties.push(Self::node_text(&name_node, source));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            methods,
            properties,
            interface_kind: ClassKind::Interface,
            implemented_interfaces: Vec::new(),
        });
    }

    fn extract_import(&self, node: &tree_sitter::Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        let module_name = self.child_by_field(node, "module")
            .or_else(|| self.child_by_field(node, "source"))
            .map(|n| Self::node_text(&n, source))
            .unwrap_or_default();

        let mut specifiers = Vec::new();

        // import { a, b } from "module"
        if let Some(specifier_list) = self.child_by_field(node, "specifier_import") {
            for i in 0..specifier_list.child_count() {
                if let Some(child) = specifier_list.child(i) {
                    match child.kind() {
                        "import_specifier" => {
                            if let Some(n) = self.child_by_field(&child, "name") {
                                specifiers.push(Self::node_text(&n, source));
                            }
                        }
                        "identifier" => {
                            specifiers.push(Self::node_text(&child, source));
                        }
                        _ => {}
                    }
                }
            }
        }

        // import a from "module"
        if let Some(default_specifier) = self.child_by_field(node, "default_import") {
            specifiers.push(Self::node_text(&default_specifier, source));
        }

        imports.push(ImportInfo {
            source: module_name,
            specifiers,
            line_number: node.start_position().row as u32 + 1,
        });
    }

    fn extract_export(&self, node: &tree_sitter::Node, source: &[u8], exports: &mut Vec<ExportInfo>) {
        let declaration = self.child_by_field(node, "declaration");

        if let Some(decl) = declaration {
            match decl.kind() {
                "function_declaration" | "class_declaration" => {
                    if let Some(name) = self.extract_name(&decl, source) {
                        exports.push(ExportInfo {
                            name,
                            line_number: node.start_position().row as u32 + 1,
                            is_default: false,
                        });
                    }
                }
                _ => {}
            }
        }

        // Check for default export
        if node.child_by_field_name("default").is_some() {
            if let Some(name) = declaration.and_then(|d| self.extract_name(&d, source)) {
                exports.push(ExportInfo {
                    name,
                    line_number: node.start_position().row as u32 + 1,
                    is_default: true,
                });
            }
        }
    }

    fn extract_callee_name(&self, call_node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        let func_node = call_node.child(0)?;

        match func_node.kind() {
            "identifier" => Some(Self::node_text(&func_node, source)),
            "member_expression" => {
                let property = self.child_by_field(&func_node, "property")?;
                let object = self.child_by_field(&func_node, "object")?;
                Some(format!("{}.{}", Self::node_text(&object, source), Self::node_text(&property, source)))
            }
            "call_expression" => self.extract_callee_name(&func_node, source),
            _ => Some(Self::node_text(&func_node, source)),
        }
    }
}

/// Recursively walk a `class_declaration` child looking for an
/// `implements_clause` (which tree-sitter-typescript may nest
/// inside a `class_heritage` parent). Extracts each
/// `type_identifier` / `generic_type` / `identifier` child name.
fn collect_implements(
    node: &tree_sitter::Node,
    source: &[u8],
    out: &mut Vec<String>,
) {
    // Direct hit.
    if node.kind() == "implements_clause" {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                match child.kind() {
                    "type_identifier" | "generic_type" | "identifier" => {
                        out.push(TypeScriptExtractor::node_text_static(&child, source));
                    }
                    _ => {}
                }
            }
        }
        return;
    }
    // Otherwise descend — class_heritage wraps it.
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_implements(&child, source, out);
        }
    }
}

impl Default for TypeScriptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageExtractor for TypeScriptExtractor {
    fn language_ids(&self) -> &[&'static str] {
        &["typescript", "tsx", "javascript", "jsx"]
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

        StructuralAnalysis { functions, classes, imports, exports, inheritances }
    }

    fn extract_call_graph(&self, root: &tree_sitter::Node, source: &[u8]) -> Vec<CallGraphEntry> {
        let mut entries = Vec::new();
        let mut function_stack: Vec<String> = Vec::new();

        self.walk_for_calls(root, source, None, &mut function_stack, &mut entries);

        entries
    }
}

impl TypeScriptExtractor {
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
            "function_declaration" => {
                self.extract_function(node, source, functions, enclosing_class);
            }
            "class_declaration" => {
                if self.extract_name(node, source).is_some() {
                    self.extract_class(node, source, classes, functions, inheritances);
                    return;
                }
            }
            "interface_declaration" | "interface" | "type_interface_declaration" => {
                if self.extract_name(node, source).is_some() {
                    self.extract_interface(node, source, classes, functions);
                    return;
                }
            }
            "method_definition" => {
                if enclosing_class.is_none() {
                    let _ = self.extract_method("", node, source, functions);
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        if child.kind() == "variable_declarator" {
                            if let Some(name_node) = self.child_by_field(&child, "name") {
                                let name = Self::node_text_static(&name_node, source);
                                if let Some(value) = self.child_by_field(&child, "value") {
                                    if value.kind() == "arrow_function" {
                                        self.extract_arrow_function(
                                            &value,
                                            source,
                                            &name,
                                            functions,
                                            enclosing_class,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "import_statement" => {
                self.extract_import(node, source, imports);
            }
            "export_statement" => {
                self.extract_export(node, source, exports);
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
            "function_declaration" => {
                if let Some(name_node) = self.child_by_field(node, "name") {
                    let name = Self::node_text_static(&name_node, source);
                    let qname = match enclosing_class {
                        Some(c) => format!("{c}.{name}"),
                        None => name,
                    };
                    function_stack.push(qname);
                    pushed = true;
                }
            }
            "method_definition" => {
                if let Some(name_node) = self.child_by_field(node, "name") {
                    let name = Self::node_text_static(&name_node, source);
                    let qname = match enclosing_class {
                        Some(c) => format!("{c}.{name}"),
                        None => name,
                    };
                    function_stack.push(qname);
                    pushed = true;
                }
            }
            "call_expression" => {
                if let Some(caller) = function_stack.last() {
                    if let Some(callee) = self.extract_callee_name(node, source) {
                        entries.push(CallGraphEntry {
                            caller: caller.clone(),
                            callee,
                            line_number: node.start_position().row as u32 + 1,
                        });
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

/// Singleton instance.
pub static TYPESCRIPT_EXTRACTOR: TypeScriptExtractor = TypeScriptExtractor;
