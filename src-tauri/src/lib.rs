use std::fs;
use std::path::PathBuf;
use tauri::Emitter;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct FrameFile {
    pub path: String,
    pub name: String,
    pub index: u32,
}

fn scan_frames_in_dir(dir: &PathBuf) -> Result<Vec<FrameFile>, String> {
    // Check if the path is a file (single frame) or directory
    if dir.is_file() {
        // Single file dropped - check if it's a .txt file
        if let Some(ext) = dir.extension().and_then(|e| e.to_str()) {
            if ext == "txt" {
                if let Some(file_name) = dir.file_name().and_then(|n| n.to_str()) {
                    return Ok(vec![FrameFile {
                        path: dir.to_string_lossy().to_string(),
                        name: file_name.to_string(),
                        index: 0,
                    }]);
                }
            }
        }
        return Err("Dropped file is not a .txt file".to_string());
    }

    if !dir.exists() {
        return Err("Directory does not exist".to_string());
    }

    let mut frames = Vec::new();

    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if ext == "txt" {
                            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                                // Extract frame index from filename (e.g., "frame_0001.txt" -> 1)
                                let index = if file_name.starts_with("frame_") {
                                    file_name
                                        .strip_prefix("frame_")
                                        .and_then(|s| s.strip_suffix(".txt"))
                                        .and_then(|s| s.parse::<u32>().ok())
                                        .unwrap_or(0)
                                } else {
                                    // Try to extract number from filename
                                    let num_str: String = file_name
                                        .chars()
                                        .filter(|c| c.is_ascii_digit())
                                        .collect();
                                    num_str.parse::<u32>().unwrap_or(frames.len() as u32)
                                };

                                frames.push(FrameFile {
                                    path: path.to_string_lossy().to_string(),
                                    name: file_name.to_string(),
                                    index,
                                });
                            }
                        }
                    }
                }
            }
        }
        Err(e) => return Err(format!("Failed to read directory: {}", e)),
    }

    // Sort by index, then by name for stable ordering
    frames.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.name.cmp(&b.name)));
    Ok(frames)
}

#[tauri::command]
fn get_frame_files(directory_path: String) -> Result<Vec<FrameFile>, String> {
    let dir = PathBuf::from(&directory_path);
    scan_frames_in_dir(&dir)
}

#[tauri::command]
fn read_frame_file(file_path: String) -> Result<String, String> {
    fs::read_to_string(&file_path).map_err(|e| format!("Failed to read frame file: {}", e))
}

#[tauri::command]
fn get_frame_count(directory_path: String) -> Result<usize, String> {
    let dir = PathBuf::from(&directory_path);
    let frames = scan_frames_in_dir(&dir)?;
    Ok(frames.len())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_frame_files,
            read_frame_file,
            get_frame_count
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                if let Some(path) = paths.first() {
                    let path_str = path.to_string_lossy().to_string();
                    let _ = window.emit("file-drop", path_str);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
