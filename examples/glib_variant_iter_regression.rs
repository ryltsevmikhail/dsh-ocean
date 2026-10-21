//! Headless regression under the application's actual release profile, not the
//! test harness's panic-unwind profile. This does not launch Ocean or initialize GTK.
#[path = "../tests/support/glib_variant_iter_cases.rs"]
mod cases;

fn main() {
    cases::run();
    println!("Patched GLib string iterator: optimized contents/traversal checks passed.");
}
