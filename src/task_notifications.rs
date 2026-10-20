//! A constant-only, top-frame isolated-world bridge. The page observer only
//! reads the existing mux and never opens another socket/session or emits RPCs.
use anyhow::{Context, Result};
use gtk::{gio, prelude::*};
use javascriptcore::ValueExt;
use std::rc::Rc;
use url::Url;
use webkit2gtk::{
    UserContentInjectedFrames, UserContentManagerExt, UserScript, UserScriptInjectionTime, WebView,
    WebViewExt,
};

pub const OBSERVER: &str = include_str!("task_observer.js");
const WORLD: &str = "dev.dsh.ocean.task-notifications";
const HANDLER: &str = "oceanTaskActivity";
const BRIDGE: &str = r#"(() => {
  if (window.top !== window) return;
  const key = Symbol.for('dev.dsh.ocean.task-bridge');
  if (window[key]) return;
  Object.defineProperty(window, key, { value: true });
  const ready = () => window.dispatchEvent(new Event('dsh-ocean:task-bridge-ready'));
  window.addEventListener('dsh-ocean:task-activity', () => {
    try { window.webkit.messageHandlers.oceanTaskActivity.postMessage(1); } catch {}
  });
  window.addEventListener('dsh-ocean:task-adapter-ready', ready);
  ready();
})();"#;

pub fn trusted_document(document: &Url, server: &Url) -> bool {
    matches!(document.scheme(), "http" | "https")
        && matches!(server.scheme(), "http" | "https")
        && document.username().is_empty()
        && document.password().is_none()
        && document.origin() == server.origin()
}
pub type Origin = Rc<dyn Fn() -> Option<Url>>;
pub fn attach(view: &WebView, server: Origin, activity: Rc<dyn Fn()>) -> Result<()> {
    let manager = view
        .user_content_manager()
        .context("missing WebKit content manager")?;
    anyhow::ensure!(
        manager.register_script_message_handler_in_world(HANDLER, WORLD),
        "task notification handler already registered"
    );
    let weak = view.downgrade();
    manager.connect_script_message_received(Some(HANDLER), move |_, result| {
        let Some(view) = weak.upgrade() else {
            return;
        };
        let Some(base) = server() else {
            return;
        };
        let Some(uri) = view.uri().and_then(|raw| Url::parse(&raw).ok()) else {
            return;
        };
        if !trusted_document(&uri, &base) {
            return;
        }
        let Some(value) = result.js_value() else {
            return;
        };
        // Never stringify or forward arbitrary page values, content, IDs or paths.
        if value.is_number() && value.to_double() == 1.0 {
            activity();
        }
    });
    manager.add_script(&UserScript::new(
        OBSERVER,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::Start,
        &[],
        &[],
    ));
    manager.add_script(&UserScript::for_world(
        BRIDGE,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::Start,
        WORLD,
        &[],
        &[],
    ));
    // Cover a document already being loaded by Tauri. Its early main-world
    // initialization script has buffered at most one anonymous activity signal.
    view.evaluate_javascript(OBSERVER, None, None, None::<&gio::Cancellable>, |_| {});
    view.evaluate_javascript(BRIDGE, Some(WORLD), None, None::<&gio::Cancellable>, |_| {});
    Ok(())
}
