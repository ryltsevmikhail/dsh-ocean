#[path = "support/glib_variant_iter_cases.rs"]
mod cases;

#[test]
fn string_iterator_returns_actual_contents_in_both_directions() {
    cases::run();
}
