//! The tray icon on desktops. With it, closing the window only hides it: the
//! app stays in the tray and keeps syncing until "Выход".

use std::sync::Mutex;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::sync::Overall;
use crate::AppState;

/// Started at login: stay in the tray, no window.
pub const HIDDEN_ARG: &str = "--hidden";

pub struct Tray {
    icon: TrayIcon,
    status: MenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
    sync_now: MenuItem<Wry>,
    /// The app's icon, small, without a badge.
    base: Image<'static>,
    shown: Mutex<Option<Overall>>,
}

/// Makes the tray icon. `None` where the desktop has no tray (or, on Linux,
/// no AppIndicator library): the window then closes the app as before.
pub fn create(app: &AppHandle) -> Option<Tray> {
    // The Linux tray panics instead of failing when the library is missing.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(app))) {
        Ok(Ok(tray)) => Some(tray),
        Ok(Err(e)) => {
            tracing::warn!("no tray icon: {e}");
            None
        }
        Err(_) => {
            tracing::warn!("no tray icon: the desktop has no tray support");
            None
        }
    }
}

fn build(app: &AppHandle) -> tauri::Result<Tray> {
    let open = MenuItem::with_id(app, "open", "Открыть RustCloud", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "", false, None::<&str>)?;
    let sync_now = MenuItem::with_id(app, "sync-now", "Синхронизировать сейчас", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Приостановить синхронизацию", true, false, None::<&str>)?;
    let autostart = app.autolaunch().is_enabled().unwrap_or(false);
    let login =
        CheckMenuItem::with_id(app, "autostart", "Запускать при входе в систему", true, autostart, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &status,
            &sync_now,
            &pause,
            &PredefinedMenuItem::separator(app)?,
            &login,
            &quit,
        ],
    )?;

    let base = app.default_window_icon().map(small).unwrap_or_else(|| Image::new_owned(vec![0; 4], 1, 1));
    let icon = TrayIconBuilder::with_id("main")
        .icon(base.clone())
        .tooltip("RustCloud")
        .menu(&menu)
        // A click opens the window, the right button the menu (Windows; Linux
        // trays always show the menu).
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "sync-now" => app.state::<AppState>().pairs.sync_all(),
            "pause" => set_paused(app, !app.state::<AppState>().pairs.is_paused()),
            "autostart" => toggle_autostart(app, &login),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(Tray { icon, status, pause, sync_now, base, shown: Mutex::new(None) })
}

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Pauses or resumes every pair; the window hears it as `sync-paused`.
pub fn set_paused(app: &AppHandle, paused: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::set_paused(&app, paused).await;
    });
}

fn toggle_autostart(app: &AppHandle, item: &CheckMenuItem<Wry>) {
    let launch = app.autolaunch();
    let on = launch.is_enabled().unwrap_or(false);
    if let Err(e) = if on { launch.disable() } else { launch.enable() } {
        tracing::warn!("autostart: {e}");
    }
    // The menu shows what the system has, whatever happened.
    let _ = item.set_checked(launch.is_enabled().unwrap_or(on));
}

/// Shows `overall` in the tray (once per change).
pub fn refresh(app: &AppHandle) {
    let (Some(tray), Some(state)) = (app.try_state::<Tray>(), app.try_state::<AppState>()) else { return };
    let overall = state.pairs.overall();
    let mut shown = tray.shown.lock().unwrap();
    if *shown == Some(overall) {
        return;
    }
    *shown = Some(overall);

    let (text, badge) = describe(&overall);
    let _ = tray.status.set_text(text);
    let _ = tray.icon.set_tooltip(Some(format!("RustCloud — {text}")));
    let _ = tray.pause.set_checked(overall.paused);
    let _ = tray.sync_now.set_enabled(!overall.paused && overall.pairs > 0);
    let icon = match badge {
        Some(color) => with_badge(&tray.base, color),
        None => tray.base.clone(),
    };
    let _ = tray.icon.set_icon(Some(icon));
}

const GREEN: [u8; 3] = [0x2e, 0xa0, 0x43];
const BLUE: [u8; 3] = [0x1f, 0x6f, 0xeb];
const RED: [u8; 3] = [0xd7, 0x3a, 0x49];
const GRAY: [u8; 3] = [0x8b, 0x94, 0x9e];

/// The status line of the menu and the badge's colour.
fn describe(o: &Overall) -> (&'static str, Option<[u8; 3]>) {
    if o.paused {
        ("Синхронизация приостановлена", Some(GRAY))
    } else if o.pairs == 0 {
        ("Нет синхронизируемых папок", None)
    } else if o.errors > 0 {
        ("Ошибка синхронизации — откройте окно", Some(RED))
    } else if o.syncing > 0 {
        ("Синхронизация…", Some(BLUE))
    } else {
        ("Всё синхронизировано", Some(GREEN))
    }
}

/// The icon at no more than 64 px: a tray needs no more, and Linux writes it
/// to a file on every change.
fn small(icon: &Image<'_>) -> Image<'static> {
    let (w, h) = (icon.width(), icon.height());
    let k = (w.max(h) / 64).max(1);
    let (sw, sh) = (w / k, h / k);
    let src = icon.rgba();
    let mut out = Vec::with_capacity((sw * sh * 4) as usize);
    for y in 0..sh {
        for x in 0..sw {
            let mut sum = [0u32; 4];
            for dy in 0..k {
                for dx in 0..k {
                    let i = (((y * k + dy) * w + x * k + dx) * 4) as usize;
                    sum.iter_mut().zip(&src[i..i + 4]).for_each(|(s, v)| *s += u32::from(*v));
                }
            }
            out.extend(sum.map(|s| (s / (k * k)) as u8));
        }
    }
    Image::new_owned(out, sw, sh)
}

/// The icon with a coloured dot in the lower right corner.
fn with_badge(icon: &Image<'_>, color: [u8; 3]) -> Image<'static> {
    let (w, h) = (icon.width(), icon.height());
    let mut px = icon.rgba().to_vec();
    let size = w.min(h) as f32;
    let r = size * 0.12;
    let ring = size * 0.05;
    let (cx, cy) = (w as f32 - r - ring, h as f32 - r - ring);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            // A white ring keeps the dot visible on any icon and panel.
            let (rgb, cover) = if d <= r {
                (color, (r - d + 0.5).min(1.0))
            } else {
                ([255, 255, 255], (r + ring - d + 0.5).clamp(0.0, 1.0))
            };
            if cover <= 0.0 {
                continue;
            }
            let i = ((y * w + x) * 4) as usize;
            for c in 0..3 {
                px[i + c] = (f32::from(px[i + c]) * (1.0 - cover) + f32::from(rgb[c]) * cover) as u8;
            }
            px[i + 3] = (f32::from(px[i + 3]) * (1.0 - cover) + 255.0 * cover) as u8;
        }
    }
    Image::new_owned(px, w, h)
}
