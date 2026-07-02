// C AST extractor for tree-sitter.
//
// Supported node types:
//   - function_definition    → FunctionInfo (with optional static / extern /
//                              inline modifiers)
//   - declaration / struct_specifier / union_specifier / enum_specifier
//                          → ClassInfo (struct → Struct, union → Union,
//                              enum → Enum; bare enums emit only the parent)
//   - preproc_include        → ImportInfo
//   - preproc_function_def   → ImportInfo / macro alias (treated as import)
//   - call_expression        → CallGraphEntry (via extract_call_graph)
//
// C has no class/method hierarchy — methods bound to a `struct` are
// rare and detected only by convention (`fn p` with `self` first
// param). For MVP we treat every `function_definition` as a free
// function; struct methods would require a separate pass anyway.

use std::collections::HashSet;

use super::{CallGraphEntry, ClassInfo, ClassKind, ExportInfo, FunctionInfo, ImportInfo,
            LanguageExtractor, StructuralAnalysis};

pub struct CExtractor;

impl CExtractor {
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

    fn declarator_name(declarator: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        let name_field = declarator.child_by_field_name("declarator")?;
        // Recurse one level: function_definition → declarator → pointer_declarator?
        if name_field.kind() == "pointer_declarator" || name_field.kind() == "array_declarator" {
            if let Some(inner) = name_field.child_by_field_name("declarator") {
                return Some(Self::node_text(&inner, source));
            }
        }
        Some(Self::node_text(&name_field, source))
    }

    fn type_text(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
        node.child_by_field_name("type")
            .map(|n| Self::node_text(&n, source))
    }

    /// Pull parameters from a `function_declarator`'s `parameters` field.
    fn params_from_fn(fn_declarator: &tree_sitter::Node, source: &[u8]) -> Vec<String> {
        let Some(params) = fn_declarator.child_by_field_name("parameters") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in 0..params.child_count() {
            if let Some(child) = params.child(i) {
                if child.kind() == "parameter_declaration" {
                    out.push(Self::node_text(&child, source).trim().to_string());
                }
            }
        }
        out
    }

    fn extract_function(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        exports: &mut Vec<ExportInfo>,
    ) {
        let declarator = match node.child_by_field_name("declarator") {
            Some(d) => d,
            None => return,
        };

        // The declarator's first field "declarator" may itself be a
        // function_declarator (for prototyped functions) or a
        // pointer_declarator wrapping one. Walk down until we find
        // a function_declarator kind.
        let mut current = declarator;
        let (name, params, return_type) = loop {
            match current.kind() {
                "function_declarator" => {
                    let n = current
                        .child_by_field_name("declarator")
                        .map(|d| Self::node_text(&d, source));
                    let p = Self::params_from_fn(&current, source);
                    let r = Self::type_text(node, source);
                    break (n, p, r);
                }
                "pointer_declarator" | "array_declarator" => {
                    match current.child_by_field_name("declarator") {
                        Some(next) => current = next,
                        None => return,
                    }
                }
                _ => {
                    // Plain identifier; treat as no-op free decl.
                    return;
                }
            }
        };

        let name = match name {
            Some(n) => n,
            None => return,
        };
        if name.is_empty() {
            return;
        }

        // Determine "static" — C free functions without `static`
        // are externally visible by default; we tag them as
        // "public". `static` → "private".
        let func_text = Self::node_text(node, source);
        let visibility = if func_text.trim_start().starts_with("static") {
            "private"
        } else {
            "public"
        };

        let line_range = Self::line_range(node);

        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            params,
            return_type,
            enclosing_class: None,
            visibility: Some(visibility.to_string()),
        });

        if visibility == "public" {
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
        kind: ClassKind,
        source: &[u8],
        classes: &mut Vec<ClassInfo>,
    ) {
        let name_node = match node.child_by_field_name("name") {
            Some(n) => n,
            None => return,
        };
        let name = Self::node_text(&name_node, source);
        let line_range = Self::line_range(node);
        let mut properties = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            for i in 0..body.child_count() {
                if let Some(child) = body.child(i) {
                    if child.kind() == "field_declaration" {
                        for j in 0..child.child_count() {
                            if let Some(grand) = child.child(j) {
                                if grand.kind() == "field_identifier" {
                                    properties.push(Self::node_text(&grand, source));
                                } else if grand.kind() == "pointer_field_declarator" {
                                    if let Some(field) = grand.child_by_field_name("declarator") {
                                        properties.push(format!(
                                            "*{}",
                                            Self::node_text(&field, source)
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        classes.push(ClassInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            methods: Vec::new(),
            properties,
            interface_kind: kind,
            implemented_interfaces: Vec::new(),
        });
    }

    fn extract_include(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        imports: &mut Vec<ImportInfo>,
    ) {
        // preproc_include wraps a string_literal or system_lib_string.
        let mut target = String::new();
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if matches!(
                    child.kind(),
                    "string_literal" | "system_lib_string" | "identifier"
                ) {
                    let txt = Self::node_text(&child, source);
                    target = txt
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .trim_matches('"')
                        .to_string();
                    break;
                }
            }
        }
        if target.is_empty() {
            return;
        }
        imports.push(ImportInfo {
            source: target,
            specifiers: Vec::new(),
            line_number: node.start_position().row as u32 + 1,
        });
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
            "malloc", "free", "calloc", "realloc", "memcpy", "memset", "memmove",
            "strlen", "strcmp", "strcpy", "strncpy", "strcat", "printf", "fprintf",
            "sprintf", "snprintf", "scanf", "fscanf", "sscanf", "exit", "abort",
            "assert", "sizeof", "va_start", "va_end", "va_arg", "getenv", "setenv",
            "open", "close", "read", "write", "fopen", "fclose", "fread", "fwrite",
            "fgets", "fputs", "puts", "putchar", "getchar", "fflush",
        ] {
            set.insert(s);
        }
        set.contains(name)
    }
}

impl Default for CExtractor { fn default() -> Self { Self::new() } }

impl LanguageExtractor for CExtractor {
    fn language_ids(&self) -> &[&'static str] { &["c"] }

    fn extract_structure(&self, root: &tree_sitter::Node, source: &[u8]) -> StructuralAnalysis {
        let mut functions = Vec::new();
        let mut classes = Vec::new();
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        self.walk_and_extract(root, source, &mut functions, &mut classes, &mut imports, &mut exports);
        StructuralAnalysis { functions, classes, imports, exports, inheritances: Vec::new() }
    }

    fn extract_call_graph(&self, root: &tree_sitter::Node, source: &[u8]) -> Vec<CallGraphEntry> {
        let mut entries = Vec::new();
        let mut function_stack: Vec<String> = Vec::new();
        self.walk_for_calls(root, source, &mut function_stack, &mut entries);
        entries
    }
}

impl CExtractor {
    fn walk_and_extract(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        classes: &mut Vec<ClassInfo>,
        imports: &mut Vec<ImportInfo>,
        exports: &mut Vec<ExportInfo>,
    ) {
        match node.kind() {
            "function_definition" => self.extract_function(node, source, functions, exports),
            "struct_specifier" => self.extract_struct(node, ClassKind::Struct, source, classes),
            "union_specifier" => self.extract_struct(node, ClassKind::Class, source, classes),
            "enum_specifier" => self.extract_struct(node, ClassKind::Enum, source, classes),
            "preproc_include" | "preproc_function_def" => self.extract_include(node, source, imports),
            _ => {}
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                self.walk_and_extract(&child, source, functions, classes, imports, exports);
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
        match node.kind() {
            "function_definition" => {
                if let Some(decl) = node.child_by_field_name("declarator") {
                    let mut cursor = decl;
                    loop {
                        match cursor.kind() {
                            "function_declarator" => {
                                if let Some(name_node) = cursor.child_by_field_name("declarator") {
                                    let name = Self::node_text(&name_node, source);
                                    if !name.is_empty() {
                                        function_stack.push(name);
                                        pushed = true;
                                    }
                                }
                                break;
                            }
                            "pointer_declarator" | "array_declarator" => {
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
                    if let Some(callee) = Self::extract_callee_name(node, source) {
                        let bare = callee.rsplit('.').next().unwrap_or(&callee).to_string();
                        if !Self::is_utility(&bare) {
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
                self.walk_for_calls(&child, source, function_stack, entries);
            }
        }
        if pushed { function_stack.pop(); }
    }
}

pub static C_EXTRACTOR: CExtractor = CExtractor;
