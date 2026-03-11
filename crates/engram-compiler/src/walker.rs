use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::parser::ParseError;

/// Walks root/.brv/context-tree/**/*.md and returns all .md file paths
/// sorted lexicographically for deterministic ordering.
pub fn walk_context_tree(root: &Path) -> Result<Vec<PathBuf>, ParseError> {
    let context_tree = root.join(".brv").join("context-tree");

    if !context_tree.exists() {
        return Err(ParseError::IoError {
            path: context_tree.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} does not exist", context_tree.display()),
            ),
        });
    }

    let mut paths: Vec<PathBuf> = WalkDir::new(&context_tree)
        .into_iter()
        .filter_entry(|e| {
            // Skip hidden files and directories
            e.file_name()
                .to_str()
                .map(|s| !s.starts_with('.'))
                .unwrap_or(false)
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "md")
                .unwrap_or(false)
        })
        .map(|e| e.into_path())
        .collect();

    paths.sort();
    Ok(paths)
}
