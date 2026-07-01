// Go AST extractor for tree-sitter.
//
// Supported node types:
//   - function_declaration     → FunctionInfo
//   - method_declaration       → FunctionInfo (track receiver)
//   - type_declaration         → ClassInfo (type_spec with struct_type or interface_type)
//   - import_declaration       → ImportInfo
//   - call_expression          → CallGraphEntry (via extract_call_graph)

use std::collections::HashMap;

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, InheritanceKind, LanguageExtractor, StructuralAnalysis};

pub struct GoExtractor;

impl GoExtractor {
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

    fn extract_name(&self, node: tree_sitter::Node) -> Option<String> {
        self.child_by_field(&node, "name")
            .map(|n| n.to_string())
    }

    fn extract_line_range(node: &tree_sitter::Node) -> [u32; 2] {
        [node.start_position().row as u32 + 1, node.end_position().row as u32 + 1]
    }

    fn extract_params(&self, params_node: Option<tree_sitter::Node>) -> Vec<String> {
        let Some(params_node) = params_node else { return Vec::new() };
        let mut params = Vec::new();

        // Go parameters can have multiple identifiers sharing one type
        // e.g., (a, b, c int) - parameter_declaration contains multiple identifiers
        let mut current_names: Vec<String> = Vec::new();

        for i in 0..params_node.child_count() {
            if let Some(child) = params_node.child(i) {
                match child.kind() {
                    "identifier" => {
                        current_names.push(child.to_string());
                    }
                    "parameter_declaration" => {
                        // Extract names and type
                        let mut names: Vec<String> = Vec::new();
                        let mut has_variadic = false;

                        for j in 0..child.child_count() {
                            if let Some(param_child) = child.child(j) {
                                match param_child.kind() {
                                    "identifier" => {
                                        names.push(param_child.to_string());
                                    }
                                    "variadic_parameter_declaration" => {
                                        // ...name
                                        has_variadic = true;
                                        if let Some(name_node) = self.child_by_field(&param_child, "name") {
                                            names.push(format!("...{}", name_node.to_string()));
                                        }
                                    }
                                    "parameter_list" => {
                                        // Nested parameter list
                                        for k in 0..param_child.child_count() {
                                            if let Some(p) = param_child.child(k) {
                                                if p.kind() == "identifier" {
                                                    names.push(p.to_string());
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }

                        for name in names {
                            if has_variadic && name.starts_with("...") {
                                params.push(name);
                            } else if has_variadic {
                                params.push(format!("...{}", name));
                            } else {
                                params.push(name);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        params
    }

    fn extract_result(&self, node: &tree_sitter::Node) -> Option<String> {
        self.child_by_field(node, "result")
            .map(|n| n.to_string())
    }

    fn is_exported(&self, name: &str) -> bool {
        if let Some(first) = name.chars().next() {
            first.is_uppercase()
        } else {
            false
        }
    }

    fn extract_function(&self, node: &tree_sitter::Node, functions: &mut Vec<FunctionInfo>, exports: &mut Vec<ExportInfo>, enclosing_class: Option<&str>) {
        let name = match self.extract_name(node.clone()) {
            Some(n) => n,
            None => return,
        };

        let params = self.extract_params(self.child_by_field(node, "parameters"));
        let return_type = self.extract_result(node);
        let line_range = Self::extract_line_range(node);
        let qualified = match enclosing_class {
            Some(c) => format!("{c}.{name}"),
            None => name.clone(),
        };
        let visibility = if self.is_exported(&name) {
            Some("public".to_string())
        } else {
            Some("private".to_string())
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

        if enclosing_class.is_none() && self.is_exported(&name) {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_method(&self, node: &tree_sitter::Node, functions: &mut Vec<FunctionInfo>, exports: &mut Vec<ExportInfo>, methods_by_receiver: &mut HashMap<String, Vec<String>>, enclosing_class: Option<&str>) {
        let name = match self.extract_name(node.clone()) {
            Some(n) => n,
            None => return,
        };

        let params = self.extract_params(self.child_by_field(node, "parameters"));
        let return_type = self.extract_result(node);
        let line_range = Self::extract_line_range(node);

        // Find receiver type from `(r *Type)` or `(r Type)`.
        let receiver_type = if let Some(receiver) = self.child_by_field(node, "receiver") {
            let mut found: Option<String> = None;
            for i in 0..receiver.child_count() {
                if let Some(child) = receiver.child(i) {
                    if child.kind() == "parameter_declaration" {
                        if let Some(type_node) = self.child_by_field(&child, "type") {
                            found = Some(type_node.to_string());
                            break;
                        }
                    }
                }
            }
            found
        } else {
            None
        };

        // Prefer the discovered receiver type; fall back to
        // enclosing_class when the receiver is unspecified.
        let effective_receiver = receiver_type.clone().or_else(|| enclosing_class.map(|s| s.to_string()));

        let qualified = match &effective_receiver {
            Some(t) => format!("{t}.{name}"),
            None => name.clone(),
        };
        let visibility = if self.is_exported(&name) {
            Some("public".to_string())
        } else {
            Some("private".to_string())
        };

        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: qualified,
            line_range,
            params,
            return_type,
            enclosing_class: effective_receiver.clone(),
            visibility,
        });

        if let Some(ref t) = effective_receiver {
            methods_by_receiver.entry(t.clone()).or_default().push(name.clone());
        }

        if self.is_exported(&name) {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_type(&self, node: &tree_sitter::Node, classes: &mut Vec<ClassInfo>, exports: &mut Vec<ExportInfo>, inheritances: &mut Vec<InheritanceInfo>) {
        let name = match self.extract_name(node.clone()) {
            Some(n) => n,
            None => return,
        };

        let line_range = Self::extract_line_range(node);
        let mut methods = Vec::new();
        let mut properties = Vec::new();

        // Check if it's a struct or interface
        if let Some(type_expr) = self.child_by_field(&node, "type") {
            match type_expr.kind() {
                "struct_type" => {
                    if let Some(field_list) = self.child_by_field(&type_expr, "declarations") {
                        for i in 0..field_list.child_count() {
                            if let Some(field) = field_list.child(i) {
                                if let Some(name_node) = self.child_by_field(&field, "name") {
                                    properties.push(name_node.to_string());
                                }
                            }
                        }
                    }
                }
                "interface_type" => {
                    if let Some(body) = self.child_by_field(&type_expr, "body") {
                        for i in 0..body.child_count() {
                            if let Some(method) = body.child(i) {
                                if let Some(name_node) = self.child_by_field(&method, "name") {
                                    methods.push(name_node.to_string());
                                }
                            }
                        }
                    }
                    // Go interface embedding: `type Foo interface { Bar }` embeds Bar
                    if let Some(body) = self.child_by_field(&type_expr, "body") {
                        for i in 0..body.child_count() {
                            if let Some(inner) = body.child(i) {
                                if inner.kind() == "type_identifier" {
                                    inheritances.push(InheritanceInfo {
                                        subclass: name.clone(),
                                        superclass: inner.to_string(),
                                        kind: InheritanceKind::Inherits,
                                        line_number: node.start_position().row as u32 + 1,
                                    });
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            methods,
            properties,
            // Default to Struct; the caller patches Interface
            // when the type expression is an interface_type.
            interface_kind: {
                if let Some(type_expr) = self.child_by_field(&node, "type") {
                    if type_expr.kind() == "interface_type" {
                        ClassKind::Interface
                    } else {
                        ClassKind::Struct
                    }
                } else {
                    ClassKind::Struct
                }
            },
            implemented_interfaces: Vec::new(),
        });

        if self.is_exported(&name) {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_import(&self, node: &tree_sitter::Node, imports: &mut Vec<ImportInfo>) {
        let mut source = String::new();
        let mut specifiers = Vec::new();

        if let Some(import_spec_list) = self.child_by_field(node, "import_spec_list") {
            for i in 0..import_spec_list.child_count() {
                if let Some(spec) = import_spec_list.child(i) {
                    match spec.kind() {
                        "import_spec" => {
                            if let Some(path) = self.child_by_field(&spec, "path") {
                                source = path.to_string().trim_matches('"').to_string();
                            }
                            if let Some(name) = self.child_by_field(&spec, "name") {
                                specifiers.push(name.to_string());
                            } else {
                                // Use last path component as imported name
                                if !source.is_empty() {
                                    if let Some(base) = source.rsplit('/').next() {
                                        let clean = base.trim_matches('"');
                                        if !clean.is_empty() {
                                            specifiers.push(clean.to_string());
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        } else {
            // Single import spec
            if let Some(path) = self.child_by_field(node, "path") {
                source = path.to_string().trim_matches('"').to_string();
            }
            if let Some(name) = self.child_by_field(node, "name") {
                specifiers.push(name.to_string());
            }
        }

        if source.is_empty() {
            return;
        }

        imports.push(ImportInfo {
            source,
            specifiers,
            line_number: node.start_position().row as u32 + 1,
        });
    }

    fn extract_callee_name(&self, call_node: &tree_sitter::Node) -> Option<String> {
        let func_node = call_node.child(0)?;

        match func_node.kind() {
            "identifier" => Some(func_node.to_string()),
            "selector_expression" => {
                let field = self.child_by_field(&func_node, "field")?;
                let operand = self.child_by_field(&func_node, "operand")?;
                Some(format!("{}.{}", operand.to_string(), field.to_string()))
            }
            "call_expression" => self.extract_callee_name(&func_node),
            _ => Some(func_node.to_string()),
        }
    }
}

impl Default for GoExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageExtractor for GoExtractor {
    fn language_ids(&self) -> &[&'static str] {
        &["go"]
    }

    fn extract_structure(&self, root: &tree_sitter::Node, _source: &[u8]) -> StructuralAnalysis {
        let mut functions = Vec::new();
        let mut classes = Vec::new();
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut inheritances = Vec::new();
        let mut methods_by_receiver: HashMap<String, Vec<String>> = HashMap::new();

        self.walk_and_extract(
            root,
            None,
            &mut functions,
            &mut classes,
            &mut imports,
            &mut exports,
            &mut methods_by_receiver,
            &mut inheritances,
        );

        // Attach methods to their receiver types
        for cls in &mut classes {
            if let Some(methods) = methods_by_receiver.get(&cls.name) {
                cls.methods.extend(methods.clone());
            }
        }

        StructuralAnalysis { functions, classes, imports, exports, inheritances }
    }

    fn extract_call_graph(&self, root: &tree_sitter::Node, _source: &[u8]) -> Vec<CallGraphEntry> {
        let mut entries = Vec::new();
        let mut function_stack: Vec<String> = Vec::new();

        self.walk_for_calls(root, None, &mut function_stack, &mut entries);

        entries
    }
}

impl GoExtractor {
    #[allow(clippy::too_many_arguments)]
    fn walk_and_extract(
        &self,
        node: &tree_sitter::Node,
        enclosing_class: Option<&str>,
        functions: &mut Vec<FunctionInfo>,
        classes: &mut Vec<ClassInfo>,
        imports: &mut Vec<ImportInfo>,
        exports: &mut Vec<ExportInfo>,
        methods_by_receiver: &mut HashMap<String, Vec<String>>,
        inheritances: &mut Vec<InheritanceInfo>,
    ) {
        match node.kind() {
            "function_declaration" => {
                self.extract_function(node, functions, exports, enclosing_class);
            }
            "method_declaration" => {
                self.extract_method(node, functions, exports, methods_by_receiver, enclosing_class);
            }
            "type_declaration" => {
                // type Foo struct { ... } or type Foo interface { ... }
                if let Some(type_spec_list) = self.child_by_field(node, "type_spec_list") {
                    for i in 0..type_spec_list.child_count() {
                        if let Some(type_spec) = type_spec_list.child(i) {
                            if let Some(type_name) = self.extract_name(type_spec.clone()) {
                                self.extract_type(&type_spec, classes, exports, inheritances);
                                // Walk with enclosing_class set so
                                // methods declared alongside the
                                // type (rare but possible) qualify
                                // themselves.
                                for j in 0..type_spec.child_count() {
                                    if let Some(child) = type_spec.child(j) {
                                        self.walk_and_extract(
                                            &child,
                                            Some(&type_name),
                                            functions,
                                            classes,
                                            imports,
                                            exports,
                                            methods_by_receiver,
                                            inheritances,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "import_declaration" => {
                self.extract_import(node, imports);
            }
            _ => {}
        }

        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_and_extract(
                    &child,
                    enclosing_class,
                    functions,
                    classes,
                    imports,
                    exports,
                    methods_by_receiver,
                    inheritances,
                );
            }
        }
    }

    fn walk_for_calls(
        &self,
        node: &tree_sitter::Node,
        enclosing_class: Option<&str>,
        function_stack: &mut Vec<String>,
        entries: &mut Vec<CallGraphEntry>,
    ) {
        let mut pushed = false;

        match node.kind() {
            "function_declaration" => {
                if let Some(name_node) = self.child_by_field(node, "name") {
                    let name = name_node.to_string();
                    let qname = match enclosing_class {
                        Some(c) => format!("{c}.{name}"),
                        None => name,
                    };
                    function_stack.push(qname);
                    pushed = true;
                }
            }
            "method_declaration" => {
                if let Some(name_node) = self.child_by_field(node, "name") {
                    let name = name_node.to_string();
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
                    if let Some(callee) = self.extract_callee_name(node) {
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
                self.walk_for_calls(&child, enclosing_class, function_stack, entries);
            }
        }

        if pushed {
            function_stack.pop();
        }
    }
}

/// Singleton instance.
pub static GO_EXTRACTOR: GoExtractor = GoExtractor;
