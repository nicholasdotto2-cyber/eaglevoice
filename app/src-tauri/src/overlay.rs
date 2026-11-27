use crate::input;
use crate::settings;
use crate::settings::{OverlayPosition, OverlayStyle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use once_cell::sync::Lazy;
use serde::Serialize;
use specta::Type;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize};

#[cfg(not(target_os = "macos"))]
use log::debug;

#[cfg(not(target_os = "macos"))]
use tauri::WebviewWindowBuilder;

#[cfg(target_os = "macos")]
use tauri::WebviewUrl;

#[cfg(target_os = "macos")]
use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};

#[cfg(target_os = "linux")]
use crate::utils;

#[cfg(target_os = "linux")]
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(RecordingOverlayPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

// Native overlay window sizes (logical points). One window is reused for every
// state and resized in `show_overlay_state`; each size need only be at least as
// large as the card it hosts (the `--ov-*` vars in RecordingOverlay.css). The
// card is CSS-anchored flush to the screen edge, so window height doesn't move
// where the card sits — only OVERLAY_TOP_OFFSET / OVERLAY_BOTTOM_OFFSET do. Keep
// these in sync with the CSS card geometry.
//
// On Windows these sizes are additionally multiplied by the accessibility text
// scale (see windows_text_scale_factor), which WebView2 applies as a zoom.
//
// Compact overlay (Minimal / transcribing / processing): the 40h pill animates
// width from 172 (--ov-rest-w) to 216 (--ov-work-w) and expands from center, so
// the window must fit the widest state plus a little slack.
// eagleVoice: bolla di vetro. A riposo una sfera piccola,
// in dettatura una pillola, con un avviso un riquadro di vetro accanto alla bolla.
const OVERLAY_WIDTH: f64 = 300.0;
const OVERLAY_HEIGHT: f64 = 72.0;
const OVERLAY_IDLE_WIDTH: f64 = 52.0;
const OVERLAY_IDLE_HEIGHT: f64 = 52.0;
const OVERLAY_NOTICE_WIDTH: f64 = 360.0;
const OVERLAY_NOTICE_HEIGHT: f64 = 190.0;

/// Ultimo stato mostrato dalla barra: serve per rimetterla a posto dopo un trascinamento.
static CURRENT_STATE: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new("idle".to_string()));
/// L'interfaccia mostra un avviso: la finestra resta a misura di avviso in ogni stato.
static NOTICE_OPEN: AtomicBool = AtomicBool::new(false);
/// Un trascinamento o un clic sulla bolla è in corso (niente doppioni).
static PRESS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Bordo e punto della bolla, per l'interfaccia (evento `flow:dock`).
#[derive(Clone, Debug, Serialize, Type)]
pub struct FlowDockEvent {
    pub edge: OverlayPosition,
    pub offset: f64,
}
pub const EVENT_DOCK: &str = "flow:dock";

pub fn dock_info(app_handle: &AppHandle) -> FlowDockEvent {
    let s = settings::get_settings(app_handle);
    FlowDockEvent { edge: s.overlay_position, offset: s.overlay_dock_offset.clamp(0.0, 1.0) }
}

fn emit_dock(app_handle: &AppHandle) {
    let _ = app_handle.emit(EVENT_DOCK, dock_info(app_handle));
}

pub fn set_notice_open(open: bool) {
    NOTICE_OPEN.store(open, Ordering::SeqCst);
}

/// Origine della finestra (x, y) per una bolla agganciata a `edge` nel punto `offset`
/// del bordo. Unità libere (fisiche o logiche), purché tutte uguali. La finestra resta
/// dentro il monitor; con `offset` 0,5 la barra è centrata come in Handy.
#[allow(clippy::too_many_arguments)]
fn dock_origin(
    monitor: (f64, f64, f64, f64),
    size: (f64, f64),
    edge: OverlayPosition,
    offset: f64,
    side: f64,
    top: f64,
    bottom: f64,
) -> (f64, f64) {
    let (mx, my, mw, mh) = monitor;
    let (w, h) = size;
    let t = offset.clamp(0.0, 1.0);
    let clamp = |v: f64, lo: f64, hi: f64| if hi < lo { lo } else { v.clamp(lo, hi) };
    match edge {
        OverlayPosition::Top | OverlayPosition::Bottom => {
            let x = clamp(mx + mw * t - w / 2.0, mx + side, mx + mw - w - side);
            let y = if edge == OverlayPosition::Top { my + top } else { my + mh - h - bottom };
            (x, y)
        }
        OverlayPosition::Left | OverlayPosition::Right => {
            let x = if edge == OverlayPosition::Left { mx + side } else { mx + mw - w - side };
            let y = clamp(my + mh * t - h / 2.0, my + top, my + mh - h - bottom);
            (x, y)
        }
    }
}

/// Bordo più vicino al punto `p` del monitor e posizione lungo quel bordo (0..1).
fn nearest_dock(monitor: (f64, f64, f64, f64), p: (f64, f64)) -> (OverlayPosition, f64) {
    let (mx, my, mw, mh) = monitor;
    let (px, py) = p;
    let d = [
        (OverlayPosition::Left, px - mx),
        (OverlayPosition::Right, mx + mw - px),
        (OverlayPosition::Top, py - my),
        (OverlayPosition::Bottom, my + mh - py),
    ];
    let (edge, _) = d
        .iter()
        .copied()
        .fold(d[0], |best, cur| if cur.1 < best.1 { cur } else { best });
    let offset = match edge {
        OverlayPosition::Top | OverlayPosition::Bottom => (px - mx) / mw,
        OverlayPosition::Left | OverlayPosition::Right => (py - my) / mh,
    };
    (edge, offset.clamp(0.0, 1.0))
}

// Actual is 394x118, just a little extra
const OVERLAY_STREAM_WIDTH: f64 = 400.0;
const OVERLAY_STREAM_HEIGHT: f64 = 120.0;

/// Overlay window size (logical) for a given UI state.
fn overlay_dimensions(state: &str) -> (f64, f64) {
    // eagleVoice: a riposo la finestra è piccola, così non copre i clic
    // sullo schermo sotto di lei (la pillola a riposo sta in 120x36).
    if state == "notice" || NOTICE_OPEN.load(Ordering::SeqCst) {
        return (OVERLAY_NOTICE_WIDTH, OVERLAY_NOTICE_HEIGHT);
    }
    if state == "idle" {
        return (OVERLAY_IDLE_WIDTH, OVERLAY_IDLE_HEIGHT);
    }
    if state == "streaming" {
        (OVERLAY_STREAM_WIDTH, OVERLAY_STREAM_HEIGHT)
    } else {
        (OVERLAY_WIDTH, OVERLAY_HEIGHT)
    }
}

static LAST_MIC_LEVEL_EMIT: AtomicU64 = AtomicU64::new(0);
const EMIT_THROTTLE_MS: u64 = 33; // ~30 FPS

#[cfg(target_os = "macos")]
const OVERLAY_TOP_OFFSET: f64 = 46.0;
#[cfg(any(target_os = "windows", target_os = "linux"))]
const OVERLAY_TOP_OFFSET: f64 = 4.0;

#[cfg(target_os = "macos")]
const OVERLAY_BOTTOM_OFFSET: f64 = 15.0;

#[cfg(target_os = "linux")]
const OVERLAY_BOTTOM_OFFSET: f64 = 40.0;

// eagleVoice: su Windows la barra si misura sull'area di lavoro, che esclude
// già la barra delle applicazioni (a qualunque altezza): basta un margine piccolo.
#[cfg(target_os = "windows")]
const OVERLAY_BOTTOM_OFFSET: f64 = 8.0;

/// Distanza dal bordo per le posizioni `Left` e `Right`.
const OVERLAY_SIDE_OFFSET: f64 = 4.0;

/// Configures the edge and offset of a GTK layer surface. gtk-layer-shell
/// commits anchor and margin changes itself, including while the surface is
/// mapped, so changing position does not require a manual hide/show cycle.
#[cfg(target_os = "linux")]
fn configure_layer_shell_position(gtk_window: &gtk::ApplicationWindow, position: OverlayPosition) {
    let (edge, opposite_edge, margin) = match position {
        OverlayPosition::Top => (Edge::Top, Edge::Bottom, OVERLAY_TOP_OFFSET),
        OverlayPosition::Bottom => (Edge::Bottom, Edge::Top, OVERLAY_BOTTOM_OFFSET),
        OverlayPosition::Left => (Edge::Left, Edge::Right, OVERLAY_SIDE_OFFSET),
        OverlayPosition::Right => (Edge::Right, Edge::Left, OVERLAY_SIDE_OFFSET),
    };

    gtk_window.set_anchor(edge, true);
    gtk_window.set_anchor(opposite_edge, false);
    gtk_window.set_layer_shell_margin(edge, margin.round() as i32);
    gtk_window.set_layer_shell_margin(opposite_edge, 0);
}

/// Configures a GTK layer surface before it is shown.
///
/// Tauri's normal `set_size` path calls `gtk_window_resize`, but layer surfaces
/// derive their dimensions from GTK's size request. gtk-layer-shell documents
/// the `set_size_request` + `resize(1, 1)` sequence for forcing a new size.
#[cfg(target_os = "linux")]
fn configure_layer_shell_surface(
    gtk_window: &gtk::ApplicationWindow,
    position: OverlayPosition,
    width: f64,
    height: f64,
) {
    use gtk::prelude::{GtkWindowExt, WidgetExt};

    configure_layer_shell_position(gtk_window, position);

    gtk_window.set_size_request(
        width.round().max(1.0) as i32,
        height.round().max(1.0) as i32,
    );
    gtk_window.resize(1, 1);
}

/// Initializes GTK layer shell for Linux overlay window
/// Returns true if layer shell was successfully initialized, false otherwise
#[cfg(target_os = "linux")]
fn init_gtk_layer_shell(overlay_window: &tauri::webview::WebviewWindow) -> bool {
    if utils::env_flag_enabled("HANDY_NO_GTK_LAYER_SHELL") {
        debug!("Skipping GTK layer shell init (HANDY_NO_GTK_LAYER_SHELL is enabled)");
        return false;
    }

    if !gtk_layer_shell::is_supported() {
        return false;
    }

    // Try to get the GTK window from the Tauri webview
    if let Ok(gtk_window) = overlay_window.gtk_window() {
        gtk_window.init_layer_shell();
        gtk_window.set_layer(Layer::Overlay);
        gtk_window.set_keyboard_mode(KeyboardMode::None);
        gtk_window.set_exclusive_zone(0);

        let overlay_position = settings::get_settings(overlay_window.app_handle()).overlay_position;
        configure_layer_shell_surface(&gtk_window, overlay_position, OVERLAY_WIDTH, OVERLAY_HEIGHT);

        let initialized = gtk_window.is_layer_window();
        LAYER_SHELL_ACTIVE.store(initialized, Ordering::SeqCst);
        return initialized;
    }
    false
}

/// Forces a window to be topmost using Win32 API (Windows only)
/// This is more reliable than Tauri's set_always_on_top which can be overridden
#[cfg(target_os = "windows")]
fn force_overlay_topmost(overlay_window: &tauri::webview::WebviewWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    // Clone because run_on_main_thread takes 'static
    let overlay_clone = overlay_window.clone();

    // Make sure the Win32 call happens on the UI thread
    let _ = overlay_clone.clone().run_on_main_thread(move || {
        if let Ok(hwnd) = overlay_clone.hwnd() {
            unsafe {
                // Force Z-order: make this window topmost without changing size/pos or stealing focus
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    });
}

fn get_monitor_with_cursor(app_handle: &AppHandle) -> Option<tauri::Monitor> {
    if let Some(mouse_location) = input::get_cursor_position(app_handle) {
        if let Ok(monitors) = app_handle.available_monitors() {
            for monitor in monitors {
                // On Windows both the cursor (enigo -> GetCursorPos) and the
                // monitor bounds are physical pixels, so compare them directly.
                #[cfg(target_os = "windows")]
                if is_mouse_within_monitor(mouse_location, monitor.position(), monitor.size()) {
                    return Some(monitor);
                }

                // macOS/Linux: enigo returns logical coords, so scale the bounds down.
                #[cfg(not(target_os = "windows"))]
                {
                    let scale = monitor.scale_factor();
                    let pos = PhysicalPosition::new(
                        (monitor.position().x as f64 / scale) as i32,
                        (monitor.position().y as f64 / scale) as i32,
                    );
                    let size = PhysicalSize::new(
                        (monitor.size().width as f64 / scale) as u32,
                        (monitor.size().height as f64 / scale) as u32,
                    );
                    if is_mouse_within_monitor(mouse_location, &pos, &size) {
                        return Some(monitor);
                    }
                }
            }
        }
    }

    app_handle.primary_monitor().ok().flatten()
}

fn is_mouse_within_monitor(
    mouse_pos: (i32, i32),
    monitor_pos: &PhysicalPosition<i32>,
    monitor_size: &PhysicalSize<u32>,
) -> bool {
    let (mouse_x, mouse_y) = mouse_pos;
    let PhysicalPosition {
        x: monitor_x,
        y: monitor_y,
    } = *monitor_pos;
    let PhysicalSize {
        width: monitor_width,
        height: monitor_height,
    } = *monitor_size;

    mouse_x >= monitor_x
        && mouse_x < (monitor_x + monitor_width as i32)
        && mouse_y >= monitor_y
        && mouse_y < (monitor_y + monitor_height as i32)
}

/// Returns overlay position in logical coordinates (points on macOS).
///
/// The Bottom anchor uses the macOS work area (visibleFrame) so the overlay
/// tracks the Dock — above it when shown, at the screen edge when hidden.
/// This relies on tauri 2.11's work_area.position.y fix (#14655), the same
/// bug that led PR #969 to abandon work_area for full monitor bounds. Top and
/// the other platforms keep full monitor bounds plus the fixed offsets
/// (work_area is unreliable on Wayland; Windows' offset clears the taskbar).
///
/// We must use LogicalPosition (not PhysicalPosition) because Tauri/tao
/// converts PhysicalPosition using the scale factor of the monitor the window
/// is *currently* on, which is wrong when moving cross-monitor. Windows uses
/// `place_windows_overlay` instead (no single logical space across mixed DPI).
fn calculate_overlay_position(
    app_handle: &AppHandle,
    width: f64,
    height: f64,
) -> Option<(f64, f64)> {
    let monitor = get_monitor_with_cursor(app_handle)?;
    let scale = monitor.scale_factor();
    let monitor_x = monitor.position().x as f64 / scale;
    let monitor_y = monitor.position().y as f64 / scale;
    let monitor_width = monitor.size().width as f64 / scale;

    let settings = settings::get_settings(app_handle);

    let monitor_height = monitor.size().height as f64 / scale;
    // eagleVoice: bolla agganciata a un bordo nel punto scelto col trascinamento.
    Some(dock_origin(
        (monitor_x, monitor_y, monitor_width, monitor_height),
        (width, height),
        settings.overlay_position,
        settings.overlay_dock_offset,
        OVERLAY_SIDE_OFFSET,
        OVERLAY_TOP_OFFSET,
        OVERLAY_BOTTOM_OFFSET,
    ))
}

/// Current overlay window size in logical units (points), for repositioning
/// without assuming a fixed size (compact vs. streaming).
#[cfg(not(target_os = "windows"))]
fn current_overlay_logical_size(window: &tauri::webview::WebviewWindow) -> Option<(f64, f64)> {
    let size = window.inner_size().ok()?;
    let scale = window.scale_factor().ok()?;
    Some((size.width as f64 / scale, size.height as f64 / scale))
}

#[cfg(target_os = "windows")]
static WINDOWS_OVERLAY_IS_STREAMING: AtomicBool = AtomicBool::new(false);

/// Windows accessibility text size (Settings > Accessibility > Text size), a
/// separate axis from display scaling that WebView2 applies as a document zoom.
#[cfg(target_os = "windows")]
fn windows_text_scale_factor() -> f64 {
    // Absent until the user moves the slider off 100%; stored as a percentage.
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Accessibility")
        .and_then(|key| key.get_value::<u32, _>("TextScaleFactor"))
        .map(|percent| (percent as f64 / 100.0).clamp(1.0, 2.25))
        .unwrap_or(1.0)
}

/// Overlay rectangle in the destination monitor's physical pixels, so nothing
/// is converted through the window's previous-monitor DPI.
#[cfg(target_os = "windows")]
fn windows_overlay_bounds(
    monitor_position: PhysicalPosition<i32>,
    monitor_size: PhysicalSize<u32>,
    scale: f64,
    text_scale: f64,
    logical_width: f64,
    logical_height: f64,
    overlay_position: OverlayPosition,
    offset: f64,
) -> (i32, i32, i32, i32) {
    // Grow the window with the text scale; offsets stay DPI-only since the
    // card sits flush against the window's screen-edge side.
    let content_scale = scale * text_scale;
    let width = (logical_width * content_scale).round().max(1.0) as i32;
    let height = (logical_height * content_scale).round().max(1.0) as i32;
    let (x, y) = dock_origin(
        (
            monitor_position.x as f64,
            monitor_position.y as f64,
            monitor_size.width as f64,
            monitor_size.height as f64,
        ),
        (width as f64, height as f64),
        overlay_position,
        offset,
        OVERLAY_SIDE_OFFSET * scale,
        OVERLAY_TOP_OFFSET * scale,
        OVERLAY_BOTTOM_OFFSET * scale,
    );

    (x.round() as i32, y.round() as i32, width, height)
}

/// Moves and sizes the overlay in one native SetWindowPos, bypassing tao's
/// current-DPI logical conversion that mislands cross-monitor moves.
#[cfg(target_os = "windows")]
fn place_windows_overlay(
    app_handle: &AppHandle,
    overlay_window: &tauri::webview::WebviewWindow,
    logical_width: f64,
    logical_height: f64,
) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

    let monitor = get_monitor_with_cursor(app_handle)
        .ok_or_else(|| "failed to determine the monitor containing the cursor".to_string())?;
    let text_scale = windows_text_scale_factor();
    // Area di lavoro: il monitor senza la barra delle applicazioni.
    let work_area = monitor.work_area();
    let (x, y, width, height) = windows_overlay_bounds(
        work_area.position,
        work_area.size,
        monitor.scale_factor(),
        text_scale,
        logical_width,
        logical_height,
        settings::get_settings(app_handle).overlay_position,
        settings::get_settings(app_handle).overlay_dock_offset,
    );
    let hwnd = overlay_window
        .hwnd()
        .map_err(|error| format!("failed to get overlay window handle: {error}"))?;

    unsafe {
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
        .map_err(|error| format!("failed to set overlay bounds: {error}"))?;
    }

    log::debug!(
        "windows overlay bounds: x={} y={} width={} height={} scale={} text_scale={}",
        x,
        y,
        width,
        height,
        monitor.scale_factor(),
        text_scale
    );
    Ok(())
}

/// Creates the recording overlay window and keeps it hidden by default
#[cfg(not(target_os = "macos"))]
pub fn create_recording_overlay(app_handle: &AppHandle) {
    // On Linux (Wayland), monitor detection often fails, but we don't need exact coordinates
    // for Layer Shell as we use anchors. On other platforms, we require a monitor.
    #[cfg(not(target_os = "linux"))]
    {
        let position = calculate_overlay_position(app_handle, OVERLAY_WIDTH, OVERLAY_HEIGHT);
        if position.is_none() {
            debug!("Failed to determine overlay position, not creating overlay window");
            return;
        }
    }

    // Position starts unset — update_overlay_position() sets the correct
    // LogicalPosition before the overlay is shown.
    let mut builder = WebviewWindowBuilder::new(
        app_handle,
        "recording_overlay",
        tauri::WebviewUrl::App("src/overlay/index.html".into()),
    )
    .title("Recording")
    .resizable(false)
    .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
    .shadow(false)
    .maximizable(false)
    .minimizable(false)
    .closable(false)
    .accept_first_mouse(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .focusable(false)
    .focused(false)
    .visible(false);

    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }

    #[allow(unused_variables)]
    match builder.build() {
        Ok(window) => {
            #[cfg(target_os = "linux")]
            {
                // Try to initialize GTK layer shell, ignore errors if compositor doesn't support it
                if init_gtk_layer_shell(&window) {
                    debug!("GTK layer shell initialized for overlay window");
                } else {
                    debug!("GTK layer shell not available, falling back to regular window");
                }
            }

            debug!("Recording overlay window created successfully (hidden)");
        }
        Err(e) => {
            debug!("Failed to create recording overlay window: {}", e);
        }
    }
}

/// Creates the recording overlay panel and keeps it hidden by default (macOS)
#[cfg(target_os = "macos")]
pub fn create_recording_overlay(app_handle: &AppHandle) {
    if let Some((x, y)) = calculate_overlay_position(app_handle, OVERLAY_WIDTH, OVERLAY_HEIGHT) {
        // PanelBuilder creates a Tauri window then converts it to NSPanel.
        // The window remains registered, so get_webview_window() still works.
        match PanelBuilder::<_, RecordingOverlayPanel>::new(app_handle, "recording_overlay")
            .url(WebviewUrl::App("src/overlay/index.html".into()))
            .title("Recording")
            .position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
            .level(PanelLevel::Status)
            .size(tauri::Size::Logical(tauri::LogicalSize {
                width: OVERLAY_WIDTH,
                height: OVERLAY_HEIGHT,
            }))
            .has_shadow(false)
            .transparent(true)
            .no_activate(true)
            .corner_radius(0.0)
            .style_mask(StyleMask::empty().borderless().nonactivating_panel())
            .with_window(|w| w.decorations(false).transparent(true).focusable(false))
            .collection_behavior(
                CollectionBehavior::new()
                    .can_join_all_spaces()
                    .full_screen_auxiliary(),
            )
            .build()
        {
            Ok(panel) => {
                panel.hide();
            }
            Err(e) => {
                log::error!("Failed to create recording overlay panel: {}", e);
            }
        }
    }
}

fn show_overlay_state(app_handle: &AppHandle, state: &str) {
    // Whether the overlay shows at all is governed by overlay_style; position
    // only chooses Top vs Bottom placement. Checked here (off the main thread)
    // so the common overlay-disabled case never pays for a main-thread hop.
    let settings = settings::get_settings(app_handle);
    if settings.overlay_style == OverlayStyle::None {
        return;
    }

    // The rest queries monitors and the cursor and mutates window geometry. On
    // Linux the monitor/cursor lookups hit GDK/Xlib on the process's shared X11
    // connection, which is only safe from the GTK main thread — running them on
    // a background thread corrupts the connection and hard-crashes the app
    // (issue #227). Hop to the main thread on every platform to keep the
    // geometry path uniform (a no-op cost on Windows, and it also keeps macOS's
    // NSScreen access main-thread-correct). run_on_main_thread runs the closure
    // inline when already on the main thread, so this never deadlocks.
    let handle = app_handle.clone();
    let state = state.to_string();
    let _ = app_handle.run_on_main_thread(move || show_overlay_state_on_main(&handle, &state));
}

fn show_overlay_state_on_main(app_handle: &AppHandle, state: &str) {
    // Size the overlay for this state (compact vs. streaming), then position it.
    if state == "idle" || state == "recording" {
        set_notice_open(false);
    }
    *CURRENT_STATE.lock().unwrap() = state.to_string();
    let (width, height) = overlay_dimensions(state);
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        // Invalidate any delayed hide still in flight from a previous session
        // (see `hide_recording_overlay`).
        OVERLAY_SHOW_GENERATION.fetch_add(1, Ordering::SeqCst);

        #[cfg(target_os = "linux")]
        let shown_with_layer_shell = if LAYER_SHELL_ACTIVE.load(Ordering::SeqCst) {
            let position = settings::get_settings(app_handle).overlay_position;
            match overlay_window.gtk_window() {
                Ok(gtk_window) => {
                    configure_layer_shell_surface(&gtk_window, position, width, height)
                }
                Err(error) => log::error!("Failed to access GTK overlay window: {error}"),
            }
            let _ = overlay_window.show();
            true
        } else {
            false
        };
        #[cfg(not(target_os = "linux"))]
        let shown_with_layer_shell = false;

        if !shown_with_layer_shell {
            let size_started = std::time::Instant::now();
            #[cfg(not(target_os = "windows"))]
            let _ =
                overlay_window.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
            #[cfg(target_os = "windows")]
            WINDOWS_OVERLAY_IS_STREAMING.store(state == "streaming", Ordering::Relaxed);
            let size_elapsed = size_started.elapsed();

            let pos_started = std::time::Instant::now();
            #[cfg(not(target_os = "windows"))]
            let set_pos_elapsed =
                if let Some((x, y)) = calculate_overlay_position(app_handle, width, height) {
                    let set_pos_started = std::time::Instant::now();
                    let _ = overlay_window
                        .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
                    set_pos_started.elapsed()
                } else {
                    std::time::Duration::ZERO
                };
            #[cfg(target_os = "windows")]
            let set_pos_elapsed = {
                let set_pos_started = std::time::Instant::now();
                if let Err(error) =
                    place_windows_overlay(app_handle, &overlay_window, width, height)
                {
                    log::error!("Failed to place recording overlay: {error}");
                }
                set_pos_started.elapsed()
            };
            let pos_calc_elapsed = pos_started.elapsed() - set_pos_elapsed;

            let show_started = std::time::Instant::now();
            let _ = overlay_window.show();
            let show_elapsed = show_started.elapsed();

            // On Windows, aggressively re-assert "topmost" in the native Z-order after showing
            #[cfg(target_os = "windows")]
            force_overlay_topmost(&overlay_window);

            // Re-assert bounds after show(): the pre-show move crosses the DPI
            // boundary, and tao's WM_DPICHANGED reflow clobbers the first placement.
            #[cfg(target_os = "windows")]
            if let Err(error) = place_windows_overlay(app_handle, &overlay_window, width, height) {
                log::error!("Failed to re-assert recording overlay position: {error}");
            }

            log::debug!(
                "overlay '{}': set_size={:?} pos_calc={:?} set_pos={:?} show={:?}",
                state,
                size_elapsed,
                pos_calc_elapsed,
                set_pos_elapsed,
                show_elapsed
            );
        }

        let _ = overlay_window.emit("show-overlay", state);
    }
}

/// Notify the visible recording overlay that the input stream has delivered its
/// first sample chunk. Audio feedback uses the same backend readiness signal,
/// but this targeted event is skipped when overlays are disabled.
pub fn emit_recording_ready(app_handle: &AppHandle) {
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // Showing the overlay is also queued onto the main thread. Queue readiness
    // there as well so a very fast always-on stream cannot overtake show-overlay
    // and then get reset back to the arming state by the frontend.
    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        let _ = handle.emit_to("recording_overlay", "recording-ready", ());
    });
}

/// eagleVoice: barra di dettatura a misura piena per una notifica a riposo
/// (incolla fallito, microfono, motore): resta finché l'interfaccia la chiude.
pub fn show_notice_overlay(app_handle: &AppHandle) {
    set_notice_open(true);
    show_overlay_state(app_handle, "notice");
}

/// eagleVoice: barra di dettatura a riposo (sempre visibile fra le dettature).
pub fn show_idle_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "idle");
}

/// Shows the recording overlay window with fade-in animation
pub fn show_recording_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "recording");
}

/// Shows the larger streaming overlay that displays live transcription text
pub fn show_streaming_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "streaming");
}

/// Shows the transcribing overlay window
pub fn show_transcribing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "transcribing");
}

/// Shows the processing overlay window
pub fn show_processing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "processing");
}

/// Updates the overlay window position based on current settings
pub fn update_overlay_position(app_handle: &AppHandle) {
    // Positioning queries monitors/cursor (GDK/Xlib on Linux) and moves the
    // window, so it must run on the main thread — see show_overlay_state.
    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || update_overlay_position_on_main(&handle));
}

fn update_overlay_position_on_main(app_handle: &AppHandle) {
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        #[cfg(target_os = "linux")]
        if LAYER_SHELL_ACTIVE.load(Ordering::SeqCst) {
            let position = settings::get_settings(app_handle).overlay_position;
            match overlay_window.gtk_window() {
                Ok(gtk_window) => configure_layer_shell_position(&gtk_window, position),
                Err(error) => log::error!("Failed to access GTK overlay window: {error}"),
            }
            return;
        }

        #[cfg(target_os = "windows")]
        {
            let state = CURRENT_STATE.lock().unwrap().clone();
            let (width, height) = overlay_dimensions(&state);
            if let Err(error) = place_windows_overlay(app_handle, &overlay_window, width, height) {
                log::error!("Failed to update recording overlay position: {error}");
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            // Use the window's current size so centering stays correct whether the
            // overlay is in compact or streaming layout.
            let (width, height) = current_overlay_logical_size(&overlay_window)
                .unwrap_or((OVERLAY_WIDTH, OVERLAY_HEIGHT));
            if let Some((x, y)) = calculate_overlay_position(app_handle, width, height) {
                let _ = overlay_window
                    .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
            }
        }
    }
}

/// eagleVoice: rimette misura e posto della barra per lo stato corrente
/// (dopo un trascinamento o quando un avviso si apre o si chiude), senza cambiare stato.
pub fn refresh_overlay_geometry(app_handle: &AppHandle) {
    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        let state = CURRENT_STATE.lock().unwrap().clone();
        let (width, height) = overlay_dimensions(&state);
        if let Some(overlay_window) = handle.get_webview_window("recording_overlay") {
            #[cfg(target_os = "windows")]
            if let Err(error) = place_windows_overlay(&handle, &overlay_window, width, height) {
                log::error!("Failed to refresh recording overlay: {error}");
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = overlay_window
                    .set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
                if let Some((x, y)) = calculate_overlay_position(&handle, width, height) {
                    let _ = overlay_window
                        .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
                }
            }
        }
        emit_dock(&handle);
    });
}

/// eagleVoice: pressione sulla bolla. Segue il mouse finché il tasto resta
/// premuto: se si sposta di più di 4 pixel è un trascinamento, al rilascio la bolla si
/// aggancia al bordo più vicino al cursore e il punto si salva. Torna `true` se c'è
/// stato un trascinamento, `false` se è un clic.
#[cfg(target_os = "windows")]
pub fn track_press(app_handle: &AppHandle) -> bool {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetSystemMetrics, GetWindowRect, SetWindowPos, SM_SWAPBUTTON,
        SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    if PRESS_ACTIVE.swap(true, Ordering::SeqCst) {
        return true;
    }
    let dragged = (|| {
        let window = app_handle.get_webview_window("recording_overlay")?;
        let hwnd = window.hwnd().ok()?;
        let mut start = POINT::default();
        let mut rect = RECT::default();
        unsafe {
            GetCursorPos(&mut start).ok()?;
            GetWindowRect(hwnd, &mut rect).ok()?;
        }
        let button = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 { VK_RBUTTON } else { VK_LBUTTON };
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mut dragged = false;
        let mut last = start;
        loop {
            let down = (unsafe { GetAsyncKeyState(button.0 as i32) } as u16) & 0x8000 != 0;
            let mut p = POINT::default();
            if unsafe { GetCursorPos(&mut p) }.is_ok() {
                last = p;
            }
            let (dx, dy) = (last.x - start.x, last.y - start.y);
            if !dragged && (dx.abs() > 4 || dy.abs() > 4) {
                dragged = true;
            }
            if dragged {
                unsafe {
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        rect.left + dx,
                        rect.top + dy,
                        0,
                        0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            if !down || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        if dragged {
            dock_at_physical_point(app_handle, (last.x as f64, last.y as f64));
        }
        Some(dragged)
    })()
    .unwrap_or(false);
    PRESS_ACTIVE.store(false, Ordering::SeqCst);
    dragged
}

#[cfg(not(target_os = "windows"))]
pub fn track_press(_app_handle: &AppHandle) -> bool {
    false
}

/// Aggancia la bolla al bordo del monitor più vicino al punto (pixel fisici) e salva.
#[cfg(target_os = "windows")]
fn dock_at_physical_point(app_handle: &AppHandle, p: (f64, f64)) {
    let monitors = app_handle.available_monitors().unwrap_or_default();
    let monitor = monitors.iter().find(|m| {
        let (x, y) = (m.position().x as f64, m.position().y as f64);
        let (w, h) = (m.size().width as f64, m.size().height as f64);
        p.0 >= x && p.0 < x + w && p.1 >= y && p.1 < y + h
    });
    let Some(m) = monitor.or(monitors.first()) else { return };
    let rect = (
        m.position().x as f64,
        m.position().y as f64,
        m.size().width as f64,
        m.size().height as f64,
    );
    let (edge, offset) = nearest_dock(rect, p);
    let mut s = settings::get_settings(app_handle);
    s.overlay_position = edge;
    s.overlay_dock_offset = offset;
    settings::write_settings(app_handle, s);
    log::info!("barra agganciata: bordo {:?}, punto {:.3}", edge, offset);
    refresh_overlay_geometry(app_handle);
}

/// Generation counter bumped every time the overlay is shown. The delayed
/// `hide()` below only unmaps the window if no show happened after it was
/// scheduled, so a hide left over from a finished transcription can never
/// take down the overlay of a session that started in the meantime — e.g. a
/// press the coordinator remembered while the pipeline was busy and started
/// the instant it drained, well inside the 300 ms hide delay.
static OVERLAY_SHOW_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Hides the recording overlay window with fade-out animation
pub fn hide_recording_overlay(app_handle: &AppHandle) {
    // Always hide the overlay regardless of settings - if setting was changed while recording,
    // we still want to hide it properly
    if let Some(overlay_window) = app_handle.get_webview_window("recording_overlay") {
        // Snapshot before doing anything observable, so any show that lands
        // after this point invalidates the delayed hide below.
        let scheduled_at = OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst);
        // Emit event to trigger fade-out animation
        let _ = overlay_window.emit("hide-overlay", ());
        // Hide the window after a short delay to allow animation to complete,
        // unless a newer session has shown the overlay again by then.
        let window_clone = overlay_window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            if OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst) != scheduled_at {
                log::debug!("Skipping stale overlay hide: a newer session is showing the overlay");
                return;
            }
            let _ = window_clone.hide();
        });
    }
}

// Cached "overlay is enabled" flag, kept in sync with overlay_style. Avoids
// reading the Tauri store on every audio callback (~24 Hz during recording).
// Defaults to false so the audio path doesn't emit until lib.rs::setup
// populates the cache from initial settings.
static OVERLAY_ENABLED: AtomicBool = AtomicBool::new(false);

/// Tracks whether gtk-layer-shell was successfully initialized (Linux only).
/// Used to skip layer-shell calls when the window is a regular fallback.
#[cfg(target_os = "linux")]
static LAYER_SHELL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Update the cached overlay-enabled flag. Called from `lib.rs` at
/// startup after settings load, and from `change_overlay_style_setting`
/// whenever the user changes whether the overlay is shown.
pub fn update_overlay_enabled_cache(enabled: bool) {
    OVERLAY_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn emit_levels(app_handle: &AppHandle, levels: &[f32]) {
    // Skip emission when the overlay is disabled. The recording_overlay
    // window is created at boot regardless of overlay_style, so without this
    // guard a hidden overlay's WebKit subprocess still
    // processes every event. Each event drives some kind of WebKit
    // C++ allocation that accumulates without bound (mechanism not
    // directly characterized; see issue #1279 for the investigation).
    // For users with `overlay_style: none` (the Linux default) this skip
    // eliminates the upstream driver of that accumulation.
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // Throttle to ~30 FPS. Even with the overlay enabled, the raw audio
    // callback fires far faster than the UI needs; capping emission rate
    // cuts the per-frame `eval_script`/IPC volume that drives the wry
    // memory growth in issue #1279 (upstream tauri-apps/wry#1489).
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let last = LAST_MIC_LEVEL_EMIT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < EMIT_THROTTLE_MS {
        return;
    }
    LAST_MIC_LEVEL_EMIT.store(now, Ordering::Relaxed);

    // Target only the overlay window. In Tauri 2 both `AppHandle::emit`
    // and `WebviewWindow::emit` broadcast to all webviews; Tauri's
    // listener filter then skips webviews with no registered listener
    // for the event, so the settings webview never received `mic-level`.
    // But the previous dual-call pattern still produced two `eval_script`
    // calls to the overlay per audio callback (one from each .emit()).
    // `emit_to` with the overlay's window label produces a single
    // eval_script call per callback, cutting the per-callback WebKit
    // dispatch work in half.
    let _ = app_handle.emit_to("recording_overlay", "mic-level", levels);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_hit_test_uses_half_open_physical_bounds() {
        let position = PhysicalPosition::new(-2560, -200);
        let size = PhysicalSize::new(2560, 1440);

        assert!(is_mouse_within_monitor((-2560, -200), &position, &size));
        assert!(is_mouse_within_monitor((-1, 1239), &position, &size));
        assert!(!is_mouse_within_monitor((0, 0), &position, &size));
        assert!(!is_mouse_within_monitor((-1, 1240), &position, &size));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_cursor_hit_test_does_not_scale_physical_monitor_bounds() {
        let position = PhysicalPosition::new(1920, 0);
        let size = PhysicalSize::new(3840, 2160);
        let cursor = (5000, 1000);

        assert!(is_mouse_within_monitor(cursor, &position, &size));

        // This is the old mixed-coordinate comparison. It excludes a cursor
        // that is visibly inside a secondary display running at 150%.
        let scale = 1.5;
        let logical_position = PhysicalPosition::new(
            (position.x as f64 / scale) as i32,
            (position.y as f64 / scale) as i32,
        );
        let logical_size = PhysicalSize::new(
            (size.width as f64 / scale) as u32,
            (size.height as f64 / scale) as u32,
        );
        assert!(!is_mouse_within_monitor(
            cursor,
            &logical_position,
            &logical_size
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_overlay_bounds_use_destination_monitor_scale() {
        let monitor_position = PhysicalPosition::new(1920, 0);
        let monitor_size = PhysicalSize::new(3840, 2160);

        assert_eq!(
            windows_overlay_bounds(
                monitor_position,
                monitor_size,
                1.5,
                1.0,
                OVERLAY_WIDTH,
                OVERLAY_HEIGHT,
                OverlayPosition::Bottom,
                0.5,
            ),
            (3615, 2040, 450, 108)
        );
        assert_eq!(
            windows_overlay_bounds(
                monitor_position,
                monitor_size,
                1.5,
                1.0,
                OVERLAY_WIDTH,
                OVERLAY_HEIGHT,
                OverlayPosition::Top,
                0.5,
            ),
            (3615, 6, 450, 108)
        );
        // eagleVoice: bordi sinistro e destro, centrati in verticale.
        for (edge, expected) in [
            (OverlayPosition::Left, (1926, 1026, 450, 108)),
            (OverlayPosition::Right, (5304, 1026, 450, 108)),
        ] {
            assert_eq!(
                windows_overlay_bounds(
                    monitor_position,
                    monitor_size,
                    1.5,
                    1.0,
                    OVERLAY_WIDTH,
                    OVERLAY_HEIGHT,
                    edge,
                    0.5,
                ),
                expected
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_overlay_bounds_support_negative_monitor_origins() {
        assert_eq!(
            windows_overlay_bounds(
                PhysicalPosition::new(-2560, -200),
                PhysicalSize::new(2560, 1440),
                1.25,
                1.0,
                OVERLAY_STREAM_WIDTH,
                OVERLAY_STREAM_HEIGHT,
                OverlayPosition::Bottom,
                0.5,
            ),
            (-1530, 1080, 500, 150)
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_overlay_bounds_grow_with_text_scale_without_moving_the_anchored_edge() {
        let monitor_position = PhysicalPosition::new(-2560, -200);
        let monitor_size = PhysicalSize::new(2560, 1440);

        let (x, y, width, height) = windows_overlay_bounds(
            monitor_position,
            monitor_size,
            1.25,
            1.1,
            OVERLAY_STREAM_WIDTH,
            OVERLAY_STREAM_HEIGHT,
            OverlayPosition::Bottom,
            0.5,
        );
        // 400x120 logical at 1.25 DPI x 1.1 text, still centered horizontally.
        assert_eq!((x, y, width, height), (-1555, 1065, 550, 165));
        // Bottom edge unchanged from the 1.0 case above (1080 + 150).
        assert_eq!(y + height, 1230);

        let (_, top_y, _, _) = windows_overlay_bounds(
            monitor_position,
            monitor_size,
            1.25,
            1.1,
            OVERLAY_STREAM_WIDTH,
            OVERLAY_STREAM_HEIGHT,
            OverlayPosition::Top,
            0.5,
        );
        // Top offset rides the DPI scale alone, so the top edge doesn't move.
        assert_eq!(top_y, -195);
    }

    #[test]
    fn bolla_si_aggancia_al_bordo_piu_vicino() {
        let mon = (0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(nearest_dock(mon, (1900.0, 900.0)).0, OverlayPosition::Right);
        assert_eq!(nearest_dock(mon, (10.0, 500.0)).0, OverlayPosition::Left);
        assert_eq!(nearest_dock(mon, (300.0, 1075.0)), (OverlayPosition::Bottom, 300.0 / 1920.0));
        assert_eq!(nearest_dock(mon, (960.0, 3.0)).0, OverlayPosition::Top);
        // Monitor con origine negativa: il punto è relativo al monitor.
        let (edge, off) = nearest_dock((-2560.0, 0.0, 2560.0, 1440.0), (-10.0, 1080.0));
        assert_eq!(edge, OverlayPosition::Right);
        assert!((off - 0.75).abs() < 1e-9);
    }

    #[test]
    fn bolla_resta_nel_monitor_e_rispetta_il_punto() {
        let mon = (0.0, 0.0, 1920.0, 1080.0);
        // Sotto, a destra: la pillola larga 300 non esce dal bordo destro.
        assert_eq!(
            dock_origin(mon, (300.0, 72.0), OverlayPosition::Bottom, 1.0, 4.0, 4.0, 40.0),
            (1616.0, 968.0)
        );
        // Sotto, punto 0,25: centro della sfera a 480.
        assert_eq!(
            dock_origin(mon, (52.0, 52.0), OverlayPosition::Bottom, 0.25, 4.0, 4.0, 40.0),
            (454.0, 988.0)
        );
        // Destra, punto 0,8: attaccata al bordo destro.
        assert_eq!(
            dock_origin(mon, (52.0, 52.0), OverlayPosition::Right, 0.8, 4.0, 4.0, 40.0),
            (1864.0, 838.0)
        );
        // Sinistra in fondo: sopra la barra delle applicazioni.
        assert_eq!(
            dock_origin(mon, (360.0, 190.0), OverlayPosition::Left, 1.0, 4.0, 4.0, 40.0),
            (4.0, 850.0)
        );
    }
}
