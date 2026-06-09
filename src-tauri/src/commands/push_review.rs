use std::fs;
use std::path::PathBuf;

#[tauri::command]
pub fn write_push_source(
    project_path: String,
    relative_path: String,
    content: String,
) -> Result<String, String> {
    let full_path = PathBuf::from(&project_path)
        .join("raw")
        .join("sources")
        .join(&relative_path);

    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directories for '{}': {}", full_path.display(), e))?;
    }

    fs::write(&full_path, content)
        .map_err(|e| format!("Failed to write file '{}': {}", full_path.display(), e))?;

    Ok(full_path.to_string_lossy().into_owned())
}
