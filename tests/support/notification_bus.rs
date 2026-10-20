use dsh_ocean::notifications::{DESTINATION, OBJECT};
use glib::variant::ToVariant;
use gtk::{gio, glib};
use std::sync::{Arc, Mutex};

const XML: &str = r#"<node><interface name="org.freedesktop.Notifications">
<method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/><arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method>
<method name="CloseNotification"><arg type="u" direction="in"/></method>
<signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
<signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
</interface></node>"#;
#[derive(Default)]
pub struct Received {
    pub packets: Vec<glib::Variant>,
    pub closed: Vec<u32>,
}
pub fn assert_private() {
    assert_eq!(
        std::env::var("DSH_OCEAN_TEST_PRIVATE_BUS").as_deref(),
        Ok("1")
    );
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("private bus");
    assert!(
        address.contains("/tmp/dbus-") && !address.contains("/run/user/"),
        "never acquire the user's real notification name"
    );
}
pub fn request_name(connection: &gio::DBusConnection) {
    let reply = connection
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            Some(&(DESTINATION, 0u32).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            None::<&gio::Cancellable>,
        )
        .unwrap();
    assert_eq!(reply.get::<(u32,)>(), Some((1,)));
}
pub fn register(
    connection: &gio::DBusConnection,
    received: Arc<Mutex<Received>>,
) -> gio::RegistrationId {
    let info = gio::DBusNodeInfo::for_xml(XML)
        .unwrap()
        .lookup_interface(DESTINATION)
        .unwrap();
    connection
        .register_object(
            OBJECT,
            &info,
            move |_, _, _, _, method, parameters, invocation| {
                let mut state = received.lock().unwrap();
                match method {
                    "Notify" => {
                        state.packets.push(parameters);
                        let id = state.packets.len() as u32;
                        invocation.return_value(Some(&(id,).to_variant()));
                    }
                    "CloseNotification" => {
                        state.closed.push(parameters.get::<(u32,)>().unwrap().0);
                        invocation.return_value(Some(&().to_variant()));
                    }
                    _ => invocation.return_dbus_error(
                        "org.freedesktop.DBus.Error.UnknownMethod",
                        "unsupported fixture method",
                    ),
                }
            },
            |_, _, _, _, _| ().to_variant(),
            |_, _, _, _, _, _| false,
        )
        .unwrap()
}
