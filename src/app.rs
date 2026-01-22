use crate::ascii_frames_viewer::AsciiFramesViewer;
use wasm_bindgen::prelude::*;
use wasm_bindgen::closure::Closure;
use yew::prelude::*;

#[wasm_bindgen(inline_js = r#"
export function setupDropListener(callback) {
    const tauri = window.__TAURI__;
    if (!tauri || !tauri.event) {
        console.warn('Tauri event API not available');
        return null;
    }

    // Listen for file drop events from Tauri (event name is 'tauri://drop' in Tauri 2.0)
    return tauri.event.listen('tauri://drop', (event) => {
        console.log('Drop event received:', event);
        const paths = event.payload.paths;
        if (paths && paths.length > 0) {
            // Get the first dropped path (folder or file)
            callback(paths[0]);
        }
    });
}

export function setupDragOverListener(enterCallback, leaveCallback) {
    const tauri = window.__TAURI__;
    if (!tauri || !tauri.event) {
        return null;
    }

    const enterUnlisten = tauri.event.listen('tauri://drag-enter', () => {
        enterCallback();
    });

    const leaveUnlisten = tauri.event.listen('tauri://drag-leave', () => {
        leaveCallback();
    });

    return { enterUnlisten, leaveUnlisten };
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = setupDropListener)]
    fn setup_drop_listener(callback: &Closure<dyn Fn(String)>) -> JsValue;

    #[wasm_bindgen(js_name = setupDragOverListener)]
    fn setup_drag_over_listener(
        enter_callback: &Closure<dyn Fn()>,
        leave_callback: &Closure<dyn Fn()>,
    ) -> JsValue;
}

#[function_component(App)]
pub fn app() -> Html {
    let directory_path = use_state(|| String::new());
    let is_drag_over = use_state(|| false);

    // Setup Tauri drag-drop listener
    {
        let directory_path = directory_path.clone();
        let is_drag_over = is_drag_over.clone();

        use_effect_with((), move |_| {
            let directory_path_clone = directory_path.clone();
            let is_drag_over_clone = is_drag_over.clone();
            let is_drag_over_clone2 = is_drag_over.clone();

            // Drop handler
            let drop_closure = Closure::wrap(Box::new(move |path: String| {
                directory_path_clone.set(path);
                is_drag_over_clone.set(false);
            }) as Box<dyn Fn(String)>);

            // Drag enter handler
            let enter_closure = Closure::wrap(Box::new(move || {
                is_drag_over_clone2.set(true);
            }) as Box<dyn Fn()>);

            // Drag leave handler
            let is_drag_over_leave = is_drag_over.clone();
            let leave_closure = Closure::wrap(Box::new(move || {
                is_drag_over_leave.set(false);
            }) as Box<dyn Fn()>);

            let _drop_unlisten = setup_drop_listener(&drop_closure);
            let _drag_unlisten = setup_drag_over_listener(&enter_closure, &leave_closure);

            // Keep closures alive
            drop_closure.forget();
            enter_closure.forget();
            leave_closure.forget();

            || ()
        });
    }

    let on_clear = {
        let directory_path = directory_path.clone();
        Callback::from(move |_| {
            directory_path.set(String::new());
        })
    };

    let drag_over_class = if *is_drag_over { "drag-over" } else { "" };

    html! {
        <main class="container">
            <div class="drop-zone">
                if directory_path.is_empty() {
                    <div class={classes!("drop-zone-hint", drag_over_class)}>
                        <div class="hint-icon">{"+"}</div>
                        <p>{"Drag and drop a folder with frames here"}</p>
                        <p style="font-size: 0.85rem; color: #666;">{"Supports folders with .txt frame files"}</p>
                    </div>
                } else {
                    <div style="position: relative; width: 100%; height: 100%;">
                        <button class="clear-btn" onclick={on_clear}>{"Clear"}</button>
                        <AsciiFramesViewer
                            directory_path={(*directory_path).clone()}
                            fps={24}
                            loop_enabled={true}
                        />
                    </div>
                }
            </div>
        </main>
    }
}
