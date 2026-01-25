use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tauri::Emitter;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct FrameFile {
    pub path: String,
    pub name: String,
    pub index: u32,
}

fn extract_frame_index(stem: &str, fallback: u32) -> u32 {
    if stem.starts_with("frame_") {
        stem.strip_prefix("frame_")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    } else {
        let num_str: String = stem.chars().filter(|c| c.is_ascii_digit()).collect();
        num_str.parse::<u32>().unwrap_or(fallback)
    }
}

fn scan_frames_in_dir(dir: &PathBuf) -> Result<Vec<FrameFile>, String> {
    // Check if the path is a file (single frame) or directory
    if dir.is_file() {
        if let Some(ext) = dir.extension().and_then(|e| e.to_str()) {
            if ext == "txt" || ext == "cframe" {
                if let Some(file_name) = dir.file_name().and_then(|n| n.to_str()) {
                    // Always use .txt path as canonical reference
                    let txt_path = dir.with_extension("txt");
                    return Ok(vec![FrameFile {
                        path: txt_path.to_string_lossy().to_string(),
                        name: file_name.to_string(),
                        index: 0,
                    }]);
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
                                    let index = extract_frame_index(stem, frames.len() as u32);
                                    // Use .txt path as canonical reference
                                    let txt_path = dir.join(format!("{}.txt", stem));
                                    frames.push(FrameFile {
                                        path: txt_path.to_string_lossy().to_string(),
                                        name: format!("{}.txt", stem),
                                        index,
                                    });
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
        return Err(format!("Neither .txt nor .cframe file exists for: {}", file_path));
    }

    let data = fs::read(&cframe_path)
        .map_err(|e| format!("Failed to read cframe file: {}", e))?;

    if data.len() < 8 {
        return Err("CFrame file too small (missing header)".to_string());
    }

    let width = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let height = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let pixel_count = width * height;
    let expected_size = 8 + pixel_count * 4;

    if data.len() < expected_size {
        return Err(format!("CFrame file size mismatch: expected {} bytes, got {}", expected_size, data.len()));
    }

    // Reconstruct text with newlines from cframe chars
    let mut text = String::with_capacity(pixel_count + height);
    for row in 0..height {
        for col in 0..width {
            let idx = row * width + col;
            let offset = 8 + idx * 4;
            let ch = data[offset] as char;
            text.push(ch);
        }
        text.push('\n');
    }

    Ok(text)
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

    let data = fs::read(&colors_path)
        .map_err(|e| format!("Failed to read colors file: {}", e))?;

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

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct CFrameData {
    pub width: u32,
    pub height: u32,
    pub chars: Vec<u8>,  // ASCII characters (width*height)
    pub rgb: Vec<u8>,    // RGB flat array (width*height*3)
}

/// Given a .txt frame file path, look for a matching .cframe file and read it.
/// The .cframe binary format: 4 bytes width (u32 LE) + 4 bytes height (u32 LE)
/// + width*height*4 bytes (char, r, g, b per position).
#[tauri::command]
fn read_cframe_file(txt_file_path: String) -> Result<Option<CFrameData>, String> {
    let txt_path = PathBuf::from(&txt_file_path);
    let cframe_path = txt_path.with_extension("cframe");

    if !cframe_path.exists() {
        return Ok(None);
    }

    let data = fs::read(&cframe_path)
        .map_err(|e| format!("Failed to read cframe file: {}", e))?;

    if data.len() < 8 {
        return Err("CFrame file too small (missing header)".to_string());
    }

    let width = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let height = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let pixel_count = width as usize * height as usize;
    let expected_size = 8 + pixel_count * 4;

    if data.len() < expected_size {
        return Err(format!(
            "CFrame file size mismatch: expected {} bytes, got {}",
            expected_size,
            data.len()
        ));
    }

    let mut chars = Vec::with_capacity(pixel_count);
    let mut rgb = Vec::with_capacity(pixel_count * 3);

    for i in 0..pixel_count {
        let offset = 8 + i * 4;
        chars.push(data[offset]);      // char
        rgb.push(data[offset + 1]);    // r
        rgb.push(data[offset + 2]);    // g
        rgb.push(data[offset + 3]);    // b
    }

    Ok(Some(CFrameData { width, height, chars, rgb }))
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

    let data = fs::read(&path)
        .map_err(|e| format!("Failed to read audio file: {}", e))?;

    use base64::{Engine as _, engine::general_purpose::STANDARD};
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
