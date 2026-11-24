#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod browser_audio;
mod engine;
mod graph;
mod session;
mod virtual_bus;

use audio::AudioCatalog;
use browser_audio::{BrowserCapture, BrowserPairing, BrowserTab};
use engine::{EngineStatus, Runtime};
use graph::Project;
use session::{Session, SessionStore};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};
use virtual_bus::{VirtualBus, VirtualBuses};

#[derive(Clone, Default)]
struct Routing(Arc<Mutex<Option<Runtime>>>);

#[tauri::command]
async fn audio_catalog() -> Result<AudioCatalog, String> {
    tauri::async_runtime::spawn_blocking(audio::enumerate)
        .await
        .map_err(engine_error)?
}

#[tauri::command]
fn load_session(store: State<'_, Arc<SessionStore>>) -> Result<Option<Session>, String> {
    store.load()
}

#[tauri::command]
fn save_session(store: State<'_, Arc<SessionStore>>, session: Session) -> Result<(), String> {
    store.save(&session)
}

#[tauri::command]
async fn start_routing(
    routing: State<'_, Routing>,
    browser: State<'_, BrowserCapture>,
    project: Project,
) -> Result<(), String> {
    let routing = routing.inner().clone();
    let browser = browser.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut current = routing.0.lock().map_err(engine_error)?;
        if current
            .as_ref()
            .is_some_and(|runtime| runtime.status().running)
        {
            return Err("Le routage est déjà démarré.".into());
        }
        drop(current.take());
        *current = Some(Runtime::start(project, browser)?);
        Ok(())
    })
    .await
    .map_err(engine_error)?
}

#[tauri::command]
async fn update_routing(routing: State<'_, Routing>, project: Project) -> Result<(), String> {
    let routing = routing.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let current = routing.0.lock().map_err(engine_error)?;
        current
            .as_ref()
            .filter(|runtime| runtime.status().running)
            .ok_or_else(|| "Le routage est arrêté.".to_string())?
            .update_mix(&project)
    })
    .await
    .map_err(engine_error)?
}

#[tauri::command]
async fn stop_routing(routing: State<'_, Routing>) -> Result<(), String> {
    let routing = routing.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut current = routing.0.lock().map_err(engine_error)?;
        if let Some(runtime) = current.take() {
            runtime.stop();
        }
        Ok(())
    })
    .await
    .map_err(engine_error)?
}

#[tauri::command]
async fn routing_status(routing: State<'_, Routing>) -> Result<EngineStatus, String> {
    let routing = routing.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let current = routing.0.lock().map_err(engine_error)?;
        Ok(current.as_ref().map(Runtime::status).unwrap_or_default())
    })
    .await
    .map_err(engine_error)?
}

#[tauri::command]
async fn browser_capture_pairing(
    browser: State<'_, BrowserCapture>,
) -> Result<BrowserPairing, String> {
    browser.pairing()
}

#[tauri::command]
fn browser_capture_tabs(browser: State<'_, BrowserCapture>) -> Vec<BrowserTab> {
    browser.tabs()
}

#[tauri::command]
fn browser_capture_extension_path(app: tauri::AppHandle) -> Result<String, String> {
    let bundled = app
        .path()
        .resolve(
            "browser-extension/chromium",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|error| format!("Extension Patchbay introuvable ({error})."))?;
    if bundled.join("manifest.json").is_file() {
        return Ok(bundled.to_string_lossy().into_owned());
    }
    #[cfg(debug_assertions)]
    {
        let project =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../browser-extension/chromium");
        if let Ok(project) = project.canonicalize()
            && project.join("manifest.json").is_file()
        {
            return Ok(project.to_string_lossy().into_owned());
        }
    }
    Err("L'extension Chromium est absente de ce bundle Patchbay.".into())
}

#[tauri::command]
async fn virtual_buses() -> Result<VirtualBuses, String> {
    tauri::async_runtime::spawn_blocking(virtual_bus::list)
        .await
        .map_err(engine_error)?
}

#[tauri::command]
async fn restart_core_audio(routing: State<'_, Routing>) -> Result<Option<VirtualBuses>, String> {
    let routing = routing.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        while_routing_stopped(&routing, virtual_bus::restart_core_audio)
    })
    .await
    .map_err(engine_error)?
}

#[tauri::command]
async fn create_virtual_bus(
    routing: State<'_, Routing>,
    name: String,
    input_channels: usize,
    output_channels: usize,
) -> Result<VirtualBus, String> {
    let routing = routing.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        while_routing_stopped(&routing, || {
            virtual_bus::create(&name, input_channels, output_channels)
        })
    })
    .await
    .map_err(engine_error)?
}

#[tauri::command]
async fn delete_virtual_bus(
    routing: State<'_, Routing>,
    store: State<'_, Arc<SessionStore>>,
    id: String,
) -> Result<(), String> {
    let routing = routing.inner().clone();
    let store = Arc::clone(store.inner());
    tauri::async_runtime::spawn_blocking(move || {
        while_routing_stopped(&routing, || {
            store.while_device_unreferenced(&id, || virtual_bus::delete(&id))
        })
    })
    .await
    .map_err(engine_error)?
}

#[tauri::command]
fn virtual_driver_path(app: tauri::AppHandle) -> Result<String, String> {
    let bundled = app
        .path()
        .resolve(
            "driver/Patchbay.driver",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|error| format!("Pilote audio Patchbay introuvable ({error})."))?;
    if bundled.join("Contents/Info.plist").is_file() {
        return Ok(bundled.to_string_lossy().into_owned());
    }
    Err("Le pilote audio Patchbay est absent de ce bundle.".into())
}

fn while_routing_stopped<T>(
    routing: &Routing,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut current = routing.0.lock().map_err(engine_error)?;
    if current
        .as_ref()
        .is_some_and(|runtime| runtime.status().running)
    {
        return Err("Arrête le routage avant d'intervenir sur les interfaces virtuelles.".into());
    }
    drop(current.take());
    action()
}

fn engine_error(error: impl std::fmt::Display) -> String {
    format!("Moteur audio indisponible ({error}).")
}

fn main() {
    tauri::Builder::default()
        .manage(Routing::default())
        .manage(BrowserCapture::default())
        .setup(|app| {
            let file = app.path().app_data_dir()?.join("session.json");
            app.manage(Arc::new(SessionStore::new(file)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            audio_catalog,
            load_session,
            save_session,
            start_routing,
            update_routing,
            stop_routing,
            routing_status,
            browser_capture_pairing,
            browser_capture_tabs,
            browser_capture_extension_path,
            virtual_buses,
            restart_core_audio,
            create_virtual_bus,
            delete_virtual_bus,
            virtual_driver_path
        ])
        .run(tauri::generate_context!())
        .expect("Impossible de démarrer Patchbay");
}
