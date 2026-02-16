use gloo_timers::callback::Interval;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::VecDeque;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::closure::Closure;
use yew::prelude::*;
use yew_icons::{Icon, IconId};

// Use shared types from cascii-core-view
use cascii_core_view::{
    draw_cached_canvas, draw_frame_from_cache, load_color_frames, load_text_frames,
    render_to_offscreen_canvas, yield_to_event_loop, FontSizing, Frame, FrameCanvasCache,
    FrameDataProvider, FrameFile, LoadResult, LoadingPhase, RenderConfig,
};

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

async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        } else {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

const BW_PLAYBACK_BACKGROUND_SLEEP_MS: i32 = 12;

struct TauriFrameProvider;

impl FrameDataProvider for TauriFrameProvider {
    fn get_frame_files(&self, directory: &str) -> impl std::future::Future<Output = LoadResult<Vec<FrameFile>>> {
        let dir = directory.to_string();
        async move {
            let args =
                serde_wasm_bindgen::to_value(&json!({ "directoryPath": dir })).unwrap();
            serde_wasm_bindgen::from_value::<Vec<FrameFile>>(
                tauri_invoke("get_frame_files", args).await,
            )
            .map_err(|e| format!("Failed to list frames: {:?}", e))
        }
    }

    fn read_frame_text(&self, path: &str) -> impl std::future::Future<Output = LoadResult<String>> {
        let path = path.to_string();
        async move {
            let args =
                serde_wasm_bindgen::to_value(&json!({ "filePath": path })).unwrap();
            serde_wasm_bindgen::from_value::<String>(
                tauri_invoke("read_frame_file", args).await,
            )
            .map_err(|e| format!("Failed to read frame: {:?}", e))
        }
    }

    fn read_cframe_bytes(&self, txt_path: &str) -> impl std::future::Future<Output = LoadResult<Option<Vec<u8>>>> {
        let path = txt_path.to_string();
        async move {
            let args =
                serde_wasm_bindgen::to_value(&json!({ "txtFilePath": path })).unwrap();
            serde_wasm_bindgen::from_value::<Option<Vec<u8>>>(
                tauri_invoke("read_cframe_file", args).await,
            )
            .map_err(|e| format!("Failed to read cframe file: {:?}", e))
        }
    }
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
    // Frame storage - use RefCell to avoid re-renders during color loading
    let frames_ref: Rc<RefCell<Vec<Frame>>> = use_mut_ref(Vec::new);

    // Reactive state for UI updates (phase, progress, errors)
    let loading_phase = use_state(|| LoadingPhase::Idle);
    let loading_error = use_state(|| None::<String>);
    let frame_count = use_state(|| 0usize);
    // Use RefCell (not UseState) so color loading never triggers re-renders.
    // Progress display piggybacks on animation re-renders instead.
    let color_progress: Rc<RefCell<(usize, usize)>> = use_mut_ref(|| (0usize, 0usize));
    let frame_canvas_cache: Rc<RefCell<FrameCanvasCache>> = use_mut_ref(FrameCanvasCache::default);
    let color_cache_queue: Rc<RefCell<VecDeque<usize>>> = use_mut_ref(VecDeque::new);
    let color_loaded_flags: Rc<RefCell<Vec<bool>>> = use_mut_ref(Vec::new);
    let has_any_color = use_state(|| false);
    let has_any_color_flag: Rc<RefCell<bool>> = use_mut_ref(|| false);
    let color_cache_refresh = use_state(|| 0u64);
    let color_cache_worker_id: Rc<RefCell<u64>> = use_mut_ref(|| 0u64);
    let is_playing_ref = use_mut_ref(|| false);
    let color_enabled_ref = use_mut_ref(|| false);
    let loading_phase_ref: Rc<RefCell<LoadingPhase>> = use_mut_ref(|| LoadingPhase::Idle);

    let current_index = use_state(|| 0usize);
    let current_index_ref = use_mut_ref(|| 0usize);
    let is_playing = use_state(|| false);
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

    {
        let is_playing_ref = is_playing_ref.clone();
        use_effect_with(*is_playing, move |playing| {
            *is_playing_ref.borrow_mut() = *playing;
            || ()
        });
    }

    {
        let color_enabled_ref = color_enabled_ref.clone();
        use_effect_with(*color_enabled, move |enabled| {
            *color_enabled_ref.borrow_mut() = *enabled;
            || ()
        });
    }

    {
        let loading_phase_ref = loading_phase_ref.clone();
        use_effect_with(*loading_phase, move |phase| {
            *loading_phase_ref.borrow_mut() = *phase;
            || ()
        });
    }

    // Load frames when directory_path changes
    // Two-phase loading:
    // Phase 1: Load text frames quickly for immediate playback
    // Phase 2: Load color data in background (no re-renders during this phase)
    {
        let directory_path = props.directory_path.clone();
        let frames_ref = frames_ref.clone();
        let loading_phase = loading_phase.clone();
        let loading_error = loading_error.clone();
        let frame_count = frame_count.clone();
        let color_progress = color_progress.clone();
        let current_index = current_index.clone();
        let interval_handle = interval_handle.clone();
        let is_playing = is_playing.clone();
        let current_fps = current_fps.clone();
        let audio_src = audio_src.clone();
        let frame_canvas_cache = frame_canvas_cache.clone();
        let color_cache_queue = color_cache_queue.clone();
        let color_loaded_flags = color_loaded_flags.clone();
        let has_any_color = has_any_color.clone();
        let has_any_color_flag = has_any_color_flag.clone();
        let color_cache_refresh_for_color = color_cache_refresh.clone();
        let current_index_ref_for_color = current_index_ref.clone();
        let color_enabled_for_color = color_enabled.clone();
        let color_cache_worker_id = color_cache_worker_id.clone();
        let is_playing_ref = is_playing_ref.clone();
        let color_enabled_ref = color_enabled_ref.clone();

        use_effect_with(directory_path.clone(), move |_| {
            // Reset state
            frames_ref.borrow_mut().clear();
            frame_count.set(0);
            loading_phase.set(LoadingPhase::Idle);
            loading_error.set(None);
            *color_progress.borrow_mut() = (0, 0);
            frame_canvas_cache.borrow_mut().clear();
            color_cache_queue.borrow_mut().clear();
            color_loaded_flags.borrow_mut().clear();
            has_any_color.set(false);
            *has_any_color_flag.borrow_mut() = false;
            let next_worker_id = color_cache_worker_id.borrow().wrapping_add(1);
            *color_cache_worker_id.borrow_mut() = next_worker_id;
            current_index.set(0);
            is_playing.set(false);
            audio_src.set(None);
            interval_handle.borrow_mut().take();

            if !directory_path.is_empty() {
                loading_phase.set(LoadingPhase::LoadingText);

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

                // Two-phase loading via cascii-core-view orchestrators
                let provider = TauriFrameProvider;
                match load_text_frames(&provider, &directory_path).await {
                    Ok((loaded_frames, frame_files)) => {
                        let total = loaded_frames.len();
                        *frames_ref.borrow_mut() = loaded_frames;
                        *color_loaded_flags.borrow_mut() = vec![false; total];
                        frame_count.set(total);
                        *color_progress.borrow_mut() = (0, total);
                        frame_canvas_cache.borrow_mut().resize(total);
                        loading_phase.set(LoadingPhase::LoadingColors);

                        let frames_for_color = frames_ref.clone();
                        let progress_for_color = color_progress.clone();
                        let _ = load_color_frames(
                            &provider,
                            &frame_files,
                            |i, _total, cf| {
                                if let Some(cframe) = cf {
                                    let mut frames = frames_for_color.borrow_mut();
                                    if i < frames.len() {
                                        frames[i].cframe = Some(cframe);
                                    }
                                    {
                                        let mut loaded_flags = color_loaded_flags.borrow_mut();
                                        if i < loaded_flags.len() {
                                            loaded_flags[i] = true;
                                        }
                                    }
                                    color_cache_queue.borrow_mut().push_back(i);
                                    if !*has_any_color_flag.borrow() {
                                        *has_any_color_flag.borrow_mut() = true;
                                        has_any_color.set(true);
                                    }
                                    if *color_enabled_for_color
                                        && i == *current_index_ref_for_color.borrow()
                                    {
                                        color_cache_refresh_for_color
                                            .set((*color_cache_refresh_for_color).wrapping_add(1));
                                    }
                                }
                                *progress_for_color.borrow_mut() = (i + 1, total);
                            },
                            || {
                                let is_playing_ref = is_playing_ref.clone();
                                let color_enabled_ref = color_enabled_ref.clone();
                                async move {
                                    if *is_playing_ref.borrow() && !*color_enabled_ref.borrow() {
                                        sleep_ms(BW_PLAYBACK_BACKGROUND_SLEEP_MS).await;
                                    } else {
                                        yield_to_event_loop().await;
                                    }
                                }
                            },
                        )
                        .await;
                        loading_phase.set(LoadingPhase::Complete);
                    }
                    Err(e) => {
                        loading_error.set(Some(e));
                        loading_phase.set(LoadingPhase::Idle);
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
        let interval_handle = interval_handle.clone();
        let loop_enabled = props.loop_enabled;
        let playing = *is_playing;
        let total_frames = *frame_count;
        let fps = *current_fps;

        use_effect_with((playing, fps, total_frames), move |_| {
            interval_handle.borrow_mut().take();

            if playing && total_frames > 0 {
                let interval_ms = (1000.0 / fps as f64).max(1.0) as u32;
                let current_index_clone = current_index.clone();
                let current_index_ref_clone = current_index_ref.clone();
                let is_playing_clone = is_playing_state.clone();
                let interval_handle_clone = interval_handle.clone();

                let interval = Interval::new(interval_ms, move || {
                    let mut current = *current_index_ref_clone.borrow();

                    if current >= total_frames - 1 {
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
        let total_frames = *frame_count;
        let fps = *current_fps;
        let has_audio = audio_src.is_some();

        use_effect_with((playing, has_audio), move |_| {
            if has_audio {
                if let Some(audio) = audio_ref.cast::<web_sys::HtmlAudioElement>() {
                    if playing {
                        // Calculate the time position based on current frame
                        if total_frames > 0 && fps > 0 {
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

    // Auto-size font to fit container using cascii-core-view
    {
        let frames_ref = frames_ref.clone();
        let calculated_font_size = calculated_font_size.clone();
        let container_width = container_size.0;
        let container_height = container_size.1;
        let total_frames = *frame_count;
        let phase = *loading_phase;

        use_effect_with(
            (
                total_frames,
                phase,
                container_width as i32,
                container_height as i32,
            ),
            move |_| {
                let frames = frames_ref.borrow();
                if frames.is_empty() {
                    return;
                }

                if let Some(first_frame) = frames.first() {
                    let (cols, rows) = first_frame.dimensions();

                    if rows == 0 || cols == 0 {
                        return;
                    }

                    // Use FontSizing from cascii-core-view
                    let optimal_font_size =
                        FontSizing::calculate(cols, rows, container_width, container_height);
                    calculated_font_size.set(optimal_font_size);
                }
            },
        );
    }

    // Keep the color cache warm in background without hurting B/W playback.
    {
        let frames_ref = frames_ref.clone();
        let frame_canvas_cache = frame_canvas_cache.clone();
        let color_cache_queue = color_cache_queue.clone();
        let color_loaded_flags = color_loaded_flags.clone();
        let color_cache_refresh = color_cache_refresh.clone();
        let color_cache_worker_id = color_cache_worker_id.clone();
        let current_index_ref = current_index_ref.clone();
        let is_playing_ref = is_playing_ref.clone();
        let color_enabled_ref = color_enabled_ref.clone();
        let loading_phase_ref = loading_phase_ref.clone();
        let total_frames = *frame_count;
        let has_any_color_val = *has_any_color;
        let font_size = *calculated_font_size;
        let font_size_key = (font_size * 100.0) as i32;

        use_effect_with((total_frames, has_any_color_val, font_size_key), move |_| {
            if total_frames == 0 || !has_any_color_val {
                return;
            }

            let next_worker_id = color_cache_worker_id.borrow().wrapping_add(1);
            *color_cache_worker_id.borrow_mut() = next_worker_id;

            {
                let mut cache = frame_canvas_cache.borrow_mut();
                cache.resize(total_frames);
                cache.invalidate_for_font_size_key(font_size_key);
            }

            {
                let loaded_flags = color_loaded_flags.borrow();
                let mut queue = color_cache_queue.borrow_mut();
                queue.clear();
                for (idx, loaded) in loaded_flags.iter().enumerate() {
                    if *loaded {
                        queue.push_back(idx);
                    }
                }
            }

            let frames_for_cache = frames_ref.clone();
            let cache_for_cache = frame_canvas_cache.clone();
            let queue_for_cache = color_cache_queue.clone();
            let refresh_for_cache = color_cache_refresh.clone();
            let worker_id_ref = color_cache_worker_id.clone();
            let current_index_ref = current_index_ref.clone();
            let is_playing_ref = is_playing_ref.clone();
            let color_enabled_ref = color_enabled_ref.clone();
            let loading_phase_ref = loading_phase_ref.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loop {
                    if *worker_id_ref.borrow() != next_worker_id {
                        return;
                    }

                    if *is_playing_ref.borrow() && !*color_enabled_ref.borrow() {
                        sleep_ms(BW_PLAYBACK_BACKGROUND_SLEEP_MS).await;
                        continue;
                    }

                    let next_frame = { queue_for_cache.borrow_mut().pop_front() };
                    let Some(i) = next_frame else {
                        if *loading_phase_ref.borrow() == LoadingPhase::Complete {
                            refresh_for_cache.set((*refresh_for_cache).wrapping_add(1));
                            return;
                        }
                        sleep_ms(BW_PLAYBACK_BACKGROUND_SLEEP_MS).await;
                        continue;
                    };

                    if cache_for_cache.borrow().has(i) {
                        continue;
                    }

                    let offscreen = {
                        let frames = frames_for_cache.borrow();
                        frames
                            .get(i)
                            .and_then(|f| f.cframe.as_ref())
                            .and_then(|cframe| {
                                render_to_offscreen_canvas(cframe, &RenderConfig::new(font_size))
                                    .ok()
                            })
                    };

                    if let Some(canvas) = offscreen {
                        cache_for_cache.borrow_mut().store(i, canvas);
                        if i == *current_index_ref.borrow() {
                            refresh_for_cache.set((*refresh_for_cache).wrapping_add(1));
                        }
                    }

                    yield_to_event_loop().await;
                }
            });
        });
    }

    // Update frame content: draw pre-rendered color canvas when available,
    // otherwise fall back to plain text to keep playback smooth.
    {
        let content_ref = content_ref.clone();
        let canvas_ref = canvas_ref.clone();
        let frames_ref = frames_ref.clone();
        let frame_canvas_cache = frame_canvas_cache.clone();
        let color_enabled = *color_enabled;
        let total_frames = *frame_count;
        let current_frame_idx = (*current_index).min(total_frames.saturating_sub(1));
        let font_size = *calculated_font_size;
        let font_size_key = (*calculated_font_size * 100.0) as i32;
        let cache_refresh_tick = *color_cache_refresh;

        use_effect_with((current_frame_idx, color_enabled, total_frames, font_size_key, cache_refresh_tick), move |_| {
            let frames = frames_ref.borrow();
            if let Some(frame) = frames.get(current_frame_idx) {
                if color_enabled {
                    if let Some(cframe) = frame.cframe.as_ref() {
                        if let Some(canvas) = canvas_ref.cast::<web_sys::HtmlCanvasElement>() {
                            {
                                let mut cache = frame_canvas_cache.borrow_mut();
                                cache.resize(total_frames);
                                cache.invalidate_for_font_size_key(font_size_key);
                            }

                            let drawn = {
                                let cache = frame_canvas_cache.borrow();
                                draw_frame_from_cache(&canvas, &cache, current_frame_idx)
                                    .unwrap_or(false)
                            };
                            if drawn {
                                return;
                            }

                            if let Ok(offscreen) =
                                render_to_offscreen_canvas(cframe, &RenderConfig::new(font_size))
                            {
                                let draw_ok = draw_cached_canvas(&canvas, &offscreen).is_ok();
                                frame_canvas_cache
                                    .borrow_mut()
                                    .store(current_frame_idx, offscreen);
                                if draw_ok {
                                    return;
                                }
                            }
                        }
                    }
                }

                if let Some(element) = content_ref.cast::<web_sys::HtmlElement>() {
                    element.set_text_content(Some(&frame.content));
                }
            }
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
        let frame_count = frame_count.clone();
        let audio_ref = audio_ref.clone();
        let fps = *current_fps;
        Callback::from(move |e: web_sys::InputEvent| {
            if let Some(target) = e.target() {
                if let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() {
                    let slider_val = input.value_as_number();
                    if slider_val.is_finite() {
                        let total_frames = *frame_count;
                        if total_frames > 0 {
                            let target_frame =
                                (slider_val.clamp(0.0, 1.0) * (total_frames - 1) as f64).round()
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
        let frame_count = frame_count.clone();
        let is_playing = is_playing.clone();
        Callback::from(move |_| {
            if *is_playing {
                is_playing.set(false);
            }
            let total_frames = *frame_count;
            if total_frames > 0 {
                let current = *current_index;
                let prev = if current == 0 {
                    total_frames - 1
                } else {
                    current - 1
                };
                current_index.set(prev);
            }
        })
    };

    let on_step_forward = {
        let current_index = current_index.clone();
        let frame_count = frame_count.clone();
        let is_playing = is_playing.clone();
        Callback::from(move |_| {
            if *is_playing {
                is_playing.set(false);
            }
            let total_frames = *frame_count;
            if total_frames > 0 {
                let current = *current_index;
                let next = if current >= total_frames - 1 {
                    0
                } else {
                    current + 1
                };
                current_index.set(next);
            }
        })
    };

    let total_frames = *frame_count;
    let current_frame = (*current_index).min(total_frames.saturating_sub(1));
    let progress = if total_frames > 1 {
        current_frame as f64 / (total_frames - 1) as f64
    } else {
        0.0
    };

    let loading_message = "Loading frames...".to_string();

    // Color loading progress message (read from RefCell - updated by color loading without re-renders)
    let (color_loaded, color_total) = *color_progress.borrow();
    let color_loading_message = if *loading_phase == LoadingPhase::LoadingColors && color_total > 0 {
        let pct = (color_loaded as f32 / color_total as f32 * 100.0) as u8;
        Some(format!("Loading colors: {}%", pct))
    } else {
        None
    };
    let colors_loading = *loading_phase == LoadingPhase::LoadingColors;

    let font_size_style = {
        let font_size = *calculated_font_size;
        let sizing = FontSizing::default();
        let line_height_px = sizing.line_height(font_size);
        let frames = frames_ref.borrow();
        if let Some(frame) = frames.get(current_frame) {
            let (cols, rows) = frame.dimensions();
            let (w, h) = sizing.canvas_dimensions(cols, rows, font_size);
            format!(
                "font-size: {:.2}px; line-height: {:.2}px; width: {:.2}px; height: {:.2}px; padding: 0;",
                font_size, line_height_px, w, h
            )
        } else {
            format!("font-size: {:.2}px; line-height: {:.2}px;", font_size, line_height_px)
        }
    };

    // Lucide icon IDs
    let play_icon = if *is_playing { IconId::LucidePause } else { IconId::LucidePlay };
    let mute_icon_id = if *audio_muted { IconId::LucideVolumeX } else { IconId::LucideVolume2 };
    let overlay_icon_id = if *overlay_hidden { IconId::LucideEyeOff } else { IconId::LucideEye };

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

    let color_available = *has_any_color;

    let has_colors = {
        if !*color_enabled || !color_available {
            false
        } else {
            let frames = frames_ref.borrow();
            frames
                .get(current_frame)
                .map(|f| f.has_color())
                .unwrap_or(false)
        }
    };

    let audio_data_url = (*audio_src).clone().unwrap_or_default();
    let has_audio = audio_src.is_some();

    let viewer_class = if *overlay_hidden { "ascii-frames-viewer fullscreen-mode" } else { "ascii-frames-viewer" };
    let show_controls = !*overlay_hidden || *is_hovering;

    html! {
        <div class={viewer_class} onmouseenter={on_mouse_enter} onmouseleave={on_mouse_leave}>
            if has_audio {
                <audio ref={audio_ref} src={audio_data_url} preload="auto" style="display: none;"></audio>
            }
            <div class="frames-display" ref={container_ref}>
                if *loading_phase == LoadingPhase::LoadingText {
                    <div class="loading-frames">{loading_message}</div>
                } else if let Some(ref error) = *loading_error {
                    <div class="error-frames">{error.clone()}</div>
                } else if total_frames == 0 {
                    <div class="no-frames">{"No frames available"}</div>
                } else {
                    if has_colors {
                        <canvas ref={canvas_ref.clone()} class="ascii-frame-canvas"></canvas>
                    } else {
                        <pre class="ascii-frame-content" style={font_size_style.clone()} ref={content_ref.clone()}></pre>
                    }
                }
            </div>

            if show_controls {
                <div class="controls">
                    // Row 1: Progress bar + Play/Pause button (only for multiple frames)
                    if total_frames > 1 {
                        <div class="control-row">
                            <input id="progress-slider" class="progress" type="range" min="0" max="1" step="0.001" value={progress.to_string()} oninput={on_seek} disabled={total_frames == 0} />
                            <button id="play-pause-btn" class="ctrl-btn play-btn" type="button" onclick={on_toggle_play} disabled={total_frames == 0} title={if *is_playing { "Pause" } else { "Play" }}><Icon icon_id={play_icon} width={"20"} height={"20"} /></button>
                        </div>
                    }

                    // Row 2: Volume slider + Mute button (always visible when frames > 1)
                    if total_frames > 1 {
                        <div class="control-row">
                            <input id="volume-slider" class="progress" type="range" min="0" max="1" step="0.01" value={audio_volume.to_string()} oninput={on_volume_change} />
                            <button id="mute-btn" class={if *audio_muted { "ctrl-btn mute-btn muted" } else { "ctrl-btn mute-btn" }} type="button" onclick={on_toggle_mute} title={if *audio_muted { "Unmute" } else { "Mute" }}><Icon icon_id={mute_icon_id} width={"20"} height={"20"} /></button>
                        </div>
                    }

                    // Row 3: FPS, color, clear, forward/backward buttons
                    <div class="control-row">
                        if total_frames > 1 {
                            <label>{"FPS:"}</label>
                            <input id="fps-input" type="number" class="fps-input" value={current_fps.to_string()} min="1" oninput={on_fps_change} />
                        }
                        <button id="color-btn" class={if *color_enabled && color_available { "ctrl-btn color-btn active" } else if !color_available { "ctrl-btn color-btn disabled" } else { "ctrl-btn color-btn" }} type="button" onclick={on_toggle_color} disabled={!color_available} title={if colors_loading { "Loading colors..." } else if !color_available { "No color data available" } else if *color_enabled { "Color enabled" } else { "Color disabled" }}><Icon icon_id={IconId::LucideBrush} width={"16"} height={"16"} /></button>
                        <button id="hide-overlay-btn" class={if *overlay_hidden { "ctrl-btn active" } else { "ctrl-btn" }} type="button" onclick={on_toggle_overlay} title={if *overlay_hidden { "Show overlay" } else { "Hide overlay" }}><Icon icon_id={overlay_icon_id} width={"20"} height={"20"} /></button>
                        <button id="clear-btn" class="ctrl-btn" type="button" onclick={on_clear_click} title="Clear"><Icon icon_id={IconId::LucideXCircle} width={"20"} height={"20"} /></button>
                        <span class="info-text">{format!("{}/{}", current_frame + 1, total_frames)}</span>
                        if let Some(ref msg) = color_loading_message {
                            <span class="info-text">{msg.clone()}</span>
                        } else if has_colors {
                            <span class="info-text">{"Color"}</span>
                        }
                        if total_frames > 1 {
                            <div style="flex: 1;"></div>
                            <button id="step-backward-btn" class="ctrl-btn" type="button" onclick={on_step_backward} disabled={total_frames == 0} title="Step backward"><span style="display: inline-flex; transform: scaleX(-1);"><Icon icon_id={IconId::LucideSkipForward} width={"20"} height={"20"} /></span></button>
                            <button id="step-forward-btn" class="ctrl-btn" type="button" onclick={on_step_forward} disabled={total_frames == 0} title="Step forward"><Icon icon_id={IconId::LucideSkipForward} width={"20"} height={"20"} /></button>
                        }
                    </div>
                </div>
            }
        </div>
    }
}
