use crate::ascii_frames_viewer::AsciiFramesViewer;
use wasm_bindgen::prelude::*;
use wasm_bindgen::closure::Closure;
use yew::prelude::*;

#[wasm_bindgen(inline_js = r#"
export async function setupDropListener(callback) {
    // Wait briefly for Tauri to be ready
    let tauri = window.__TAURI__;
    if (!tauri) {
        await new Promise(r => setTimeout(r, 500));
        tauri = window.__TAURI__;
    }
    if (!tauri || !tauri.event) {
        console.error('Tauri event API not available');
        return;
    }

    // Listen for our custom 'file-drop' event emitted from Rust backend
    await tauri.event.listen('file-drop', (event) => {
        callback(event.payload);
    });
}

export async function setupDragOverListener(enterCallback, leaveCallback) {
    let tauri = window.__TAURI__;
    if (!tauri) {
        await new Promise(r => setTimeout(r, 500));
        tauri = window.__TAURI__;
    }
    if (!tauri || !tauri.event) {
        return;
    }

    await tauri.event.listen('tauri://drag-enter', () => {
        enterCallback();
    });

    await tauri.event.listen('tauri://drag-leave', () => {
        leaveCallback();
    });

    await tauri.event.listen('tauri://drop', () => {
        leaveCallback();
    });
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = setupDropListener)]
    fn setup_drop_listener(callback: &Closure<dyn Fn(String)>);

    #[wasm_bindgen(js_name = setupDragOverListener)]
    fn setup_drag_over_listener(
        enter_callback: &Closure<dyn Fn()>,
        leave_callback: &Closure<dyn Fn()>,
    );
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
                    <AsciiFramesViewer
                        directory_path={(*directory_path).clone()}
                        fps={24}
                        loop_enabled={true}
                        on_clear={on_clear}
                    />
                }
            </div>
        </main>
    }
}
