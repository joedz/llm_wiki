// Ruby AST extractor for tree-sitter.
//
// Supported node types:
//   - method       → FunctionInfo (with optional `def name` body)
//   - singleton_method → FunctionInfo (qualified `Class.method`)
//   - class / module → ClassInfo
//   - call         → CallGraphEntry (via extract_call_graph)
//
// Ruby conventions:
//   - Methods whose name starts with a lowercase letter OR is
//     `_` / `initialize` are conventionally private. We mark
//     everything else as public.
//   - `class Foo < Bar` → Inherits; `class Foo include M` →
//     Implements (mixin).
//   - Methods inside a class/module become `ClassName#method` for
//     instance methods (qualified with `#` is the Ruby idiom; we
//     use `.` because the A milestone pattern uses `.` for the
//     `qualified_name` anchor — the actual rendering can re-
//     substitute `#` later if desired).

use std::collections::HashSet;

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, InheritanceKind, LanguageExtractor, StructuralAnalysis};

pub struct RubyExtractor;

impl RubyExtractor {
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

    fn params_from_method(node: &tree_sitter::Node, source: &[u8]) -> Vec<String> {
        let Some(params_node) = node.child_by_field_name("parameters") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in 0..params_node.child_count() {
            if let Some(child) = params_node.child(i) {
                let text = Self::node_text(&child, source).trim().to_string();
                if !text.is_empty()
                    && !matches!(text.as_str(), "(" | ")")
                {
                    out.push(text);
                }
            }
        }
        out
    }

    fn visibility_for(name: &str) -> String {
        if name.starts_with(|c: char| c.is_lowercase())
            && name != "initialize"
            && name != "new"
        {
            "private".to_string()
        } else {
            "public".to_string()
        }
    }

    fn extract_method(
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
        let params = Self::params_from_method(node, source);
        let line_range = Self::line_range(node);
        let visibility = Some(Self::visibility_for(&name));
        let qualified = match enclosing_class {
            Some(c) => format!("{c}.{name}"),
            None => name.clone(),
        };
        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: qualified,
            line_range,
            params,
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
        let line_range = Self::line_range(node);

        // `class Foo < Bar` shows up as a `superclass` field
        // holding the constant name.
        if let Some(sup) = node.child_by_field_name("superclass") {
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
                match child.kind() {
                    "method" => {
                        let _ = self.extract_method(&child, source, functions, enclosing_class);
                    }
                    "singleton_method" => {
                        // self.foo = body — define a class-method.
                        if let Some(name_node) = child.child_by_field_name("name") {
                            let name = Self::node_text(&name_node, source);
                            if !name.is_empty() {
                                let params = Self::params_from_method(&child, source);
                                let line_range = Self::line_range(&child);
                                let qualified = match enclosing_class {
                                    Some(c) => format!("{c}.{name}"),
                                    None => name.clone(),
                                };
                                functions.push(FunctionInfo {
                                    name: name.clone(),
                                    qualified_name: qualified,
                                    line_range,
                                    params,
                                    return_type: None,
                                    enclosing_class: enclosing_class.map(|s| s.to_string()),
                                    visibility: Some("public".to_string()),
                                });
                            }
                        }
                    }
                    "class" | "module" => {
                        // Nested class — top-level walk covers it.
                    }
                    _ => {}
                }
            }
        }
    }

    fn extract_callee_name(call_node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        let method = call_node.child_by_field_name("method")?;
        let _ = call_node.child_by_field_name("arguments");
        let mut text = Self::node_text(&method, source);
        // For `obj.method` the parent call has `method` pointing
        // at the last identifier; prefix with the receiver if
        // available.
        if let Some(receiver) = call_node.child_by_field_name("receiver") {
            let recv_text = Self::node_text(&receiver, source);
            if !recv_text.is_empty() && recv_text != "self" {
                text = format!("{recv_text}.{text}");
            }
        }
        Some(text)
    }

    fn is_utility(name: &str) -> bool {
        let mut set: HashSet<&str> = HashSet::new();
        for s in [
            "puts", "print", "p", "pp", "tap", "then", "yield", "raise",
            "attr_reader", "attr_writer", "attr_accessor", "to_s", "to_str",
            "to_a", "to_h", "to_i", "to_f", "nil?", "blank?", "present?",
            "freeze", "dup", "clone", "kind_of?", "is_a?", "instance_of?",
            "respond_to?", "send", "public_send",
        ] {
            set.insert(s);
        }
        let bare = name.rsplit('.').next().unwrap_or(name);
        set.contains(bare)
    }
}

impl Default for RubyExtractor { fn default() -> Self { Self::new() } }

impl LanguageExtractor for RubyExtractor {
    fn language_ids(&self) -> &[&'static str] { &["ruby"] }

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

impl RubyExtractor {
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
            "method" => {
                let _ = self.extract_method(node, source, functions, enclosing_class);
            }
            "class" => {
                self.extract_class(node, source, ClassKind::Class, classes, functions, inheritances);
                return;
            }
            "module" => {
                self.extract_class(node, source, ClassKind::Class, classes, functions, inheritances);
                return;
            }
            "singleton_method" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::node_text(&name_node, source);
                    if !name.is_empty() {
                        functions.push(FunctionInfo {
                            name: name.clone(),
                            qualified_name: enclosing_class
                                .map(|c| format!("{c}.{name}"))
                                .unwrap_or_else(|| name.clone()),
                            line_range: Self::line_range(node),
                            params: Self::params_from_method(node, source),
                            return_type: None,
                            enclosing_class: enclosing_class.map(|s| s.to_string()),
                            visibility: Some("public".to_string()),
                        });
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
            "method" | "singleton_method" => {
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
            "call" => {
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

pub static RUBY_EXTRACTOR: RubyExtractor = RubyExtractor;
