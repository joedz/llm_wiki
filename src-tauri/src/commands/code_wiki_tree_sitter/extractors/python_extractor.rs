// Python AST extractor for tree-sitter.
//
// Supported node types:
//   - function_definition      → FunctionInfo
//   - class_definition         → ClassInfo
//   - decorated_definition     → unwrap to inner def/class
//   - import_statement        → ImportInfo
//   - import_from_statement    → ImportInfo
//   - call                     → CallGraphEntry (via extract_call_graph)
//
// IMPORTANT: tree-sitter's `Node::to_string()` returns the
// S-expression representation (e.g. `(identifier)`), NOT the
// source text. We must use `utf8_text(source)` to get the real
// text. This is the same bug we fixed in the Rust extractor —
// leaving it here means every Python function/class ends up
// named `(identifier)`.

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, InheritanceKind, LanguageExtractor, StructuralAnalysis};

pub struct PythonExtractor;

impl PythonExtractor {
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

    /// Get the actual source text of a node, falling back to
    /// stripping S-expression quoting if `utf8_text` fails.
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

    fn extract_name(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        Self::child_static(node, "name").map(|n| Self::node_text(&n, source))
    }

    fn child_static<'a>(node: &tree_sitter::Node<'a>, field: &str) -> Option<tree_sitter::Node<'a>> {
        node.child_by_field_name(field)
    }

    fn extract_line_range(node: &tree_sitter::Node) -> [u32; 2] {
        [node.start_position().row as u32 + 1, node.end_position().row as u32 + 1]
    }

    fn extract_params(params_node: Option<tree_sitter::Node>, source: &[u8]) -> Vec<String> {
        let Some(params_node) = params_node else { return Vec::new() };
        let mut params = Vec::new();
        for i in 0..params_node.child_count() {
            if let Some(child) = params_node.child(i) {
                match child.kind() {
                    "identifier" => {
                        let name = Self::node_text(&child, source);
                        // Skip self/cls
                        if name != "self" && name != "cls" {
                            params.push(name);
                        }
                    }
                    "typed_parameter" | "default_parameter" | "typed_default_parameter" => {
                        if let Some(name_node) = Self::child_static(&child, "name") {
                            let name = Self::node_text(&name_node, source);
                            if name != "self" && name != "cls" {
                                params.push(name);
                            }
                        }
                    }
                    "list_splat_pattern" | "dictionary_splat_pattern" => {
                        if let Some(pattern) = Self::child_static(&child, "pattern") {
                            params.push(format!("*{}", Self::node_text(&pattern, source)));
                        } else {
                            params.push("*".to_string());
                        }
                    }
                    "list_pattern" => {
                        if let Some(name_node) = Self::child_static(&child, "name") {
                            params.push(format!("*[{}]", Self::node_text(&name_node, source)));
                        }
                    }
                    _ => {}
                }
            }
        }
        params
    }

    fn extract_return_type(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        Self::child_static(node, "return_type").map(|n| Self::node_text(&n, source))
    }

    fn extract_function(
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        enclosing_class: Option<&str>,
        exports: &mut Vec<ExportInfo>,
    ) {
        let name = match Self::extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let params = Self::extract_params(Self::child_static(node, "parameters"), source);
        let return_type = Self::extract_return_type(node, source);
        let line_range = Self::extract_line_range(node);
        let qualified = match enclosing_class {
            Some(c) => format!("{c}.{name}"),
            None => name.clone(),
        };
        let is_dunder = name.starts_with("__") && name.ends_with("__");
        let is_private = !is_dunder && name.starts_with("__") || name.starts_with("_");
        let visibility = if is_dunder {
            Some("dunder".to_string())
        } else if is_private {
            Some("private".to_string())
        } else {
            Some("public".to_string())
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

        // Public top-level functions / classes are exported in
        // Python's convention. Methods don't get their own
        // export — they're reachable through the class.
        if enclosing_class.is_none() && !is_private && !is_dunder {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_class(
        node: &tree_sitter::Node,
        source: &[u8],
        classes: &mut Vec<ClassInfo>,
        functions: &mut Vec<FunctionInfo>,
        exports: &mut Vec<ExportInfo>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        let name = match Self::extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let line_range = Self::extract_line_range(node);
        let mut methods = Vec::new();
        let mut properties = Vec::new();
        let mut supers: Vec<String> = Vec::new();

        if let Some(args) = Self::child_static(node, "superclasses") {
            for i in 0..args.child_count() {
                if let Some(child) = args.child(i) {
                    match child.kind() {
                        "identifier" | "dotted_name" | "attribute" => {
                            supers.push(Self::node_text(&child, source));
                        }
                        _ => {}
                    }
                }
            }
        } else if let Some(args) = Self::child_static(node, "argument_list") {
            for i in 0..args.child_count() {
                if let Some(child) = args.child(i) {
                    match child.kind() {
                        "identifier" | "dotted_name" | "attribute" => {
                            supers.push(Self::node_text(&child, source));
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Some(body) = Self::child_static(node, "body") {
            for i in 0..body.child_count() {
                if let Some(child) = body.child(i) {
                    match child.kind() {
                        "function_definition" => {
                            if let Some(mname) = Self::extract_name(&child, source) {
                                Self::extract_function(
                                    &child,
                                    source,
                                    functions,
                                    Some(&name),
                                    exports,
                                );
                                methods.push(mname);
                            }
                        }
                        "expression_statement" => {
                            if let Some(name_node) = Self::child_static(&child, "name") {
                                properties.push(Self::node_text(&name_node, source));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let is_dunder_name = name.starts_with("__") && name.ends_with("__");
        let is_private_class = !is_dunder_name && name.starts_with("_");

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            methods,
            properties,
            interface_kind: ClassKind::Class,
            implemented_interfaces: Vec::new(),
        });

        if !is_private_class && !is_dunder_name {
            exports.push(ExportInfo {
                name: name.clone(),
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }

        for sup in &supers {
            inheritances.push(InheritanceInfo {
                subclass: name.clone(),
                superclass: sup.clone(),
                kind: InheritanceKind::Inherits,
                line_number: node.start_position().row as u32 + 1,
            });
        }
    }

    fn extract_import(node: &tree_sitter::Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        let mut specifiers = Vec::new();
        let mut import_source = String::new();

        // import a, b as c
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                match child.kind() {
                    "dotted_name" => {
                        import_source = Self::node_text(&child, source);
                    }
                    "identifier" => {
                        specifiers.push(Self::node_text(&child, source));
                    }
                    "aliased_import" => {
                        if let Some(name) = Self::child_static(&child, "name") {
                            specifiers.push(Self::node_text(&name, source));
                        }
                    }
                    _ => {}
                }
            }
        }

        imports.push(ImportInfo {
            source: import_source,
            specifiers,
            line_number: node.start_position().row as u32 + 1,
        });
    }

    fn extract_import_from(node: &tree_sitter::Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
        let module_name = Self::child_static(node, "module_name")
            .map(|n| Self::node_text(&n, source))
            .unwrap_or_default();

        let mut specifiers = Vec::new();

        // The specifiers in tree-sitter-python's import_from_statement
        // are typically anonymous children (not under a named
        // "specifiers" field). Walk all children of the import
        // node and pick out the identifier-like nodes that aren't
        // the module_name.
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                match child.kind() {
                    // Skip these — already handled above or not relevant
                    "from" | "import" | "module_name" | "(" | ")" | "," => continue,
                    "dotted_name" | "identifier" => {
                        let text = Self::node_text(&child, source);
                        if text == module_name {
                            continue;
                        }
                        specifiers.push(text);
                    }
                    "aliased_import" => {
                        // `x as y` — extract the alias (y)
                        if let Some(alias) = Self::child_static(&child, "alias") {
                            specifiers.push(Self::node_text(&alias, source));
                        } else if let Some(name) = Self::child_static(&child, "name") {
                            specifiers.push(Self::node_text(&name, source));
                        }
                    }
                    "wildcard_import" => {
                        specifiers.push("*".to_string());
                    }
                    _ => {}
                }
            }
        }

        imports.push(ImportInfo {
            source: module_name,
            specifiers,
            line_number: node.start_position().row as u32 + 1,
        });
    }

    fn extract_callee_name(call_node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        let func_node = call_node.child(0)?;

        match func_node.kind() {
            "identifier" => {
                let text = Self::node_text(&func_node, source);
                if Self::is_utility_method(&text) {
                    return None;
                }
                Some(text)
            }
            "attribute" => {
                let property = Self::child_static(&func_node, "attribute")?;
                let object = Self::child_static(&func_node, "object")?;
                let property_text = Self::node_text(&property, source);
                let object_text = Self::node_text(&object, source);
                if Self::is_utility_method(&property_text) {
                    return None;
                }
                Some(format!("{}.{}", object_text, property_text))
            }
            "call" => Self::extract_callee_name(&func_node, source),
            _ => {
                let text = Self::node_text(&func_node, source);
                // Skip if it contains whitespace, parens, braces, or looks like an expression
                if text.contains(' ') || text.contains('\n') || text.contains('(')
                    || text.contains(')') || text.contains('{') || text.contains('}')
                    || text.contains('[') || text.contains(']')
                    || text.starts_with('"') || text.starts_with('\'')
                {
                    return None;
                }
                Some(text)
            }
        }
    }

    /// Returns true if this is a common utility method that creates noise in the call graph.
    fn is_utility_method(name: &str) -> bool {
        matches!(name,
            "print" | "len" | "range" | "str" | "int" | "float" | "bool" | "list" | "dict"
            | "set" | "tuple" | "frozenset" | "bytes" | "bytearray" | "complex"
            | "open" | "input" | "isinstance" | "issubclass" | "type" | "getattr"
            | "setattr" | "delattr" | "hasattr" | "callable" | "repr" | "format"
            | "hash" | "id" | "iter" | "next" | "enumerate" | "zip" | "map" | "filter"
            | "reversed" | "sorted" | "min" | "max" | "sum" | "abs" | "round" | "pow"
            | "all" | "any" | "globals" | "locals" | "vars" | "dir" | "help"
            | "append" | "extend" | "insert" | "remove" | "pop" | "clear" | "update"
            | "get" | "keys" | "values" | "items" | "copy" | "deepcopy"
            | "join" | "split" | "strip" | "lstrip" | "rstrip" | "lower" | "upper"
            | "startswith" | "endswith" | "replace" | "find" | "index" | "count"
            | "read" | "write" | "close" | "seek" | "tell" | "flush"
        )
    }
}

impl Default for PythonExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageExtractor for PythonExtractor {
    fn language_ids(&self) -> &[&'static str] {
        &["python"]
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

impl PythonExtractor {
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
                // Skip if we're descending into a class — extract_class handles those.
                if enclosing_class.is_none() {
                    Self::extract_function(node, source, functions, None, exports);
                }
            }
            "class_definition" => {
                if Self::extract_name(node, source).is_some() {
                    Self::extract_class(node, source, classes, functions, exports, inheritances);
                    return; // body already walked by extract_class
                }
            }
            "decorated_definition" => {
                // Unwrap decorated definitions at top level only.
                if enclosing_class.is_none() {
                    for i in 0..node.child_count() {
                        if let Some(child) = node.child(i) {
                            match child.kind() {
                                "function_definition" => {
                                    Self::extract_function(&child, source, functions, None, exports);
                                }
                                "class_definition" => {
                                    Self::extract_class(
                                        &child,
                                        source,
                                        classes,
                                        functions,
                                        exports,
                                        inheritances,
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            "import_statement" => {
                Self::extract_import(node, source, imports);
            }
            "import_from_statement" => {
                Self::extract_import_from(node, source, imports);
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
            "function_definition" => {
                if let Some(name_node) = Self::child_static(node, "name") {
                    let name = Self::node_text(&name_node, source);
                    if name != "<lambda>" && !name.is_empty() && !name.starts_with('(') {
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
pub static PYTHON_EXTRACTOR: PythonExtractor = PythonExtractor;