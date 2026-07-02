// Bash / POSIX shell AST extractor for tree-sitter.
//
// Supported node types:
//   - function_definition (POSIX: `NAME() { ... }`)
//   - function_definition with `function` keyword
//     (Bash-specific: `function NAME { ... }`)
//   - command_substitution / pipeline / command — recorded as
//     call sites
//
// Bash has no classes. Every function is a free function;
// qualified_name == bare name. Visibility is reduced to two
// values: `public` (unprefixed script-level function) and
// `local` (function declared with `local`).

use std::collections::HashSet;

use super::{CallGraphEntry, ClassInfo, ExportInfo, FunctionInfo, ImportInfo,
            LanguageExtractor, StructuralAnalysis};

pub struct BashExtractor;

impl BashExtractor {
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

    fn extract_function(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        exports: &mut Vec<ExportInfo>,
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
        let func_text = Self::node_text(node, source);
        let visibility = if func_text.contains("local") {
            Some("local".to_string())
        } else {
            Some("public".to_string())
        };
        functions.push(FunctionInfo {
            name: name.clone(),
            qualified_name: name.clone(),
            line_range,
            params: Vec::new(),
            return_type: None,
            enclosing_class: None,
            visibility: visibility.clone(),
        });
        if visibility.as_deref() == Some("public") {
            exports.push(ExportInfo {
                name,
                line_number: node.start_position().row as u32 + 1,
                is_default: false,
            });
        }
    }

    fn extract_command_callee(name: &str, source: &[u8]) -> Option<String> {
        if name.is_empty() {
            return None;
        }
        // command_node has a name like `command_name` or
        // `word`/`string`; we don't need a tree-sitter node for
        // the simple case.
        let _ = source;
        Some(name.to_string())
    }

    fn is_utility(name: &str) -> bool {
        let mut set: HashSet<&str> = HashSet::new();
        for s in [
            "cd", "echo", "pwd", "true", "false", ":", "test", "[", "]",
            "set", "export", "unset", "local", "read", "printf", "exit",
            "return", "shift", "wait", "trap", "kill", "type", "alias",
            "unalias", "declare", "typeset", "readonly", "declare -a",
            "declare -A", "declare -i", "declare -x",
            "if", "then", "else", "elif", "fi", "for", "while", "until",
            "do", "done", "case", "esac", "function", "select", "in",
        ] {
            set.insert(s);
        }
        set.contains(name)
    }
}

impl Default for BashExtractor { fn default() -> Self { Self::new() } }

impl LanguageExtractor for BashExtractor {
    fn language_ids(&self) -> &[&'static str] { &["bash", "shell"] }

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

impl BashExtractor {
    fn walk_and_extract(
        &self,
        node: &tree_sitter::Node,
        source: &[u8],
        functions: &mut Vec<FunctionInfo>,
        classes: &mut Vec<ClassInfo>,
        imports: &mut Vec<ImportInfo>,
        exports: &mut Vec<ExportInfo>,
    ) {
        if node.kind() == "function_definition" {
            self.extract_function(node, source, functions, exports);
            return;
        }
        // `source` (POSIX: `.`) and `source` builtin load
        // another script — record as an import.
        if node.kind() == "command" {
            let cmd_text = Self::node_text(node, source);
            let trimmed = cmd_text.trim_start();
            if let Some(rest) = trimmed.strip_prefix("source ").or_else(|| trimmed.strip_prefix(". ")) {
                let arg = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !arg.is_empty() {
                    imports.push(ImportInfo {
                        source: arg.trim_matches('"').trim_matches('\'').to_string(),
                        specifiers: Vec::new(),
                        line_number: node.start_position().row as u32 + 1,
                    });
                }
            }
            let _ = cmd_text;
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
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = Self::node_text(&name_node, source);
                    if !name.is_empty() {
                        function_stack.push(name);
                        pushed = true;
                    }
                }
            }
            "command" => {
                if let Some(caller) = function_stack.last() {
                    // The first non-whitespace child node name is the command name.
                    let cmd_name = node
                        .child_by_field_name("name")
                        .map(|n| Self::node_text(&n, source))
                        .or_else(|| {
                            // Fallback: scan children for the
                            // first `command_name`-like node.
                            for i in 0..node.child_count() {
                                if let Some(child) = node.child(i) {
                                    let kind = child.kind();
                                    if matches!(kind, "command_name" | "word" | "string"
                                                | "raw_string" | "concatenation")
                                    {
                                        return Some(Self::node_text(&child, source));
                                    }
                                }
                            }
                            None
                        });
                    if let Some(name) = cmd_name {
                        if let Some(callee) = Self::extract_command_callee(&name, source) {
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

pub static BASH_EXTRACTOR: BashExtractor = BashExtractor;
