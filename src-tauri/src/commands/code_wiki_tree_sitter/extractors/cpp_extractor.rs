// C++ AST extractor for tree-sitter.
//
// Extends `c_extractor.rs` for most node types. Differences:
//   - `class_specifier` (with optional `struct` keyword) — emits
//     ClassKind::Class nodes with method bodies.
//   - `namespace_definition` — qualifies nested classes via
//     `ns::Class` so cross-namespace lookup works.
//   - Template declarations are unwrapped to reach the inner
//     function/class node (we don't track template parameters).
//   - Access specifiers (`public:` / `private:` / `protected:`)
//     flip a running `current_visibility` while walking the body,
//     so methods inherit the correct visibility from the
//     preceding access label.

use std::collections::HashSet;

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, InheritanceKind, LanguageExtractor, StructuralAnalysis};

pub struct CppExtractor;

impl CppExtractor {
    pub fn new() -> Self { Self }

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

    fn line_range(node: &tree_sitter::Node) -> [u32; 2] {
        [node.start_position().row as u32 + 1, node.end_position().row as u32 + 1]
    }

    fn unwrap_template<'a>(node: &tree_sitter::Node<'a>) -> tree_sitter::Node<'a> {
        if node.kind() == "template_declaration" {
            // The actual declaration is the last-named child.
            for i in (0..node.child_count()).rev() {
                if let Some(child) = node.child(i) {
                    if !matches!(child.kind(), "template" | "<" | ">" | "type_parameter"
                                  | "optional_type_parameter" | "template_parameter_list") {
                        return child;
                    }
                }
            }
        }
        *node
    }

    fn extract_function(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        exports: &mut Vec<ExportInfo>,
        enclosing_class: Option<&str>,
        visibility_override: Option<&str>,
    ) {
        let declarator = match node.child_by_field_name("declarator") {
            Some(d) => d,
            None => return,
        };
        let mut current = declarator;
        let (name, params, return_type) = loop {
            match current.kind() {
                "function_declarator" => {
                    let n = current
                        .child_by_field_name("declarator")
                        .map(|d| Self::node_text(&d, source));
                    let mut p = Vec::new();
                    if let Some(params) = current.child_by_field_name("parameters") {
                        for i in 0..params.child_count() {
                            if let Some(child) = params.child(i) {
                                if child.kind() == "parameter_declaration"
                                    || child.kind() == "optional_parameter_declaration"
                                {
                                    p.push(Self::node_text(&child, source).trim().to_string());
                                }
                            }
                        }
                    }
                    let r = node.child_by_field_name("type").map(|n| Self::node_text(&n, source));
                    break (n, p, r);
                }
                "pointer_declarator" | "array_declarator" | "reference_declarator" => {
                    match current.child_by_field_name("declarator") {
                        Some(next) => current = next,
                        None => return,
                    }
                }
                _ => return,
            }
        };
        let name = match name {
            Some(n) => n,
            None => return,
        };
        if name.is_empty() {
            return;
        }
        let func_text = Self::node_text(node, source);
        let detected = if func_text.trim_start().starts_with("static") {
            Some("private".to_string())
        } else if visibility_override.is_some() {
            visibility_override.map(|s| s.to_string())
        } else {
            Some("public".to_string())
        };
        let line_range = Self::line_range(node);
        let qualified = match enclosing_class {
            Some(c) => format!("{c}::{name}"),
            None => name.clone(),
        };
        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: qualified,
            line_range,
            params,
            return_type,
            enclosing_class: enclosing_class.map(|s| s.to_string()),
            visibility: detected.clone(),
        });
        if !enclosing_class.is_some() && detected.as_deref() == Some("public") {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_class(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        ns_prefix: &str,
        classes: &mut Vec<ClassInfo>,
        functions: &mut Vec<FunctionInfo>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        let name_node = match node.child_by_field_name("name") {
            Some(n) => n,
            None => return,
        };
        let raw_name = Self::node_text(&name_node, source);
        let full_name = if ns_prefix.is_empty() {
            raw_name.clone()
        } else {
            format!("{ns_prefix}::{raw_name}")
        };
        let line_range = Self::line_range(node);
        let mut properties = Vec::new();
        let mut bases: Vec<(String, bool)> = Vec::new();

        // `class A : public B, private C, IInterface` is a
        // single `base_class_clause` with multiple children.
        if let Some(bases_node) = node.child_by_field_name("base_class_clause") {
            for i in 0..bases_node.child_count() {
                if let Some(b) = bases_node.child(i) {
                    if b.kind() == "base_class" {
                        let text = Self::node_text(&b, source).trim().to_string();
                        // Detect implements-vs-inherits by access
                        // specifier: `class A : IInterface` (no
                        // access label, no `public`/`private`)
                        // → Implements; `class A : public B`
                        // → Inherits.
                        let kind = if text.starts_with("public")
                            || text.starts_with("private")
                            || text.starts_with("protected")
                        {
                            InheritanceKind::Inherits
                        } else {
                            InheritanceKind::Implements
                        };
                        // Strip access specifier for the
                        // superclass name.
                        let super_name = text
                            .trim_start_matches("public")
                            .trim_start_matches("private")
                            .trim_start_matches("protected")
                            .trim_start_matches("virtual")
                            .trim()
                            .trim_end_matches('{')
                            .trim()
                            .to_string();
                        if !super_name.is_empty() {
                            bases.push((super_name.clone(), kind == InheritanceKind::Inherits));
                            inheritances.push(InheritanceInfo {
                                subclass: full_name.clone(),
                                superclass: super_name,
                                kind,
                                line_number: node.start_position().row as u32 + 1,
                            });
                        }
                    }
                }
            }
        }

        if let Some(body) = node.child_by_field_name("body") {
            self.walk_class_body(
                &full_name,
                body,
                source,
                functions,
                &mut properties,
            );
            let _ = bases;
        }

        classes.push(ClassInfo {
            name: raw_name.clone(),
            qualified_name: full_name,
            line_range,
            methods: Vec::new(),
            properties,
            interface_kind: ClassKind::Class,
            implemented_interfaces: Vec::new(),
        });
    }

    fn walk_class_body(
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
                // C++ access labels appear as plain
                // `access_specifier` nodes (with a `:` after).
                if child.kind() == "access_specifier" {
                    let txt = Self::node_text(&child, source);
                    if txt.contains("public") {
                        current_visibility = Some("public".to_string());
                    } else if txt.contains("protected") {
                        current_visibility = Some("protected".to_string());
                    } else if txt.contains("private") {
                        current_visibility = Some("private".to_string());
                    }
                    continue;
                }
                let target = Self::unwrap_template(&child);
                match target.kind() {
                    "function_definition" | "method_definition" | "constructor_definition"
                    | "destructor_definition" => {
                        self.extract_function(
                            &target,
                            source,
                            functions,
                            &mut Vec::new(),
                            Some(class_name),
                            current_visibility.as_deref(),
                        );
                    }
                    "field_declaration" => {
                        for j in 0..target.child_count() {
                            if let Some(grand) = target.child(j) {
                                if grand.kind() == "field_identifier" {
                                    properties.push(Self::node_text(&grand, source));
                                }
                            }
                        }
                    }
                    "class_specifier" => {
                        // Nested class — record as a separate
                        // ClassInfo without descending further
                        // here; top-level walk will pick it up.
                    }
                    _ => {}
                }
            }
        }
    }

    fn extract_callee_name(call_node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        let func_node = call_node.child(0)?;
        match func_node.kind() {
            "identifier" => Some(Self::node_text(&func_node, source)),
            "field_expression" => {
                let field = func_node.child_by_field_name("field")?;
                let value = func_node.child_by_field_name("argument")?;
                Some(format!(
                    "{}.{}",
                    Self::node_text(&value, source),
                    Self::node_text(&field, source),
                ))
            }
            "call_expression" => Self::extract_callee_name(&func_node, source),
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

    fn is_utility(name: &str) -> bool {
        let mut set: HashSet<&str> = HashSet::new();
        for s in [
            "malloc", "free", "new", "delete", "printf", "fprintf", "sprintf",
            "cout", "cin", "cerr", "endl", "begin", "end", "size", "length",
            "empty", "push_back", "emplace_back", "c_str", "data", "get",
            "find", "insert", "erase", "clear", "make_unique", "make_shared",
            "static_cast", "dynamic_cast", "const_cast", "reinterpret_cast",
            "assert", "sizeof", "decltype",
        ] {
            set.insert(s);
        }
        set.contains(name)
    }
}

impl Default for CppExtractor { fn default() -> Self { Self::new() } }

impl LanguageExtractor for CppExtractor {
    fn language_ids(&self) -> &[&'static str] { &["cpp"] }

    fn extract_structure(&self, root: &tree_sitter::Node, source: &[u8]) -> StructuralAnalysis {
        let mut functions = Vec::new();
        let mut classes = Vec::new();
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut inheritances = Vec::new();
        self.walk_and_extract(
            root,
            source,
            "",
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
        self.walk_for_calls(root, source, &mut function_stack, &mut entries);
        entries
    }
}

impl CppExtractor {
    #[allow(clippy::too_many_arguments)]
    fn walk_and_extract(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        ns_prefix: &str,
        functions: &mut Vec<FunctionInfo>,
        classes: &mut Vec<ClassInfo>,
        imports: &mut Vec<ImportInfo>,
        exports: &mut Vec<ExportInfo>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        // Unwrap templates and namespaces as we descend.
        let target = Self::unwrap_template(node);
        match target.kind() {
            "function_definition" => {
                let mut exports_local = std::mem::take(exports);
                self.extract_function(
                    &target,
                    source,
                    functions,
                    &mut exports_local,
                    None,
                    None,
                );
                let _ = exports_local;
            }
            "class_specifier" | "struct_specifier" => {
                // struct in C++ still acts like a class.
                self.extract_class(
                    &target,
                    source,
                    ns_prefix,
                    classes,
                    functions,
                    inheritances,
                );
            }
            "namespace_definition" => {
                // Build new prefix and recurse.
                let new_prefix = if let Some(name_node) = target.child_by_field_name("name") {
                    let ns_name = Self::node_text(&name_node, source);
                    if ns_prefix.is_empty() { ns_name } else { format!("{ns_prefix}::{ns_name}") }
                } else {
                    ns_prefix.to_string()
                };
                for i in 0..target.child_count() {
                    if let Some(child) = target.child(i) {
                        self.walk_and_extract(
                            &child,
                            source,
                            &new_prefix,
                            functions,
                            classes,
                            imports,
                            exports,
                            inheritances,
                        );
                    }
                }
                return;
            }
            "preproc_include" => {
                // Reuse C-style import extraction.
                let mut text = String::new();
                for i in 0..target.child_count() {
                    if let Some(child) = target.child(i) {
                        if matches!(
                            child.kind(),
                            "string_literal" | "system_lib_string" | "identifier"
                        ) {
                            text = Self::node_text(&child, source);
                            break;
                        }
                    }
                }
                let trimmed = text
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .trim_matches('"')
                    .to_string();
                if !trimmed.is_empty() {
                    imports.push(ImportInfo {
                        source: trimmed,
                        specifiers: Vec::new(),
                        line_number: target.start_position().row as u32 + 1,
                    });
                }
            }
            _ => {}
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_and_extract(
                    &child,
                    source,
                    ns_prefix,
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
        function_stack: &mut Vec<String>,
        entries: &mut Vec<CallGraphEntry>,
    ) {
        let mut pushed = false;
        let target = Self::unwrap_template(node);
        match target.kind() {
            "function_definition" | "method_definition" => {
                let decl = target.child_by_field_name("declarator");
                if let Some(decl) = decl {
                    let mut cursor = decl;
                    loop {
                        match cursor.kind() {
                            "function_declarator" => {
                                if let Some(name_node) = cursor.child_by_field_name("declarator") {
                                    function_stack.push(Self::node_text(&name_node, source));
                                    pushed = true;
                                }
                                break;
                            }
                            "pointer_declarator" | "array_declarator"
                            | "reference_declarator" | "field_identifier" => {
                                match cursor.child_by_field_name("declarator") {
                                    Some(next) => cursor = next,
                                    None => break,
                                }
                            }
                            _ => break,
                        }
                    }
                }
            }
            "call_expression" => {
                if let Some(caller) = function_stack.last() {
                    if let Some(callee) = Self::extract_callee_name(&target, source) {
                        let bare = callee.rsplit('.').next().unwrap_or(&callee).to_string();
                        if !Self::is_utility(&bare) {
                            entries.push(CallGraphEntry {
                                caller: caller.clone(),
                                callee,
                                line_number: target.start_position().row as u32 + 1,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_for_calls(&child, source, function_stack, entries);
            }
        }
        if pushed { function_stack.pop(); }
    }
}

pub static CPP_EXTRACTOR: CppExtractor = CppExtractor;
