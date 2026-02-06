use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tauri::Emitter;

// Re-export shared types from cascii-core-view
use cascii_core_view::FrameFile;

fn scan_frames_in_dir(dir: &PathBuf) -> Result<Vec<FrameFile>, String> {
    // Check if the path is a file (single frame) or directory
    if dir.is_file() {
        if let Some(ext) = dir.extension().and_then(|e| e.to_str()) {
            if ext == "txt" || ext == "cframe" {
                if let Some(file_name) = dir.file_name().and_then(|n| n.to_str()) {
                    // Always use .txt path as canonical reference
                    let txt_path = dir.with_extension("txt");
                    return Ok(vec![FrameFile::new(
                        txt_path.to_string_lossy().to_string(),
                        file_name.to_string(),
                        0,
                    )]);
                }
            }
        }
        return Err("Dropped file is not a .txt or .cframe file".to_string());
    }

    if !dir.exists() {
        return Err("Directory does not exist".to_string());
    }

    // Collect unique stems from both .txt and .cframe files
    let mut seen_stems: HashSet<String> = HashSet::new();
    let mut frames = Vec::new();

    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        if ext == "txt" || ext == "cframe" {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                if seen_stems.insert(stem.to_string()) {
                                    let index = FrameFile::extract_index(stem, frames.len() as u32);
                                    // Use .txt path as canonical reference
                                    let txt_path = dir.join(format!("{}.txt", stem));
                                    frames.push(FrameFile::new(
                                        txt_path.to_string_lossy().to_string(),
                                        format!("{}.txt", stem),
                                        index,
                                    ));
                                }
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
    // Try to read .txt file first
    if let Ok(content) = fs::read_to_string(&file_path) {
        return Ok(content);
    }

    // Fall back to extracting text from .cframe file
    let txt_path = PathBuf::from(&file_path);
    let cframe_path = txt_path.with_extension("cframe");

    if !cframe_path.exists() {
        return Err(format!(
            "Neither .txt nor .cframe file exists for: {}",
            file_path
        ));
    }

    let data =
        fs::read(&cframe_path).map_err(|e| format!("Failed to read cframe file: {}", e))?;

    // Use the shared parser to extract text from cframe
    cascii_core_view::parse_cframe_text(&data).map_err(|e| e.to_string())
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ColorData {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>, // flat RGB array: [r,g,b, r,g,b, ...]
}

/// Given a .txt frame file path, look for a matching .colors file and read it.
/// The .colors binary format: 4 bytes width (u32 LE) + 4 bytes height (u32 LE) + width*height*3 bytes RGB.
#[tauri::command]
fn read_colors_file(txt_file_path: String) -> Result<Option<ColorData>, String> {
    let txt_path = PathBuf::from(&txt_file_path);
    let colors_path = txt_path.with_extension("colors");

    if !colors_path.exists() {
        return Ok(None);
    }

    let data =
        fs::read(&colors_path).map_err(|e| format!("Failed to read colors file: {}", e))?;

    if data.len() < 8 {
        return Err("Colors file too small (missing header)".to_string());
    }

    let width = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let height = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let expected_size = 8 + (width as usize * height as usize * 3);

    if data.len() < expected_size {
        return Err(format!(
            "Colors file size mismatch: expected {} bytes, got {}",
            expected_size,
            data.len()
        ));
    }

    let rgb = data[8..expected_size].to_vec();

    Ok(Some(ColorData { width, height, rgb }))
}

/// Given a .txt frame file path, look for a matching .cframe file and return raw bytes as base64.
/// Parsing happens on the WASM side to avoid expensive element-by-element JSON array
/// deserialization of Vec<u8> fields across the JS-WASM boundary.
#[tauri::command]
fn read_cframe_file(txt_file_path: String) -> Result<Option<String>, String> {
    let txt_path = PathBuf::from(&txt_file_path);
    let cframe_path = txt_path.with_extension("cframe");

    if !cframe_path.exists() {
        return Ok(None);
    }

    let data =
        fs::read(&cframe_path).map_err(|e| format!("Failed to read cframe file: {}", e))?;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    Ok(Some(STANDARD.encode(&data)))
}

#[tauri::command]
fn get_frame_count(directory_path: String) -> Result<usize, String> {
    let dir = PathBuf::from(&directory_path);
    let frames = scan_frames_in_dir(&dir)?;
    Ok(frames.len())
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ProjectDetails {
    pub fps: Option<u32>,
    pub has_audio: bool,
    pub audio_path: Option<String>,
}

/// Read audio file and return as base64 for data URL
#[tauri::command]
fn read_audio_file(audio_path: String) -> Result<String, String> {
    let path = PathBuf::from(&audio_path);
    if !path.exists() {
        return Err("Audio file does not exist".to_string());
    }

    let data = fs::read(&path).map_err(|e| format!("Failed to read audio file: {}", e))?;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(&data);

    // Return as data URL
    Ok(format!("data:audio/mpeg;base64,{}", b64))
}

/// Read details.md from the directory and parse project metadata
#[tauri::command]
fn read_project_details(directory_path: String) -> Result<ProjectDetails, String> {
    let dir = PathBuf::from(&directory_path);

    // Handle single file drop - get parent directory
    let dir = if dir.is_file() {
        dir.parent().map(|p| p.to_path_buf()).unwrap_or(dir)
    } else {
        dir
    };

    let details_path = dir.join("details.md");
    let audio_path = dir.join("audio.mp3");

    let mut fps: Option<u32> = None;
    let has_audio = audio_path.exists();
    let audio_path_str = if has_audio {
        Some(audio_path.to_string_lossy().to_string())
    } else {
        None
    };

    // Parse details.md if it exists
    if details_path.exists() {
        if let Ok(content) = fs::read_to_string(&details_path) {
            for line in content.lines() {
                if let Some(value) = line.strip_prefix("FPS:") {
                    fps = value.trim().parse::<u32>().ok();
                }
            }
        }
    }

    Ok(ProjectDetails {
        fps,
        has_audio,
        audio_path: audio_path_str,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_frame_files,
            read_frame_file,
            read_colors_file,
            read_cframe_file,
            get_frame_count,
            read_project_details,
            read_audio_file
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
