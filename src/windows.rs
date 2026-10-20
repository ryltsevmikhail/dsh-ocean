use anyhow::{Context, Result};
use tauri::image::Image;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

use crate::service::Server;
use crate::state::AppState;
use crate::util;

const ICON: &[u8] = include_bytes!("../icons/icon.png");
const MIN_ZOOM: f64 = 0.2;
const MAX_ZOOM: f64 = 10.0;
const ZOOM_STEP: f64 = 0.2;

/// Auxiliary windows keep their existing hide-only behavior. Closing a primary
/// window quits only when the user explicitly disables close-to-tray.
pub fn close_exits_app(label: &str, close_to_tray: Option<bool>) -> bool {
    matches!(label, "main" | "setup") && !close_to_tray.unwrap_or(true)
}

pub fn create_setup(app: &tauri::App) -> Result<()> {
    let icon = Image::from_bytes(ICON)?;
    WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("index.html".into()))
        .title("DSH Ocean")
        .inner_size(760.0, 540.0)
        .min_inner_size(560.0, 400.0)
        .center()
        .icon(icon)?
        .build()
        .context("create setup window")?;
    Ok(())
}

pub fn open_main(app: &AppHandle, server: &Server) -> Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        navigate_to_server(&window, server)?;
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    let icon = Image::from_bytes(ICON)?;
    let url = crate::auth::token_url(&server.base_url, &server.token);
    let app_for_navigation = app.clone();
    let initial_origin = server.base_url.clone();
    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("DSH Ocean")
        .inner_size(1320.0, 880.0)
        .min_inner_size(720.0, 480.0)
        .center()
        .icon(icon)?
        .initialization_script(crate::task_notifications::OBSERVER)
        .on_navigation(move |url| {
            let base = app_for_navigation
                .state::<AppState>()
                .server()
                .map(|s| s.base_url)
                .unwrap_or_else(|| initial_origin.clone());
            navigation_allowed(url, &base)
        })
        .on_page_load(|window, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                let _ = window.eval("document.documentElement.dataset.dshDesktop = '1';");
            }
        })
        .build()
        .context("create DSH window")?;

    attach_zoom_shortcuts(&window);
    crate::download_ui::attach(&window, server.base_url.clone());
    crate::notification_ui::attach_tasks(&window, server.base_url.clone());
    Ok(())
}

pub fn navigate_to_server(window: &WebviewWindow, server: &Server) -> Result<()> {
    window
        .navigate(crate::auth::token_url(&server.base_url, &server.token))
        .context("navigate DSH window")
}

pub fn show_main_or_setup(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    if let Some(window) = app.get_webview_window("setup") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn navigation_allowed(url: &Url, server: &Url) -> bool {
    match crate::navigation::decide(url, server) {
        crate::navigation::Decision::Allow => true,
        crate::navigation::Decision::External => {
            util::open_url(url.as_str());
            false
        }
        crate::navigation::Decision::Reject => false,
    }
}

fn attach_zoom_shortcuts(window: &WebviewWindow) {
    let window_for_webview = window.clone();
    let _ = window.with_webview(move |platform| {
        use gtk::gdk::keys::constants as key;
        use gtk::prelude::*;

        let view = platform.inner();
        let window = window_for_webview.clone();
        view.add_events(gtk::gdk::EventMask::KEY_PRESS_MASK);
        view.connect_key_press_event(move |_, event| {
            if !event.state().contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                return gtk::glib::Propagation::Proceed;
            }

            let keyval = event.keyval();
            let is_zoom_in = keyval == key::equal || keyval == key::plus || keyval == key::KP_Add;
            let is_zoom_out = keyval == key::minus || keyval == key::KP_Subtract;
            let is_reset = keyval == key::_0;

            if !is_zoom_in && !is_zoom_out && !is_reset {
                return gtk::glib::Propagation::Proceed;
            }

            let state = window.app_handle().state::<AppState>();
            let mut zoom = state.zoom.lock().expect("zoom mutex");
            let next = if is_reset {
                1.0
            } else if is_zoom_in {
                (*zoom + ZOOM_STEP).min(MAX_ZOOM)
            } else {
                (*zoom - ZOOM_STEP).max(MIN_ZOOM)
            };
            *zoom = next;
            let _ = window.set_zoom(next);
            gtk::glib::Propagation::Stop
        });
    });
}

#[cfg(test)]
mod close_tests {
    use super::close_exits_app;

    #[test]
    fn primary_windows_hide_by_default_and_quit_only_on_explicit_opt_out() {
        for label in ["main", "setup"] {
            assert!(!close_exits_app(label, None));
            assert!(!close_exits_app(label, Some(true)));
            assert!(close_exits_app(label, Some(false)));
        }
    }

    #[test]
    fn auxiliary_windows_never_quit_the_application() {
        for label in ["remote", "downloads", "unknown"] {
            for setting in [None, Some(true), Some(false)] {
                assert!(!close_exits_app(label, setting));
            }
        }
    }
}
