// Language extractors registry.
// Each extractor handles a specific language's tree-sitter AST.

use std::collections::HashMap;
use std::sync::Arc;

// Re-export the structural types so per-language extractors can
// import them via `super::{ClassKind, InheritanceKind}` without
// dragging in cross-language dependencies.
pub(super) use super::{ClassKind, InheritanceKind};

use super::{CallGraphEntry, ClassInfo, ExportInfo, FunctionInfo, ImportInfo,
            InheritanceInfo, StructuralAnalysis};

mod rust_extractor;
mod typescript_extractor;
mod python_extractor;
mod go_extractor;

// Re-export extractors
pub use rust_extractor::RustExtractor;
pub use typescript_extractor::TypeScriptExtractor;
pub use python_extractor::PythonExtractor;
pub use go_extractor::GoExtractor;

/// Language extractor trait.
/// Mirrors UA's `LanguageExtractor` TypeScript interface.
pub trait LanguageExtractor: Send + Sync {
    /// Language IDs this extractor handles (e.g. ["rust"], ["typescript", "tsx"])
    fn language_ids(&self) -> &[&'static str];

    /// Walk the AST root node and extract all structural elements.
    /// The `source` parameter is the original file content bytes, used to extract
    /// text from nodes via utf8_text.
    fn extract_structure(&self, root: &tree_sitter::Node, source: &[u8]) -> StructuralAnalysis;

    /// Walk the AST root node and extract call-graph edges.
    fn extract_call_graph(&self, root: &tree_sitter::Node, source: &[u8]) -> Vec<CallGraphEntry>;
}

/// Create a new extractor instance for a given language.
pub fn create_extractor(lang: &str) -> Option<Arc<dyn LanguageExtractor>> {
    match lang {
        "rust" => Some(Arc::new(RustExtractor::new())),
        "typescript" | "tsx" | "javascript" | "jsx" => Some(Arc::new(TypeScriptExtractor::new())),
        "python" => Some(Arc::new(PythonExtractor::new())),
        "go" => Some(Arc::new(GoExtractor::new())),
        _ => None,
    }
}

/// Register all built-in extractors into the map.
pub fn register_extractors(
    map: &mut HashMap<&'static str, Arc<dyn LanguageExtractor>>,
) {
    map.insert("rust", Arc::new(RustExtractor::new()));
    map.insert("typescript", Arc::new(TypeScriptExtractor::new()));
    map.insert("tsx", Arc::new(TypeScriptExtractor::new()));
    map.insert("javascript", Arc::new(TypeScriptExtractor::new()));
    map.insert("jsx", Arc::new(TypeScriptExtractor::new()));
    map.insert("python", Arc::new(PythonExtractor::new()));
    map.insert("go", Arc::new(GoExtractor::new()));
}