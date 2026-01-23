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
struct ColorData {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

/// A loaded frame: text content + optional color data
#[derive(Clone, Debug)]
struct Frame {
    content: String,
    colors: Option<ColorData>,
}

#[derive(Properties, PartialEq, Clone)]
pub struct AsciiFramesViewerProps {
    pub directory_path: String,
    #[prop_or(24)]
    pub fps: u32,
    #[prop_or(true)]
    pub loop_enabled: bool,
}

/// Build an HTML string with colored spans for each character
fn build_colored_html(content: &str, colors: &ColorData) -> String {
    let mut html = String::with_capacity(content.len() * 40);
    let mut row: u32 = 0;
    let mut col: u32 = 0;

    for ch in content.chars() {
        if ch == '\n' {
            html.push('\n');
            row += 1;
            col = 0;
            continue;
        }

        if row < colors.height && col < colors.width {
            let idx = ((row * colors.width + col) * 3) as usize;
            if idx + 2 < colors.rgb.len() {
                let r = colors.rgb[idx];
                let g = colors.rgb[idx + 1];
                let b = colors.rgb[idx + 2];
                // Skip styling for spaces or very dark colors (both r,g,b < 5)
                if ch == ' ' || (r < 5 && g < 5 && b < 5) {
                    html.push(ch);
                } else {
                    html.push_str(&format!(
                        "<span style=\"color:rgb({},{},{})\">",
                        r, g, b
                    ));
                    // Escape HTML entities
                    match ch {
                        '<' => html.push_str("&lt;"),
                        '>' => html.push_str("&gt;"),
                        '&' => html.push_str("&amp;"),
                        '"' => html.push_str("&quot;"),
                        _ => html.push(ch),
                    }
                    html.push_str("</span>");
                }
            } else {
                html.push(ch);
            }
        } else {
            html.push(ch);
        }

        col += 1;
    }

    html
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

    // Auto-sizing state
    let container_ref = use_node_ref();
    let calculated_font_size = use_state(|| 10.0f64);
    let container_size = use_state(|| (0.0f64, 0.0f64));

    // FPS control
    let current_fps = use_state(|| props.fps);

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

        use_effect_with(directory_path.clone(), move |_| {
            loading_progress.set((0, 0));
            is_loading.set(true);
            error_message.set(None);
            frames.set(Vec::new());
            current_index.set(0);
            is_playing.set(false);

            interval_handle.borrow_mut().take();

            if directory_path.is_empty() {
                is_loading.set(false);
            } else {
                wasm_bindgen_futures::spawn_local(async move {
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

                                        // Try to load matching .colors file
                                        let colors_args = serde_wasm_bindgen::to_value(
                                            &json!({ "txtFilePath": frame_file.path }),
                                        )
                                        .unwrap();
                                        let colors = match tauri_invoke("read_colors_file", colors_args).await {
                                            result => {
                                                serde_wasm_bindgen::from_value::<Option<ColorData>>(result)
                                                    .unwrap_or(None)
                                            }
                                        };

                                        loaded_frames.push(Frame { content, colors });
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
    let play_icon = if *is_playing { "||" } else { ">" };

    // Build frame content (colored or plain)
    let frame_html = if frame_count > 0 {
        if let Some(frame) = frames.get(current_frame) {
            if let Some(ref colors) = frame.colors {
                // Render with colors using raw HTML
                let colored = build_colored_html(&frame.content, colors);
                Html::from_html_unchecked(AttrValue::from(colored))
            } else {
                // Plain text
                Html::from(frame.content.clone())
            }
        } else {
            Html::from("")
        }
    } else {
        Html::from("")
    };

    let has_colors = frame_count > 0
        && frames
            .get(current_frame)
            .map(|f| f.colors.is_some())
            .unwrap_or(false);

    html! {
        <div class="ascii-frames-viewer">
            <div class="frames-display" ref={container_ref}>
                if *is_loading {
                    <div class="loading-frames">{loading_message}</div>
                } else if let Some(error) = &*error_message {
                    <div class="error-frames">{error}</div>
                } else if frames.is_empty() {
                    <div class="no-frames">{"No frames available"}</div>
                } else {
                    <pre class="ascii-frame-content" style={font_size_style}>{
                        frame_html
                    }</pre>
                    <div class="frame-info-overlay">
                        <span class="info-left">{format!("FPS: {}", *current_fps)}</span>
                        <span class="info-center">{format!("{}/{}", current_frame + 1, frame_count)}</span>
                        <span class="info-right">{if has_colors { "Color" } else { "" }}</span>
                    </div>
                }
            </div>

            <div class="controls">
                <div class="control-row">
                    <input
                        class="progress"
                        type="range"
                        min="0"
                        max="1"
                        step="0.001"
                        value={progress.to_string()}
                        oninput={on_seek}
                        disabled={frame_count == 0}
                    />
                    <button
                        class="ctrl-btn"
                        type="button"
                        onclick={on_toggle_play}
                        disabled={frame_count == 0}
                        title="Play/Pause"
                    >
                        {play_icon}
                    </button>
                </div>

                <div class="control-row">
                    <label>{"FPS:"}</label>
                    <input
                        type="number"
                        class="fps-input"
                        value={current_fps.to_string()}
                        min="1"
                        oninput={on_fps_change}
                    />
                    <div style="flex: 1;"></div>
                    <button
                        class="ctrl-btn"
                        type="button"
                        onclick={on_step_backward}
                        disabled={frame_count == 0}
                        title="Step backward"
                    >
                        {"<"}
                    </button>
                    <button
                        class="ctrl-btn"
                        type="button"
                        onclick={on_step_forward}
                        disabled={frame_count == 0}
                        title="Step forward"
                    >
                        {">"}
                    </button>
                </div>
            </div>
        </div>
    }
}
