use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

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

    write_file_utf8(&full_path, &content)
        .map_err(|e| format!("Failed to write file '{}': {}", full_path.display(), e))?;

    Ok(full_path.to_string_lossy().into_owned())
}

fn write_file_utf8(path: &PathBuf, content: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let file = File::options()
            .create(true)
            .write(true)
            .truncate(true)
            .access_mode(0x40000000) // GENERIC_WRITE
            .open(path)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(content.as_bytes())?;
        writer.flush()
    }

    #[cfg(not(windows))]
    {
        fs::write(path, content)
    }
}
