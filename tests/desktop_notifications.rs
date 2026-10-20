//! Run only under dbus-run-session. The fake daemon must never replace the
//! user's actual notification service or create a real desktop notification.
use dsh_ocean::notifications::{Client, Kind, DESTINATION, OBJECT};
use glib::variant::ToVariant;
use gtk::{gio, glib};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[path = "support/notification_bus.rs"]
mod bus;
use bus::{register, request_name, Received};
fn tick() {
    for _ in 0..32 {
        if !glib::MainContext::default().pending() {
            break;
        }
        glib::MainContext::default().iteration(false);
    }
}
fn wait_for(test: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !test() {
        tick();
        assert!(Instant::now() < deadline, "notification fixture timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    tick();
}
fn spin() {
    let end = Instant::now() + Duration::from_millis(100);
    while Instant::now() < end {
        tick();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires an explicitly isolated test D-Bus session"]
fn delivery_actions_disable_and_daemon_replacement_are_private_and_owned() {
    bus::assert_private();
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("private bus");
    let old_connection =
        gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).unwrap();
    let old = Arc::new(Mutex::new(Received::default()));
    let old_registration = register(&old_connection, old.clone());
    request_name(&old_connection);
    let client = Arc::new(Client::default());
    client.post(Kind::Task);
    spin();
    assert!(
        old.lock().unwrap().packets.is_empty(),
        "unconfigured transport must not send"
    );
    let activated = Arc::new(AtomicUsize::new(0));
    let counter = activated.clone();
    client.set_activate(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    client.set_enabled(true);
    client.post(Kind::Task);
    wait_for(|| old.lock().unwrap().packets.len() == 1);
    client.post(Kind::DownloadSaved);
    client.post(Kind::DownloadFailed);
    wait_for(|| old.lock().unwrap().packets.len() == 3);
    for packet in &old.lock().unwrap().packets {
        assert_eq!(packet.type_().as_str(), "(susssasa{sv}i)");
    }
    old_connection
        .emit_signal(
            None,
            OBJECT,
            DESTINATION,
            "ActionInvoked",
            Some(&(999u32, "default").to_variant()),
        )
        .unwrap();
    spin();
    assert_eq!(activated.load(Ordering::Relaxed), 0);
    old_connection
        .emit_signal(
            None,
            OBJECT,
            DESTINATION,
            "ActionInvoked",
            Some(&(1u32, "default").to_variant()),
        )
        .unwrap();
    wait_for(|| activated.load(Ordering::Relaxed) == 1);

    // A replacement daemon can reuse IDs. Its action must not activate an old
    // ticket, and disabling must send Close only to the original unique owner.
    old_connection
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "ReleaseName",
            Some(&(DESTINATION,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
        )
        .unwrap();
    let replacement = gio::DBusConnection::for_address_sync(
        &address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        None::<&gio::Cancellable>,
    )
    .unwrap();
    let new = Arc::new(Mutex::new(Received::default()));
    let new_registration = register(&replacement, new.clone());
    request_name(&replacement);
    spin();
    replacement
        .emit_signal(
            None,
            OBJECT,
            DESTINATION,
            "ActionInvoked",
            Some(&(1u32, "default").to_variant()),
        )
        .unwrap();
    spin();
    assert_eq!(activated.load(Ordering::Relaxed), 1);
    client.set_enabled(false);
    wait_for(|| old.lock().unwrap().closed.len() == 3);
    assert!(
        new.lock().unwrap().closed.is_empty(),
        "never close another daemon's reused IDs"
    );
    let disabled = Arc::new(Client::default());
    disabled.post(Kind::DownloadSaved);
    spin();
    assert!(new.lock().unwrap().packets.is_empty());
    old_connection.unregister_object(old_registration).unwrap();
    replacement.unregister_object(new_registration).unwrap();
}
