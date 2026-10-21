//! Shared headless assertions: used by the normal test and the real release
//! profile example (including opt-level=s, LTO and panic=abort). No GTK init.
use gtk::glib::{variant::ToVariant, Variant};

pub fn run() {
    let expected = ["zero", "", "雪", "three", "four", "five"];
    let variant = expected.to_variant();
    assert_eq!(variant.type_().as_str(), "as");
    assert_eq!(
        variant.array_iter_str().unwrap().collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        variant.array_iter_str().unwrap().rev().collect::<Vec<_>>(),
        expected.iter().copied().rev().collect::<Vec<_>>()
    );

    let mut iter = variant.array_iter_str().unwrap();
    assert_eq!(iter.size_hint(), (6, Some(6)));
    assert_eq!(iter.nth(2), Some("雪"));
    assert_eq!(iter.next(), Some("three"));
    assert_eq!(iter.len(), 2);
    assert_eq!(iter.size_hint(), (2, Some(2)));
    assert_eq!(iter.last(), Some("five"));
    assert_eq!(variant.array_iter_str().unwrap().last(), Some("five"));
    let mut iter = variant.array_iter_str().unwrap();
    assert_eq!(iter.next_back(), Some("five"));
    assert_eq!(iter.last(), Some("four"));

    let mut iter = variant.array_iter_str().unwrap();
    assert_eq!(iter.next_back(), Some("five"));
    assert_eq!(iter.nth_back(1), Some("three"));
    assert_eq!(iter.next_back(), Some("雪"));
    assert_eq!(iter.len(), 2);
    assert_eq!(iter.next(), Some("zero"));
    assert_eq!(iter.next(), Some(""));
    assert_eq!(iter.next_back(), None);

    let mut iter = variant.array_iter_str().unwrap();
    assert_eq!(iter.next(), Some("zero"));
    assert_eq!(iter.next_back(), Some("five"));
    assert_eq!(iter.nth(1), Some("雪"));
    assert_eq!(iter.nth_back(1), Some("three"));
    assert_eq!(iter.len(), 0);
    assert_eq!(iter.size_hint(), (0, Some(0)));
    for _ in 0..3 {
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }
    let borrowed = {
        let mut iter = variant.array_iter_str().unwrap();
        iter.next().unwrap()
    };
    assert_eq!(borrowed, "zero"); // Borrow is tied to Variant, not iterator storage.

    for n in [expected.len(), usize::MAX] {
        let mut iter = variant.array_iter_str().unwrap();
        assert_eq!(iter.nth(n), None);
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
        let mut iter = variant.array_iter_str().unwrap();
        assert_eq!(iter.nth_back(n), None);
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }
    let empty = Vec::<String>::new().to_variant();
    let mut iter = empty.array_iter_str().unwrap();
    assert_eq!(iter.len(), 0);
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);
    assert_eq!(iter.last(), None);
    let one = ["only"].to_variant();
    assert_eq!(one.array_iter_str().unwrap().next(), Some("only"));
    assert_eq!(one.array_iter_str().unwrap().next_back(), Some("only"));

    for text in [
        "'scalar'",
        "42",
        "('a', 'b')",
        "@ai [1, 2]",
        "@av [<'a'>, <'b'>]",
        "@ao ['/a']",
        "@ag ['s']",
        "<['a', 'b']>",
    ] {
        let other = Variant::parse(None, text).unwrap();
        assert!(
            other.array_iter_str().is_err(),
            "non-as variant accepted: {}",
            other.type_()
        );
    }
}
