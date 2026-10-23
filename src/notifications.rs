//! Privacy-preserving freedesktop notifications (enabled by default in app preferences). Payloads are fixed by kind: callers cannot
//! accidentally forward task text, filenames, URLs, cookies or provider errors.
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use gio::prelude::*;
use glib::variant::ToVariant;
use gtk::{gio, glib};
use serde::{Deserialize, Serialize};

pub const DESTINATION: &str = "org.freedesktop.Notifications";
pub const OBJECT: &str = "/org/freedesktop/Notifications";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Task,
    DownloadSaved,
    DownloadFailed,
}
impl Kind {
    pub fn parameters(self, replaces: u32) -> glib::Variant {
        self.parameters_for(crate::i18n::active_language(), replaces)
    }
    pub fn parameters_for(self, language: crate::i18n::Language, replaces: u32) -> glib::Variant {
        let tr = |key| crate::i18n::message(language, key).expect("validated notification message");
        let (summary, body, category) = match self {
            Self::Task => (
                tr("notification.task_title"),
                tr("notification.task_body"),
                "im.received",
            ),
            Self::DownloadSaved => (
                tr("notification.download_saved"),
                tr("notification.download_saved_body"),
                "transfer.complete",
            ),
            Self::DownloadFailed => (
                tr("notification.download_failed"),
                tr("notification.download_failed_body"),
                "transfer.error",
            ),
        };
        // freedesktop body supports markup; our messages are always plain text.
        let body = glib::markup_escape_text(body);
        let hints: HashMap<String, glib::Variant> = [
            ("desktop-entry", "dsh-ocean".to_variant()),
            ("category", category.to_variant()),
            ("transient", true.to_variant()),
            ("suppress-sound", true.to_variant()),
            ("urgency", 0u8.to_variant()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        (
            "DSH Ocean",
            replaces,
            "dsh-ocean",
            summary,
            body.as_str(),
            vec!["default", tr("notification.open")],
            hints,
            5000i32,
        )
            .to_variant()
    }
}
#[derive(Default)]
pub struct Limiter {
    recent: VecDeque<Instant>,
    last: HashMap<Kind, Instant>,
}
impl Limiter {
    pub fn admit(&mut self, kind: Kind, now: Instant) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= Duration::from_secs(30))
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= 3
            || self
                .last
                .get(&kind)
                .is_some_and(|at| now.saturating_duration_since(*at) < Duration::from_secs(10))
        {
            return false;
        }
        self.recent.push_back(now);
        self.last.insert(kind, now);
        true
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preference {
    version: u32,
    enabled: bool,
}
pub fn load_preference(path: &Path) -> Result<bool> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e).context("open notification preference"),
    };
    let metadata = file.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && metadata.nlink() == 1 && metadata.len() <= 8192,
        "invalid notification preference file"
    );
    let mut bytes = Vec::new();
    file.take(8193).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 8192,
        "notification preference grew beyond limit"
    );
    let value: Preference =
        serde_json::from_slice(&bytes).context("invalid notification preference")?;
    anyhow::ensure!(
        value.version == 1,
        "unsupported notification preference version"
    );
    Ok(value.enabled)
}
pub fn save_preference(path: &Path, enabled: bool) -> Result<()> {
    load_preference(path)?; // Never silently replace a malformed/link/future-format file.
    let parent = path.parent().context("notification preference parent")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let temp = parent.join(format!(
        ".notification-pref-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec(&Preference {
            version: 1,
            enabled,
        })?)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        assert!(
            temp.parent() == Some(parent)
                && temp
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".notification-pref-")
        );
        let _ = fs::remove_file(&temp);
    }
    result
}

#[derive(Clone)]
struct Ticket {
    owner: String,
    id: u32,
}
#[derive(Default)]
pub struct Client {
    enabled: AtomicBool,
    connecting: AtomicBool,
    proxy: Mutex<Option<gio::DBusProxy>>,
    pending: Mutex<HashSet<Kind>>,
    ids: Mutex<HashMap<Kind, Ticket>>,
    limiter: Mutex<Limiter>,
    activate: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl Client {
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }
    pub fn set_activate(&self, action: Arc<dyn Fn() + Send + Sync>) {
        *self.activate.lock().expect("notification action mutex") = Some(action);
    }
    pub fn set_enabled(self: &Arc<Self>, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            self.pending
                .lock()
                .expect("notification queue mutex")
                .clear();
            let this = self.clone();
            glib::idle_add_once(move || this.close_owned());
        }
    }
    pub fn post(self: &Arc<Self>, kind: Kind) {
        if !self.enabled()
            || !self
                .limiter
                .lock()
                .expect("notification limiter mutex")
                .admit(kind, Instant::now())
        {
            return;
        }
        let this = self.clone();
        glib::idle_add_once(move || {
            if !this.enabled() {
                return;
            }
            if this
                .proxy
                .lock()
                .expect("notification proxy mutex")
                .is_some()
            {
                this.deliver(kind);
            } else {
                this.pending
                    .lock()
                    .expect("notification queue mutex")
                    .insert(kind);
                this.connect();
            }
        });
    }
    fn connect(self: &Arc<Self>) {
        if self.connecting.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        gio::DBusProxy::for_bus(
            gio::BusType::Session,
            gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
            None,
            DESTINATION,
            OBJECT,
            DESTINATION,
            None::<&gio::Cancellable>,
            move |result| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                this.connecting.store(false, Ordering::SeqCst);
                let Ok(proxy) = result else {
                    this.pending
                        .lock()
                        .expect("notification queue mutex")
                        .clear();
                    eprintln!("dsh-ocean: desktop notification service unavailable");
                    return;
                };
                let weak = Arc::downgrade(&this);
                proxy.connect_g_signal(move |_, sender, signal, parameters| {
                    let Some(this) = weak.upgrade() else {
                        return;
                    };
                    if signal == "ActionInvoked" {
                        if let Some((id, action)) = parameters.get::<(u32, String)>() {
                            let owned = this
                                .ids
                                .lock()
                                .expect("notification ids mutex")
                                .values()
                                .any(|t| Some(t.owner.as_str()) == sender && t.id == id);
                            if owned && action == "default" && this.enabled() {
                                let action = this
                                    .activate
                                    .lock()
                                    .expect("notification action mutex")
                                    .clone();
                                if let Some(action) = action {
                                    action();
                                }
                            }
                        }
                    } else if signal == "NotificationClosed" {
                        if let Some((id, _reason)) = parameters.get::<(u32, u32)>() {
                            this.ids
                                .lock()
                                .expect("notification ids mutex")
                                .retain(|_, t| !(Some(t.owner.as_str()) == sender && t.id == id));
                        }
                    }
                });
                *this.proxy.lock().expect("notification proxy mutex") = Some(proxy);
                let pending: Vec<_> = this
                    .pending
                    .lock()
                    .expect("notification queue mutex")
                    .drain()
                    .collect();
                for kind in pending {
                    this.deliver(kind);
                }
            },
        );
    }
    fn deliver(self: &Arc<Self>, kind: Kind) {
        if !self.enabled() {
            return;
        }
        let Some(proxy) = self.proxy.lock().expect("notification proxy mutex").clone() else {
            return;
        };
        let Some(owner) = proxy.name_owner().map(|s| s.to_string()) else {
            eprintln!("dsh-ocean: desktop notification service has no owner");
            return;
        };
        let replaces = self
            .ids
            .lock()
            .expect("notification ids mutex")
            .get(&kind)
            .filter(|t| t.owner == owner)
            .map_or(0, |t| t.id);
        let parameters = kind.parameters(replaces);
        let this = self.clone();
        let connection = proxy.connection();
        let owner_for_reply = owner.clone();
        connection.call(
            Some(&owner),
            OBJECT,
            DESTINATION,
            "Notify",
            Some(&parameters),
            Some(glib::VariantTy::new("(u)").unwrap()),
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
            move |result| {
                if let Ok(reply) = result {
                    if let Some((id,)) = reply.get::<(u32,)>() {
                        let ticket = Ticket {
                            owner: owner_for_reply,
                            id,
                        };
                        if this.enabled() {
                            this.ids
                                .lock()
                                .expect("notification ids mutex")
                                .insert(kind, ticket);
                        } else {
                            this.close_ticket(&ticket);
                        }
                    }
                } else {
                    eprintln!("dsh-ocean: desktop notification delivery failed");
                }
            },
        );
    }
    fn close_ticket(&self, ticket: &Ticket) {
        let Some(proxy) = self.proxy.lock().expect("notification proxy mutex").clone() else {
            return;
        };
        // Pin the unique service owner, never send an old ID to a replacement daemon.
        proxy.connection().call(
            Some(&ticket.owner),
            OBJECT,
            DESTINATION,
            "CloseNotification",
            Some(&(ticket.id,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
            |_| {},
        );
    }
    fn close_owned(&self) {
        let tickets: Vec<_> = self
            .ids
            .lock()
            .expect("notification ids mutex")
            .drain()
            .map(|(_, t)| t)
            .collect();
        for ticket in tickets {
            self.close_ticket(&ticket);
        }
    }
}
