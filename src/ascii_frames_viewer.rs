use gloo_timers::callback::Interval;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::closure::Closure;
use yew::prelude::*;

#[wasm_bindgen(inline_js = r#"
export async function tauriInvoke(cmd, args) {
  const g = globalThis.__TAURI__;
  if (g?.core?.invoke) return g.core.invoke(cmd, args);
  if (g?.tauri?.invoke) return g.tauri.invoke(cmd, args);
  throw new Error('Tauri invoke is not available');
}

export function observeResize(element, callback) {
  const observer = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const { width, height } = entry.contentRect;
      callback(width, height);
    }
  });
  observer.observe(element);
  return observer;
}

export function disconnectObserver(observer) {
  observer.disconnect();
}

"#)]
extern "C" {
    #[wasm_bindgen(js_name = tauriInvoke)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> JsValue;

    #[wasm_bindgen(js_name = observeResize)]
    fn observe_resize(element: &web_sys::Element, callback: &Closure<dyn Fn(f64, f64)>) -> JsValue;

    #[wasm_bindgen(js_name = disconnectObserver)]
    fn disconnect_observer(observer: &JsValue);

}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct FrameFile {
    path: String,
    name: String,
    index: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct CFrameData {
    width: u32,
    height: u32,
    chars: Vec<u8>,
    rgb: Vec<u8>,
}

/// A loaded frame: text content + optional cframe data for canvas rendering
#[derive(Clone, Debug)]
struct Frame {
    content: String,
    cframe: Option<CFrameData>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct ProjectDetails {
    fps: Option<u32>,
    has_audio: bool,
    audio_path: Option<String>,
}

#[derive(Properties, PartialEq, Clone)]
pub struct AsciiFramesViewerProps {
    pub directory_path: String,
    #[prop_or(24)]
    pub fps: u32,
    #[prop_or(true)]
    pub loop_enabled: bool,
    #[prop_or_default]
    pub on_clear: Callback<()>,
}

#[function_component(AsciiFramesViewer)]
pub fn ascii_frames_viewer(props: &AsciiFramesViewerProps) -> Html {
    let frames = use_state(|| Vec::<Frame>::new());
    let current_index = use_state(|| 0usize);
    let current_index_ref = use_mut_ref(|| 0usize);
    let is_playing = use_state(|| false);
    let is_loading = use_state(|| true);
    let error_message = use_state(|| None::<String>);
    let loading_progress = use_state(|| (0, 0));
    let interval_handle: Rc<RefCell<Option<Interval>>> = use_mut_ref(|| None);

    // Color display toggle
    let color_enabled = use_state(|| false);

    // Auto-sizing state
    let container_ref = use_node_ref();
    let content_ref = use_node_ref();
    let canvas_ref = use_node_ref();
    let calculated_font_size = use_state(|| 10.0f64);
    let container_size = use_state(|| (0.0f64, 0.0f64));

    // FPS control
    let current_fps = use_state(|| props.fps);

    // Audio state
    let audio_ref = use_node_ref();
    let audio_src = use_state(|| None::<String>);
    let audio_volume = use_state(|| 0.5f64);
    let audio_muted = use_state(|| false);

    // Overlay visibility toggle
    let overlay_hidden = use_state(|| false);

    // Hover state for showing controls when overlay is hidden
    let is_hovering = use_state(|| false);

    // Sync ref when current_index state changes
    {
        let current_index_ref = current_index_ref.clone();
        use_effect_with(*current_index, move |idx| {
            *current_index_ref.borrow_mut() = *idx;
            || ()
        });
    }

    // Load frames when directory_path changes
    {
        let directory_path = props.directory_path.clone();
        let frames = frames.clone();
        let is_loading = is_loading.clone();
        let error_message = error_message.clone();
        let current_index = current_index.clone();
        let interval_handle = interval_handle.clone();
        let is_playing = is_playing.clone();
        let loading_progress = loading_progress.clone();
        let current_fps = current_fps.clone();
        let audio_src = audio_src.clone();

        use_effect_with(directory_path.clone(), move |_| {
            loading_progress.set((0, 0));
            is_loading.set(true);
            error_message.set(None);
            frames.set(Vec::new());
            current_index.set(0);
            is_playing.set(false);
            audio_src.set(None);

            interval_handle.borrow_mut().take();

            if directory_path.is_empty() {
                is_loading.set(false);
            } else {
                wasm_bindgen_futures::spawn_local(async move {
                    // Load project details (FPS, audio path)
                    let details_args =
                        serde_wasm_bindgen::to_value(&json!({ "directoryPath": directory_path }))
                            .unwrap();
                    if let Ok(details) = serde_wasm_bindgen::from_value::<ProjectDetails>(
                        tauri_invoke("read_project_details", details_args).await
                    ) {
                        if let Some(fps) = details.fps {
                            current_fps.set(fps);
                        }
                        if let Some(audio_path) = details.audio_path {
                            // Load audio as data URL
                            let audio_args =
                                serde_wasm_bindgen::to_value(&json!({ "audioPath": audio_path }))
                                    .unwrap();
                            if let Ok(data_url) = serde_wasm_bindgen::from_value::<String>(
                                tauri_invoke("read_audio_file", audio_args).await
                            ) {
                                audio_src.set(Some(data_url));
                            }
                        }
                    }

                    // Get total frame count
                    let count_args =
                        serde_wasm_bindgen::to_value(&json!({ "directoryPath": directory_path }))
                            .unwrap();
                    let total_frames = match tauri_invoke("get_frame_count", count_args).await {
                        result => serde_wasm_bindgen::from_value::<usize>(result).unwrap_or(0),
                    };

                    // Get list of frame files
                    let args =
                        serde_wasm_bindgen::to_value(&json!({ "directoryPath": directory_path }))
                            .unwrap();
                    match tauri_invoke("get_frame_files", args).await {
                        result => {
                            match serde_wasm_bindgen::from_value::<Vec<FrameFile>>(result) {
                                Ok(frame_files) => {
                                    let total_count = if total_frames > 0 {
                                        total_frames
                                    } else {
                                        frame_files.len()
                                    };
                                    loading_progress.set((0, total_count));

                                    let mut loaded_frames = Vec::new();
                                    for (i, frame_file) in frame_files.into_iter().enumerate() {
                                        // Load frame text
                                        let args = serde_wasm_bindgen::to_value(
                                            &json!({ "filePath": frame_file.path }),
                                        )
                                        .unwrap();
                                        let content = match tauri_invoke("read_frame_file", args).await {
                                            result => {
                                                match serde_wasm_bindgen::from_value::<String>(result) {
                                                    Ok(c) => c,
                                                    Err(e) => {
                                                        error_message.set(Some(format!(
                                                            "Failed to read frame {}: {:?}",
                                                            frame_file.name, e
                                                        )));
                                                        break;
                                                    }
                                                }
                                            }
                                        };

                                        // Try to load matching .cframe file for canvas rendering
                                        let cframe_args = serde_wasm_bindgen::to_value(
                                            &json!({ "txtFilePath": frame_file.path }),
                                        )
                                        .unwrap();
                                        let cframe = match tauri_invoke("read_cframe_file", cframe_args).await {
                                            result => {
                                                serde_wasm_bindgen::from_value::<Option<CFrameData>>(result)
                                                    .unwrap_or(None)
                                            }
                                        };

                                        loaded_frames.push(Frame { content, cframe });
                                        loading_progress.set((i + 1, total_count));
                                    }

                                    if loaded_frames.is_empty() {
                                        error_message
                                            .set(Some("No frames found in directory".to_string()));
                                    } else {
                                        frames.set(loaded_frames);
                                    }
                                    is_loading.set(false);
                                }
                                Err(e) => {
                                    error_message
                                        .set(Some(format!("Failed to list frames: {:?}", e)));
                                    is_loading.set(false);
                                }
                            }
                        }
                    }
                });
            }

            || ()
        });
    }

    // Animation effect
    {
        let current_index = current_index.clone();
        let current_index_ref = current_index_ref.clone();
        let is_playing_state = is_playing.clone();
        let frames = frames.clone();
        let interval_handle = interval_handle.clone();
        let loop_enabled = props.loop_enabled;
        let playing = *is_playing;
        let frame_count = frames.len();
        let fps = *current_fps;

        use_effect_with((playing, fps, frame_count), move |_| {
            interval_handle.borrow_mut().take();

            if playing && frame_count > 0 {
                let interval_ms = (1000.0 / fps as f64).max(1.0) as u32;
                let current_index_clone = current_index.clone();
                let current_index_ref_clone = current_index_ref.clone();
                let is_playing_clone = is_playing_state.clone();
                let interval_handle_clone = interval_handle.clone();

                let interval = Interval::new(interval_ms, move || {
                    let mut current = *current_index_ref_clone.borrow();

                    if current >= frame_count - 1 {
                        if loop_enabled {
                            current = 0;
                            *current_index_ref_clone.borrow_mut() = current;
                            current_index_clone.set(current);
                        } else {
                            interval_handle_clone.borrow_mut().take();
                            is_playing_clone.set(false);
                        }
                    } else {
                        current += 1;
                        *current_index_ref_clone.borrow_mut() = current;
                        current_index_clone.set(current);
                    }
                });

                *interval_handle.borrow_mut() = Some(interval);
            }

            || ()
        });
    }

    // Audio playback control - sync with frame playback
    {
        let audio_ref = audio_ref.clone();
        let playing = *is_playing;
        let current_frame_idx = *current_index;
        let frame_count = frames.len();
        let fps = *current_fps;
        let has_audio = audio_src.is_some();

        use_effect_with((playing, has_audio), move |_| {
            if has_audio {
                if let Some(audio) = audio_ref.cast::<web_sys::HtmlAudioElement>() {
                    if playing {
                        // Calculate the time position based on current frame
                        if frame_count > 0 && fps > 0 {
                            let target_time = current_frame_idx as f64 / fps as f64;
                            // Only seek if we're significantly out of sync (> 0.1s)
                            let current_time = audio.current_time();
                            if (current_time - target_time).abs() > 0.1 {
                                audio.set_current_time(target_time);
                            }
                        }
                        let _ = audio.play();
                    } else {
                        audio.pause().ok();
                    }
                }
            }
            || ()
        });
    }

    // Volume and mute control effect
    {
        let audio_ref = audio_ref.clone();
        let volume = *audio_volume;
        let muted = *audio_muted;

        use_effect_with((volume, muted), move |_| {
            if let Some(audio) = audio_ref.cast::<web_sys::HtmlAudioElement>() {
                audio.set_volume(volume);
                audio.set_muted(muted);
            }
            || ()
        });
    }

    // ResizeObserver to track container size changes
    {
        let container_ref = container_ref.clone();
        let container_size = container_size.clone();
        let observer_handle: Rc<RefCell<Option<JsValue>>> = use_mut_ref(|| None);

        use_effect_with(container_ref.clone(), move |container_ref| {
            let container_size = container_size.clone();
            let observer_handle = observer_handle.clone();

            if let Some(element) = container_ref.cast::<web_sys::Element>() {
                let container_size_clone = container_size.clone();
                let closure = Closure::wrap(Box::new(move |width: f64, height: f64| {
                    container_size_clone.set((width, height));
                }) as Box<dyn Fn(f64, f64)>);

                let observer = observe_resize(&element, &closure);
                *observer_handle.borrow_mut() = Some(observer);

                closure.forget();
            }

            move || {
                if let Some(obs) = observer_handle.borrow_mut().take() {
                    disconnect_observer(&obs);
                }
            }
        });
    }

    // Auto-size font to fit container
    {
        let frames = frames.clone();
        let calculated_font_size = calculated_font_size.clone();
        let is_loading = is_loading.clone();
        let container_width = container_size.0;
        let container_height = container_size.1;

        use_effect_with(
            (
                frames.len(),
                (*is_loading).clone(),
                container_width as i32,
                container_height as i32,
            ),
            move |_| {
                if frames.is_empty() {
                    return;
                }

                if let Some(first_frame) = frames.first() {
                    let lines: Vec<&str> = first_frame.content.lines().collect();
                    let row_count = lines.len();
                    let col_count = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);

                    if row_count == 0 || col_count == 0 {
                        return;
                    }

                    let available_width = container_width - 20.0;
                    let available_height = container_height - 20.0;

                    if available_width <= 0.0 || available_height <= 0.0 {
                        return;
                    }

                    let char_width_ratio = 0.6;
                    let line_height_ratio = 1.11;

                    let max_font_from_width =
                        available_width / (col_count as f64 * char_width_ratio);
                    let max_font_from_height =
                        available_height / (row_count as f64 * line_height_ratio);

                    let optimal_font_size = max_font_from_width.min(max_font_from_height);
                    let clamped_font_size = optimal_font_size.max(1.0).min(50.0);

                    calculated_font_size.set(clamped_font_size);
                }
            },
        );
    }

    // Update frame content: canvas for colored mode, text for plain mode
    {
        let content_ref = content_ref.clone();
        let canvas_ref = canvas_ref.clone();
        let frames = frames.clone();
        let color_enabled = *color_enabled;
        let current_frame_idx = (*current_index).min(frames.len().saturating_sub(1));
        let font_size = *calculated_font_size;

        use_effect_with((current_frame_idx, color_enabled, frames.len(), (font_size * 100.0) as i32), move |_| {
            if let Some(frame) = frames.get(current_frame_idx) {
                if color_enabled {
                    if let Some(ref cframe) = frame.cframe {
                        // Draw on canvas
                        if let Some(canvas) = canvas_ref.cast::<web_sys::HtmlCanvasElement>() {
                            let char_width = font_size * 0.6;
                            let line_height = font_size * 1.11;
                            let canvas_w = (cframe.width as f64 * char_width).ceil() as u32;
                            let canvas_h = (cframe.height as f64 * line_height).ceil() as u32;
                            canvas.set_width(canvas_w);
                            canvas.set_height(canvas_h);

                            if let Ok(Some(ctx_obj)) = canvas.get_context("2d") {
                                if let Ok(ctx) = ctx_obj.dyn_into::<web_sys::CanvasRenderingContext2d>() {
                                    ctx.clear_rect(0.0, 0.0, canvas_w as f64, canvas_h as f64);
                                    let font_str = format!("{:.2}px monospace", font_size);
                                    ctx.set_font(&font_str);
                                    ctx.set_text_baseline("top");

                                    let width = cframe.width as usize;
                                    let height = cframe.height as usize;

                                    for row in 0..height {
                                        let mut col = 0;
                                        while col < width {
                                            let idx = row * width + col;
                                            let ch = cframe.chars[idx];
                                            let r = cframe.rgb[idx * 3];
                                            let g = cframe.rgb[idx * 3 + 1];
                                            let b = cframe.rgb[idx * 3 + 2];

                                            // Skip spaces and very dark chars
                                            if ch == b' ' || (r < 5 && g < 5 && b < 5) {
                                                col += 1;
                                                continue;
                                            }

                                            // Batch consecutive chars with same color
                                            let mut batch = String::new();
                                            batch.push(ch as char);
                                            let start_col = col;
                                            col += 1;

                                            while col < width {
                                                let next_idx = row * width + col;
                                                let next_ch = cframe.chars[next_idx];
                                                let nr = cframe.rgb[next_idx * 3];
                                                let ng = cframe.rgb[next_idx * 3 + 1];
                                                let nb = cframe.rgb[next_idx * 3 + 2];

                                                if nr == r && ng == g && nb == b
                                                    && next_ch != b' '
                                                    && !(nr < 5 && ng < 5 && nb < 5)
                                                {
                                                    batch.push(next_ch as char);
                                                    col += 1;
                                                } else {
                                                    break;
                                                }
                                            }

                                            let color_str = format!("rgb({},{},{})", r, g, b);
                                            ctx.set_fill_style_str(&color_str);
                                            let x = start_col as f64 * char_width;
                                            let y = row as f64 * line_height;
                                            let _ = ctx.fill_text(&batch, x, y);
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        // No cframe data, fall back to plain text
                        if let Some(element) = content_ref.cast::<web_sys::HtmlElement>() {
                            element.set_text_content(Some(&frame.content));
                        }
                    }
                } else {
                    // Plain text mode
                    if let Some(element) = content_ref.cast::<web_sys::HtmlElement>() {
                        element.set_text_content(Some(&frame.content));
                    }
                }
            }
            || ()
        });
    }

    let on_toggle_play = {
        let is_playing = is_playing.clone();
        Callback::from(move |_| {
            is_playing.set(!*is_playing);
        })
    };

    let on_seek = {
        let current_index = current_index.clone();
        let is_playing = is_playing.clone();
        let frames = frames.clone();
        let audio_ref = audio_ref.clone();
        let fps = *current_fps;
        Callback::from(move |e: web_sys::InputEvent| {
            if let Some(target) = e.target() {
                if let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() {
                    let slider_val = input.value_as_number();
                    if slider_val.is_finite() {
                        let frame_count = frames.len();
                        if frame_count > 0 {
                            let target_frame =
                                (slider_val.clamp(0.0, 1.0) * (frame_count - 1) as f64).round()
                                    as usize;
                            is_playing.set(false);
                            current_index.set(target_frame);

                            // Seek audio to match frame
                            if let Some(audio) = audio_ref.cast::<web_sys::HtmlAudioElement>() {
                                let target_time = target_frame as f64 / fps as f64;
                                audio.set_current_time(target_time);
                            }
                        }
                    }
                }
            }
        })
    };

    let on_fps_change = {
        let current_fps = current_fps.clone();
        Callback::from(move |e: web_sys::InputEvent| {
            if let Some(target) = e.target() {
                if let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() {
                    if let Ok(fps) = input.value().parse::<u32>() {
                        if fps > 0 {
                            current_fps.set(fps);
                        }
                    }
                }
            }
        })
    };

    let on_volume_change = {
        let audio_volume = audio_volume.clone();
        Callback::from(move |e: web_sys::InputEvent| {
            if let Some(target) = e.target() {
                if let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() {
                    let vol = input.value_as_number();
                    if vol.is_finite() {
                        audio_volume.set(vol.clamp(0.0, 1.0));
                    }
                }
            }
        })
    };

    let on_toggle_mute = {
        let audio_muted = audio_muted.clone();
        Callback::from(move |_| {
            audio_muted.set(!*audio_muted);
        })
    };

    let on_step_backward = {
        let current_index = current_index.clone();
        let frames = frames.clone();
        let is_playing = is_playing.clone();
        Callback::from(move |_| {
            if *is_playing {
                is_playing.set(false);
            }
            let frame_count = frames.len();
            if frame_count > 0 {
                let current = *current_index;
                let prev = if current == 0 {
                    frame_count - 1
                } else {
                    current - 1
                };
                current_index.set(prev);
            }
        })
    };

    let on_step_forward = {
        let current_index = current_index.clone();
        let frames = frames.clone();
        let is_playing = is_playing.clone();
        Callback::from(move |_| {
            if *is_playing {
                is_playing.set(false);
            }
            let frame_count = frames.len();
            if frame_count > 0 {
                let current = *current_index;
                let next = if current >= frame_count - 1 {
                    0
                } else {
                    current + 1
                };
                current_index.set(next);
            }
        })
    };

    let frame_count = frames.len();
    let current_frame = (*current_index).min(frame_count.saturating_sub(1));
    let progress = if frame_count > 1 {
        current_frame as f64 / (frame_count - 1) as f64
    } else {
        0.0
    };

    let loading_message = {
        let (loaded, total) = *loading_progress;
        if total > 0 {
            let percentage = (loaded as f32 / total as f32 * 100.0) as i32;
            format!("Loading frames... {} / {} ({}%)", loaded, total, percentage)
        } else {
            "Loading frames...".to_string()
        }
    };

    let font_size_style = format!("font-size: {:.2}px;", *calculated_font_size);

    // SVG icons (Lucide-style)
    let play_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 3 20 12 6 21 6 3"></polygon></svg>"#
    ));
    let pause_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="14" y="4" width="4" height="16" rx="1"></rect><rect x="6" y="4" width="4" height="16" rx="1"></rect></svg>"#
    ));
    let skip_forward_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 4 15 12 5 20 5 4"></polygon><line x1="19" y1="5" x2="19" y2="19"></line></svg>"#
    ));
    let skip_backward_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="19 20 9 12 19 4 19 20"></polygon><line x1="5" y1="19" x2="5" y2="5"></line></svg>"#
    ));

    let color_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9.06 11.9 8.07-8.06a2.85 2.85 0 1 1 4.03 4.03l-8.06 8.08"></path><path d="M7.07 14.94c-1.66 0-3 1.35-3 3.02 0 1.33-2.5 1.52-2 2.02 1.08 1.1 2.49 2.02 4 2.02 2.2 0 4-1.8 4-4.04a3.01 3.01 0 0 0-3-3.02z"></path></svg>"#
    ));
    let volume_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"></path><path d="M16 9a5 5 0 0 1 0 6"></path><path d="M19.364 18.364a9 9 0 0 0 0-12.728"></path></svg>"#
    ));
    let mute_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"></path><line x1="22" x2="16" y1="9" y2="15"></line><line x1="16" x2="22" y1="9" y2="15"></line></svg>"#
    ));
    let circle_x_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"></circle><path d="m15 9-6 6"></path><path d="m9 9 6 6"></path></svg>"#
    ));
    let eye_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"></path><circle cx="12" cy="12" r="3"></circle></svg>"#
    ));
    let eye_off_svg = Html::from_html_unchecked(AttrValue::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49"></path><path d="M14.084 14.158a3 3 0 0 1-4.242-4.242"></path><path d="M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143"></path><path d="m2 2 20 20"></path></svg>"#
    ));

    let on_toggle_color = {
        let color_enabled = color_enabled.clone();
        Callback::from(move |_| {
            color_enabled.set(!*color_enabled);
        })
    };

    let on_clear_click = {
        let on_clear = props.on_clear.clone();
        Callback::from(move |_| {
            on_clear.emit(());
        })
    };

    let on_toggle_overlay = {
        let overlay_hidden = overlay_hidden.clone();
        Callback::from(move |_| {
            overlay_hidden.set(!*overlay_hidden);
        })
    };

    let on_mouse_enter = {
        let is_hovering = is_hovering.clone();
        Callback::from(move |_: web_sys::MouseEvent| {
            is_hovering.set(true);
        })
    };

    let on_mouse_leave = {
        let is_hovering = is_hovering.clone();
        Callback::from(move |_: web_sys::MouseEvent| {
            is_hovering.set(false);
        })
    };

    let play_pause_icon = if *is_playing { pause_svg } else { play_svg };

    // Check if any loaded frame has color data
    let color_available = frame_count > 0
        && frames.iter().any(|f| f.cframe.is_some());

    let has_colors = *color_enabled && color_available
        && frames
            .get(current_frame)
            .map(|f| f.cframe.is_some())
            .unwrap_or(false);

    let audio_data_url = (*audio_src).clone().unwrap_or_default();
    let has_audio = audio_src.is_some();
    let mute_icon = if *audio_muted { mute_svg } else { volume_svg };
    let overlay_icon = if *overlay_hidden { eye_off_svg } else { eye_svg };

    let viewer_class = if *overlay_hidden { "ascii-frames-viewer fullscreen-mode" } else { "ascii-frames-viewer" };
    let show_controls = !*overlay_hidden || *is_hovering;

    html! {
        <div class={viewer_class} onmouseenter={on_mouse_enter} onmouseleave={on_mouse_leave}>
            if has_audio {
                <audio ref={audio_ref} src={audio_data_url} preload="auto" style="display: none;"></audio>
            }
            <div class="frames-display" ref={container_ref}>
                if *is_loading {
                    <div class="loading-frames">{loading_message}</div>
                } else if let Some(error) = &*error_message {
                    <div class="error-frames">{error}</div>
                } else if frames.is_empty() {
                    <div class="no-frames">{"No frames available"}</div>
                } else {
                    if has_colors {
                        <canvas ref={canvas_ref.clone()} class="ascii-frame-canvas"></canvas>
                    } else {
                        <pre class="ascii-frame-content" style={font_size_style.clone()} ref={content_ref.clone()}></pre>
                    }
                    if !*overlay_hidden {
                        <div class="frame-info-overlay">
                            <span class="info-left">{format!("FPS: {}", *current_fps)}</span>
                            <span class="info-center">{format!("{}/{}", current_frame + 1, frame_count)}</span>
                            <span class="info-right">{if has_colors { "Color" } else { "" }}</span>
                        </div>
                    }
                }
            </div>

            if show_controls {
                <div class="controls">
                    // Row 1: Progress bar + Play/Pause button (only for multiple frames)
                    if frame_count > 1 {
                        <div class="control-row">
                            <input id="progress-slider" class="progress" type="range" min="0" max="1" step="0.001" value={progress.to_string()} oninput={on_seek} disabled={frame_count == 0} />
                            <button id="play-pause-btn" class="ctrl-btn play-btn" type="button" onclick={on_toggle_play} disabled={frame_count == 0} title={if *is_playing { "Pause" } else { "Play" }}>{play_pause_icon}</button>
                        </div>
                    }

                    // Row 2: Volume slider + Mute button (only if audio)
                    if has_audio {
                        <div class="control-row">
                            <input id="volume-slider" class="volume-slider" type="range" min="0" max="1" step="0.01" value={audio_volume.to_string()} oninput={on_volume_change} />
                            <button id="mute-btn" class={if *audio_muted { "ctrl-btn mute-btn muted" } else { "ctrl-btn mute-btn" }} type="button" onclick={on_toggle_mute} title={if *audio_muted { "Unmute" } else { "Mute" }}>{mute_icon}</button>
                        </div>
                    }

                    // Row 3: FPS, color, clear, forward/backward buttons
                    <div class="control-row">
                        if frame_count > 1 {
                            <label>{"FPS:"}</label>
                            <input id="fps-input" type="number" class="fps-input" value={current_fps.to_string()} min="1" oninput={on_fps_change} />
                        }
                        <button id="color-btn" class={if *color_enabled && color_available { "ctrl-btn color-btn active" } else if !color_available { "ctrl-btn color-btn disabled" } else { "ctrl-btn color-btn" }} type="button" onclick={on_toggle_color} disabled={!color_available} title={if !color_available { "No color data available" } else if *color_enabled { "Color enabled" } else { "Color disabled" }}>{color_svg}</button>
                        <button id="hide-overlay-btn" class={if *overlay_hidden { "ctrl-btn active" } else { "ctrl-btn" }} type="button" onclick={on_toggle_overlay} title={if *overlay_hidden { "Show overlay" } else { "Hide overlay" }}>{overlay_icon}</button>
                        <button id="clear-btn" class="ctrl-btn" type="button" onclick={on_clear_click} title="Clear">{circle_x_svg}</button>
                        if frame_count > 1 {
                            <div style="flex: 1;"></div>
                            <button id="step-backward-btn" class="ctrl-btn" type="button" onclick={on_step_backward} disabled={frame_count == 0} title="Step backward">{skip_backward_svg}</button>
                            <button id="step-forward-btn" class="ctrl-btn" type="button" onclick={on_step_forward} disabled={frame_count == 0} title="Step forward">{skip_forward_svg}</button>
                        }
                    </div>
                </div>
            }
        </div>
    }
}
