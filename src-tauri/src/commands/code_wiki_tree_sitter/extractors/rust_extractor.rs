// Rust AST extractor for tree-sitter.
// Mirrors Understand-Anything's rust-extractor.ts logic in native Rust.
//
// Supported node types:
//   - function_item        → FunctionInfo
//   - struct_item          → ClassInfo (properties from field_declaration_list)
//   - enum_item            → ClassInfo (properties from enum_variant_list)
//   - trait_item           → ClassInfo (methods from declaration_list / function_item)
//   - impl_item            → FunctionInfo for methods + tracking for class attachment
//   - use_declaration      → ImportInfo
//   - call_expression      → CallGraphEntry (via extract_call_graph)

use std::collections::HashMap;

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, InheritanceKind, LanguageExtractor, StructuralAnalysis};

pub struct RustExtractor;

impl RustExtractor {
    pub fn new() -> Self {
        Self
    }

    // --- helpers ---

    /// Get the text content of a node from the source bytes.
    fn node_text(&self, node: &tree_sitter::Node, source: &[u8]) -> String {
        // Try utf8_text first - this gives the actual source text
        if let Ok(text) = node.utf8_text(source) {
            return text.to_string();
        }
        // Fallback: use sexp representation
        let sexp = node.to_string();
        // Sexp format: "(identifier)" for anonymous nodes or "(identifier) \"text\"" for named
        if let Some(quote_start) = sexp.find(" \"") {
            let after = &sexp[quote_start + 2..];
            if let Some(end) = after.find('"') {
                return after[..end].to_string();
            }
        }
        sexp
    }

    fn child_for_field<'a>(&self, node: &tree_sitter::Node<'a>, field: &str) -> Option<tree_sitter::Node<'a>> {
        node.child_by_field_name(field)
    }

    fn children_by_type<'a>(&self, node: &tree_sitter::Node<'a>, ty: &str) -> Vec<tree_sitter::Node<'a>> {
        let mut result: Vec<tree_sitter::Node> = Vec::new();
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == ty {
                    result.push(child);
                }
            }
        }
        result
    }

    fn extract_params(&self, params_node: Option<tree_sitter::Node>, source: &[u8]) -> Vec<String> {
        let Some(params_node) = params_node else { return Vec::new() };
        let mut params = Vec::new();
        for i in 0..params_node.child_count() {
            if let Some(child) = params_node.child(i) {
                if child.kind() == "parameter" {
                    if let Some(pattern) = child.child_by_field_name("pattern") {
                        params.push(self.node_text(&pattern, source));
                    }
                }
            }
        }
        params
    }

    fn extract_return_type(&self, node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        self.child_for_field(node, "return_type")
            .map(|n| self.node_text(&n, source))
    }

    fn is_public(&self, node: &tree_sitter::Node, source: &[u8]) -> bool {
        // tree-sitter-rust exposes visibility as a positional
        // child node of kind `visibility_modifier`, not as a
        // named field. Match the kind AND read its source text
        // — `pub`, `pub(crate)`, `pub(super)`, etc.
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == "visibility_modifier" {
                    let text = self.node_text(&child, source);
                    if text.starts_with("pub") {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn extract_name(&self, node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        self.child_for_field(node, "name")
            .map(|n| self.node_text(&n, source))
    }

    fn extract_line_range(node: &tree_sitter::Node) -> [u32; 2] {
        [node.start_position().row as u32 + 1, node.end_position().row as u32 + 1]
    }

    fn extract_use_import(&self, node: &tree_sitter::Node, source: &[u8]) -> Option<ImportInfo> {
        let argument = self.child_for_field(node, "argument")?;

        let (source_text, specifiers): (String, Vec<String>) = match argument.kind() {
            "identifier" => {
                let name = self.node_text(&argument, source);
                (name.clone(), vec![name])
            }
            "scoped_identifier" => {
                let source_text = self.node_text(&argument, source);
                (source_text.clone(), vec![source_text])
            }
            "primitive_type" => {
                let name = self.node_text(&argument, source);
                (name.clone(), vec![name])
            }
            _ => return None,
        };

        Some(ImportInfo {
            source: source_text,
            specifiers,
            line_number: node.start_position().row as u32 + 1,
        })
    }

    fn extract_function(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        exports: &mut Vec<ExportInfo>,
        enclosing_type: Option<&str>,
    ) {
        let name = match self.extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let params = self.extract_params(self.child_for_field(node, "parameters"), source);
        let return_type = self.extract_return_type(node, source);
        let line_range = Self::extract_line_range(node);
        let is_pub = self.is_public(node, source);
        let visibility = if is_pub { Some("pub".to_string()) } else { None };
        let qualified = match enclosing_type {
            Some(t) => format!("{t}::{name}"),
            None => name.clone(),
        };

        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: qualified,
            line_range,
            params,
            return_type,
            enclosing_class: enclosing_type.map(|s| s.to_string()),
            visibility,
        });

        if enclosing_type.is_none() && is_pub {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_struct(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        classes: &mut Vec<ClassInfo>,
        exports: &mut Vec<ExportInfo>,
    ) {
        let name = match self.extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let mut properties = Vec::new();
        if let Some(body) = self.child_for_field(node, "body") {
            if body.kind() == "field_declaration_list" {
                for field in self.children_by_type(&body, "field_declaration") {
                    if let Some(field_name) = self.child_for_field(&field, "field_identifier") {
                        properties.push(self.node_text(&field_name, source));
                    }
                }
            }
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range: Self::extract_line_range(node),
            methods: Vec::new(),
            properties,
            interface_kind: ClassKind::Struct,
            implemented_interfaces: Vec::new(),
        });

        if self.is_public(node, source) {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_enum(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        classes: &mut Vec<ClassInfo>,
        exports: &mut Vec<ExportInfo>,
    ) {
        let name = match self.extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let mut properties = Vec::new();
        if let Some(body) = self.child_for_field(node, "body") {
            for variant in self.children_by_type(&body, "enum_variant") {
                if let Some(vn) = self.child_for_field(&variant, "name") {
                    properties.push(self.node_text(&vn, source));
                }
            }
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range: Self::extract_line_range(node),
            methods: Vec::new(),
            properties,
            interface_kind: ClassKind::Enum,
            implemented_interfaces: Vec::new(),
        });

        if self.is_public(node, source) {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_trait(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        classes: &mut Vec<ClassInfo>,
        exports: &mut Vec<ExportInfo>,
    ) {
        let name = match self.extract_name(node, source) {
            Some(n) => n,
            None => return,
        };

        let mut methods = Vec::new();
        if let Some(body) = self.child_for_field(node, "declaration_list") {
            for sig in self.children_by_type(&body, "function_signature_item") {
                if let Some(n) = self.child_for_field(&sig, "identifier") {
                    methods.push(self.node_text(&n, source));
                }
            }
            for func in self.children_by_type(&body, "function_item") {
                if let Some(n) = self.extract_name(&func, source) {
                    methods.push(n);
                }
            }
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range: Self::extract_line_range(node),
            methods,
            properties: Vec::new(),
            // Rust traits are most analogous to UA's "Interface"
            // (cannot be instantiated, can have default method
            // bodies). The dashboard will theme them with the
            // interface color.
            interface_kind: ClassKind::Trait,
            implemented_interfaces: Vec::new(),
        });

        if self.is_public(node, source) {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_impl(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        _exports: &mut Vec<ExportInfo>,
        methods_by_type: &mut HashMap<String, Vec<String>>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        // `impl Foo { ... }` or `impl Bar for Foo { ... }`.
        // In the first form, `type` is `Foo` and there's no trait.
        // In the second form, `trait` is `Bar` and `type` is `Foo`.
        let trait_name = self
            .child_for_field(node, "trait")
            .map(|n| self.node_text(&n, source));
        let type_name = self
            .child_for_field(node, "type")
            .map(|n| self.node_text(&n, source));

        // If both present, we have `impl Trait for Type` →
        // emit an `implements` edge (Type conforms to Trait).
        if let (Some(tn), Some(typ)) = (&trait_name, &type_name) {
            inheritances.push(InheritanceInfo {
                subclass: typ.clone(),
                superclass: tn.clone(),
                kind: InheritanceKind::Implements,
                line_number: node.start_position().row as u32 + 1,
            });
        }

        let body = match self.child_for_field(node, "body") {
            Some(b) => b,
            None => return,
        };
        for func in self.children_by_type(&body, "function_item") {
            let name = match self.extract_name(&func, source) {
                Some(n) => n,
                None => continue,
            };

            let params = self.extract_params(self.child_for_field(&func, "parameters"), source);
            let return_type = self.extract_return_type(&func, source);
            let line_range = Self::extract_line_range(&func);
            let visibility = if self.is_public(&func, source) {
                Some("pub".to_string())
            } else {
                None
            };
            let qualified = match &type_name {
                Some(t) => format!("{t}::{name}"),
                None => name.clone(),
            };

            functions.push(FunctionInfo {
                name: name.clone(),
                qualified_name: qualified,
                line_range,
                params,
                return_type,
                enclosing_class: type_name.clone(),
                visibility,
            });

            if let Some(ref tn) = type_name {
                methods_by_type.entry(tn.clone()).or_default().push(name.clone());
            }
        }
    }

    /// Extract callee name from a call_expression node.
    fn extract_callee_name(&self, call_node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        let func_node = call_node.child(0)?;

        match func_node.kind() {
            "identifier" => {
                let text = self.node_text(&func_node, source);
                // Skip common utility methods that create noise
                if Self::is_utility_method(&text) {
                    return None;
                }
                Some(text)
            }
            "field_expression" => {
                let field = self.child_for_field(&func_node, "field")?;
                let value = self.child_for_field(&func_node, "value")?;
                let field_text = self.node_text(&field, source);
                // Skip common utility methods
                if Self::is_utility_method(&field_text) {
                    return None;
                }
                Some(format!("{}.{}", self.node_text(&value, source), field_text))
            }
            "method_expression" => {
                // method_expression has receiver and method: "ok".to_string
                let method = self.child_for_field(&func_node, "method")?;
                let method_text = self.node_text(&method, source);
                if Self::is_utility_method(&method_text) {
                    return None;
                }
                Some(method_text)
            }
            "scoped_identifier" => Some(self.node_text(&func_node, source)),
            _ => {
                let text = self.node_text(&func_node, source);
                // Skip if it contains whitespace, parens, braces, or looks like an expression
                if text.contains(' ') || text.contains('\n') || text.contains('(')
                    || text.contains(')') || text.contains('{') || text.contains('}')
                    || text.contains('[') || text.contains(']')
                    || text.starts_with('"') || text.starts_with('\'')
                    || text.contains("::") {
                    return None;
                }
                Some(text)
            }
        }
    }

    /// Returns true if this is a common utility method that creates noise in the call graph.
    fn is_utility_method(name: &str) -> bool {
        matches!(name,
            "to_string" | "to_owned" | "to_vec" | "clone" | "cloned" | "copied" |
            "as_ref" | "as_mut" | "unwrap" | "unwrap_or" | "unwrap_or_else" |
            "expect" | "ok" | "err" | "Some" | "None" | "Ok" | "Err" |
            "map" | "and_then" | "or_else" | "unwrap_err" |
            "is_none" | "is_some" | "is_ok" | "is_err" |
            "len" | "is_empty" | "get" | "insert" | "remove" |
            "push" | "pop" | "new" | "default" |
            "println" | "print" | "eprintln" | "eprint" |
            "format" | "format!" |
            "vec" | "vec!" | "string" | "stringify" |
            "into_iter" | "iter" | "iter_mut" | "next" |
            "collect" | "fold" | "reduce" |
            "split" | "trim" | "to_uppercase" | "to_lowercase" |
            "contains" | "find" | "replace" | "starts_with" | "ends_with" |
            "read" | "write" | "open" | "close"
        )
    }

    fn walk_and_extract(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        classes: &mut Vec<ClassInfo>,
        imports: &mut Vec<ImportInfo>,
        exports: &mut Vec<ExportInfo>,
        methods_by_type: &mut HashMap<String, Vec<String>>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        match node.kind() {
            "function_item" => self.extract_function(node, source, functions, exports, None),
            "struct_item" => self.extract_struct(node, source, classes, exports),
            "enum_item" => self.extract_enum(node, source, classes, exports),
            "trait_item" => self.extract_trait(node, source, classes, exports),
            "impl_item" => self.extract_impl(
                node,
                source,
                functions,
                exports,
                methods_by_type,
                inheritances,
            ),
            "use_declaration" => {
                if let Some(imp) = self.extract_use_import(node, source) {
                    imports.push(imp);
                }
            }
            _ => {}
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_and_extract(
                    &child,
                    source,
                    functions,
                    classes,
                    imports,
                    exports,
                    methods_by_type,
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

        if node.kind() == "function_item" {
            if let Some(name_node) = self.child_for_field(node, "name") {
                function_stack.push(self.node_text(&name_node, source));
                pushed = true;
            }
        }

        if node.kind() == "call_expression" {
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

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_for_calls(&child, source, function_stack, entries);
            }
        }

        if pushed {
            function_stack.pop();
        }
    }
}

impl Default for RustExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageExtractor for RustExtractor {
    fn language_ids(&self) -> &[&'static str] {
        &["rust"]
    }

    fn extract_structure(&self, root: &tree_sitter::Node, source: &[u8]) -> StructuralAnalysis {
        let mut functions = Vec::new();
        let mut classes = Vec::new();
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut methods_by_type: HashMap<String, Vec<String>> = HashMap::new();
        let mut inheritances: Vec<InheritanceInfo> = Vec::new();

        self.walk_and_extract(
            root,
            source,
            &mut functions,
            &mut classes,
            &mut imports,
            &mut exports,
            &mut methods_by_type,
            &mut inheritances,
        );

        // Attach collected impl methods to their corresponding structs/enums/traits
        for cls in &mut classes {
            if let Some(methods) = methods_by_type.get(&cls.name) {
                cls.methods.extend(methods.clone());
            }
        }

        StructuralAnalysis { functions, classes, imports, exports, inheritances }
    }

    fn extract_call_graph(&self, root: &tree_sitter::Node, source: &[u8]) -> Vec<CallGraphEntry> {
        let mut entries = Vec::new();
        let mut function_stack: Vec<String> = Vec::new();

        self.walk_for_calls(root, source, &mut function_stack, &mut entries);

        entries
    }
}

/// Singleton instance.
pub static RUST_EXTRACTOR: RustExtractor = RustExtractor;
