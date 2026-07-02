// PHP AST extractor for tree-sitter.
//
// Supported node types:
//   - php_tag (entry marker, no-op)
//   - function_definition        → FunctionInfo (top-level)
//   - method_declaration         → FunctionInfo (enclosing_class = class)
//   - class_declaration          → ClassInfo (kind=Class)
//   - interface_declaration      → ClassInfo (kind=Interface)
//   - trait_declaration          → ClassInfo (kind=Trait)
//   - enum_declaration           → ClassInfo (kind=Enum)
//   - namespace_definition       → context-only, qualifies classes
//   - use_declaration / use_list → ImportInfo (per alias)
//   - call_expression            → CallGraphEntry
//
// Visibility for members follows PHP's `public`/`protected`/
// `private` modifiers. Top-level functions inside a
// `namespace Foo;` block are treated as `public` for export.

use std::collections::HashSet;

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, InheritanceKind, LanguageExtractor, StructuralAnalysis};

pub struct PhpExtractor;

impl PhpExtractor {
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

    /// PHP visibility is one of `public`, `protected`, `private`.
    /// Read modifiers in declaration order to capture a single
    /// visibility keyword.
    fn visibility_from_modifiers(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if matches!(
                    child.kind(),
                    "visibility_modifier" | "public_modifier" | "protected_modifier"
                    | "private_modifier"
                ) {
                    let t = Self::node_text(&child, source);
                    if t.contains("public") { return Some("public".to_string()); }
                    if t.contains("protected") { return Some("protected".to_string()); }
                    if t.contains("private") { return Some("private".to_string()); }
                }
            }
        }
        None
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
        if name.is_empty() {
            return None;
        }
        let visibility = Self::visibility_from_modifiers(node, source);
        let line_range = Self::line_range(node);
        let qualified = match enclosing_class {
            Some(c) => format!("{c}.{name}"),
            None => name.clone(),
        };
        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: qualified,
            line_range,
            params: Vec::new(),
            return_type: None,
            enclosing_class: enclosing_class.map(|s| s.to_string()),
            visibility,
        });
        Some(name)
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
        if name.is_empty() {
            return;
        }
        let line_range = Self::line_range(node);

        // `class Foo extends Bar` and `class Foo implements IBar,
        // IBaz` are both children of class_declaration; the
        // former is a single `parent` child and the latter a
        // list under `interface`/`class_interface_clause`.
        if let Some(sup) = node.child_by_field_name("parent") {
            let sup_name = Self::node_text(&sup, source);
            if !sup_name.is_empty() {
                inheritances.push(InheritanceInfo {
                    subclass: name.clone(),
                    superclass: sup_name,
                    kind: InheritanceKind::Inherits,
                    line_number: node.start_position().row as u32 + 1,
                });
            }
        }
        if let Some(ifaces) = node.child_by_field_name("interface") {
            for i in 0..ifaces.child_count() {
                if let Some(item) = ifaces.child(i) {
                    if item.kind() == "interface" || item.kind() == "qualified_identifier"
                        || item.kind() == "name"
                    {
                        let n = Self::node_text(&item, source);
                        if !n.is_empty() {
                            inheritances.push(InheritanceInfo {
                                subclass: name.clone(),
                                superclass: n,
                                kind: InheritanceKind::Implements,
                                line_number: node.start_position().row as u32 + 1,
                            });
                        }
                    }
                }
            }
        }

        if let Some(body) = node.child_by_field_name("body") {
            self.walk_body(body, source, Some(&name), functions);
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            methods: Vec::new(),
            properties: Vec::new(),
            interface_kind: kind,
            implemented_interfaces: Vec::new(),
        });
    }

    fn walk_body(
        &self,
        body: tree_sitter::Node,
        source: &[u8],
        enclosing_class: Option<&str>,
        functions: &mut Vec<FunctionInfo>,
    ) {
        for i in 0..body.child_count() {
            if let Some(child) = body.child(i) {
                if matches!(
                    child.kind(),
                    "method_declaration" | "function_definition"
                ) {
                    let _ = self.extract_function(&child, source, functions, enclosing_class);
                }
            }
        }
    }

    fn extract_callee_name(call_node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        let func_node = call_node.child(0)?;
        match func_node.kind() {
            "name" => Some(Self::node_text(&func_node, source)),
            "qualified_name" | "namespace_name" => Some(Self::node_text(&func_node, source)),
            "member_access_expression" | "field_access_expression" => {
                // `obj->method` (object) or `Class::method` (static).
                let member = func_node.child_by_field_name("name")?;
                Some(Self::node_text(&member, source))
            }
            "call_expression" => Self::extract_callee_name(&func_node, source),
            "scoped_property_access_expression" => {
                let member = func_node.child_by_field_name("name")?;
                Some(Self::node_text(&member, source))
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

    fn is_utility(name: &str) -> bool {
        let mut set: HashSet<&str> = HashSet::new();
        for s in [
            "var_dump", "print_r", "var_export", "echo", "print", "isset", "empty",
            "unset", "die", "exit", "count", "sizeof", "strlen", "substr",
            "str_replace", "strtolower", "strtoupper", "trim", "ltrim", "rtrim",
            "array_merge", "array_push", "array_pop", "array_keys", "array_values",
            "array_map", "array_filter", "in_array", "json_encode", "json_decode",
            "sprintf", "printf", "implode", "explode", "compact", "extract",
            "is_array", "is_string", "is_int", "is_null", "is_bool",
            "define", "defined", "constant", "function_exists", "class_exists",
            "header", "session_start", "require", "include", "require_once",
            "include_once",
        ] {
            set.insert(s);
        }
        let bare = name.rsplit(['.', ':', '\\', '/']).next().unwrap_or(name);
        set.contains(bare)
    }
}

impl Default for PhpExtractor { fn default() -> Self { Self::new() } }

impl LanguageExtractor for PhpExtractor {
    fn language_ids(&self) -> &[&'static str] { &["php"] }

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

impl PhpExtractor {
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
            "function_definition" => {
                if enclosing_class.is_none() {
                    if let Some(name) = self.extract_function(node, source, functions, None) {
                        if !name.is_empty() {
                            exports.push(ExportInfo {
                                name,
                                line_number: node.start_position().row as u32 + 1,
                                is_default: false,
                            });
                        }
                    }
                }
            }
            "method_declaration" => {
                let _ = self.extract_function(node, source, functions, enclosing_class);
            }
            "class_declaration" => {
                self.extract_class(node, source, ClassKind::Class, classes, functions, inheritances);
                return;
            }
            "interface_declaration" => {
                self.extract_class(node, source, ClassKind::Interface, classes, functions, inheritances);
                return;
            }
            "trait_declaration" => {
                self.extract_class(node, source, ClassKind::Trait, classes, functions, inheritances);
                return;
            }
            "enum_declaration" => {
                self.extract_class(node, source, ClassKind::Enum, classes, functions, inheritances);
                return;
            }
            "use_declaration" => {
                // Tree-sitter-php parses `use App\Foo as Bar;` as
                // a `use_list`; collect names either way.
                for i in 0..node.child_count() {
                    if let Some(child) = node.child(i) {
                        if matches!(child.kind(), "use_list" | "namespace_use_list"
                                     | "namespace_use_clause" | "use_clause")
                        {
                            // Recurse one level: tree-sitter-php
                            // groups clauses under a list parent.
                            for j in 0..child.child_count() {
                                if let Some(gc) = child.child(j) {
                                    let n = Self::node_text(&gc, source)
                                        .trim()
                                        .trim_end_matches(';')
                                        .to_string();
                                    if !n.is_empty() && gc.kind() != "(" && gc.kind() != ")"
                                       && gc.kind() != "," && gc.kind() != "use"
                                    {
                                        imports.push(ImportInfo {
                                            source: n.clone(),
                                            specifiers: vec![n
                                                .rsplit(['\\', '/'])
                                                .next()
                                                .unwrap_or(&n)
                                                .to_string()],
                                            line_number: node.start_position().row as u32 + 1,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
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
            "function_definition" | "method_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::node_text(&name_node, source);
                    if !name.is_empty() {
                        let qname = match enclosing_class {
                            Some(c) => format!("{c}.{name}"),
                            None => name,
                        };
                        function_stack.push(qname);
                        pushed = true;
                    }
                }
            }
            "call_expression" => {
                if let Some(caller) = function_stack.last() {
                    if let Some(callee) = Self::extract_callee_name(node, source) {
                        if !Self::is_utility(&callee) {
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
        if pushed { function_stack.pop(); }
    }
}

pub static PHP_EXTRACTOR: PhpExtractor = PhpExtractor;
