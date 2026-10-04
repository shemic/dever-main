use dever_runtime::bytes::Bytes;
use dever_runtime::collections::{List, Map};
use dever_runtime::number::{self, DecimalValue as Decimal};
use dever_runtime::render::Render;
use dever_runtime::resource;
use std::cell::Cell;
use std::rc::Rc;

#[path = "support/temp.rs"]
mod temp;

#[test]
fn decimal128_is_exact_at_literals_and_half_even_at_operations() {
    let parse = |text| Decimal::parse(text).unwrap();
    assert_eq!(
        parse("0.1").checked_add(parse("0.2")).unwrap(),
        parse("0.3")
    );
    assert_eq!(
        parse("1").checked_div(parse("3")).unwrap().to_string(),
        "0.3333333333333333333333333333333333"
    );
    assert_eq!(parse("2.345").round(2).unwrap(), parse("2.34"));
    assert_eq!(parse("2.355").round(2).unwrap(), parse("2.36"));
    assert_eq!(parse("-2.345").round(2).unwrap(), parse("-2.34"));
    assert_eq!(parse("1250").round(-2).unwrap(), parse("1200"));
    assert!(Decimal::parse("0.12345678901234567890123456789012345").is_err());
    assert!(parse("1").checked_div(Decimal::ZERO).is_err());
    assert!(Decimal::maximum().checked_mul(parse("10")).is_err());
    assert_eq!(parse("1").next_up().unwrap().next_down(), Some(parse("1")));
    assert!(Decimal::maximum().next_up().is_none());
    assert!(Decimal::minimum().next_down().is_none());
    assert_eq!(
        Decimal::from_bytes(parse("2.5").to_bytes()).unwrap(),
        parse("2.5")
    );
    assert_eq!(number::int_div(-5, 2).unwrap(), -2);
    assert_eq!(number::int_rem(-5, 2).unwrap(), -1);
    assert!(number::int_rem(i64::MIN, -1).is_err());
}

#[test]
fn ordered_collections_preserve_values_and_nan_equality() {
    let list = List::new(vec![1_i64, 2]);
    let changed = list.clone().append(3);
    assert_eq!(list.values(), &[1, 2]);
    assert_eq!(changed.values(), &[1, 2, 3]);
    let nan = List::new(vec![f64::NAN]);
    assert_ne!(nan, nan.clone());
    let map = Map::new(vec![("a".to_string(), 1_i64), ("b".to_string(), 2)]).unwrap();
    let replaced = map.clone().put("a".into(), 3);
    assert_eq!(replaced.render(), "{a = 3, b = 2}");
    assert_eq!(map.render(), "{a = 1, b = 2}");
    let reordered = map.clone().remove(&"a".into()).put("a".into(), 1);
    assert_ne!(map, reordered);
    assert_eq!(reordered.render(), "{b = 2, a = 1}");
    assert_eq!(
        List::new(vec![map.clone(), replaced]).render(),
        "[{a = 1, b = 2}, {a = 3, b = 2}]"
    );
    assert!(Map::new(vec![(true, 1), (true, 2)]).is_err());
    assert_eq!(map.entries().values()[0].key, "a");
}

#[test]
fn bytes_validate_utf8_indices_and_value_semantics() {
    let original = Bytes::from_text("abc");
    let combined = original.clone().concat(&Bytes::from_text("def"));
    assert_eq!(original.to_text().unwrap(), "abc");
    assert_eq!(combined.slice(1, 4).unwrap().to_text().unwrap(), "bcd");
    assert_eq!(original.at(-1), None);
    assert_eq!(original.at(3), None);
    assert!(original.slice(2, 1).is_err());
    assert!(original.slice(0, 4).is_err());
    assert!(Bytes::from_ints(&[256]).is_err());
    assert!(Bytes::from_ints(&[255]).unwrap().to_text().is_err());
}

#[test]
fn file_handles_close_all_aliases_and_creation_never_overwrites() {
    let directory = temp::TemporaryDirectory::new();
    let path = directory.path().join("sample.txt");
    let path = path.to_str().unwrap();
    let file = resource::file_create(path).unwrap();
    let alias = file.clone();
    resource::write(&file, &Bytes::from_text("hello")).unwrap();
    assert!(resource::file_create(path).is_err());
    file.close().unwrap();
    assert!(alias.close().unwrap_err().contains("closed"));
    assert!(
        resource::write(&alias, &Bytes::from_text("changed"))
            .unwrap_err()
            .contains("closed")
    );
    let reader = resource::file_open(path).unwrap();
    assert!(resource::read(&reader, 0).is_err());
    assert_eq!(
        resource::read(&reader, 64)
            .unwrap()
            .unwrap()
            .to_text()
            .unwrap(),
        "hello"
    );
    assert_eq!(resource::read(&reader, 64).unwrap(), None);
    reader.close().unwrap();
    assert!(resource::read(&reader, 64).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "hello");
}

#[test]
fn streams_share_progress_close_idempotently_and_release_only_their_alias() {
    let directory = temp::TemporaryDirectory::new();
    let path = directory.path().join("stream.txt");
    std::fs::write(&path, "abcdef").unwrap();
    let file = resource::file_open(path.to_str().unwrap()).unwrap();
    let stream = resource::read_chunks(file.clone(), 2).unwrap();
    let alias = stream.clone();
    assert_eq!(stream.pull().unwrap().unwrap().to_text().unwrap(), "ab");
    assert_eq!(alias.pull().unwrap().unwrap().to_text().unwrap(), "cd");
    stream.close();
    stream.close();
    assert!(alias.pull().is_none());
    assert_eq!(
        resource::read(&file, 2)
            .unwrap()
            .unwrap()
            .to_text()
            .unwrap(),
        "ef"
    );
}

#[test]
fn stream_creation_and_terminal_failures_are_explicit_once() {
    let directory = temp::TemporaryDirectory::new();
    let path = directory.path().join("failure.txt");
    std::fs::write(&path, "x").unwrap();
    let file = resource::file_open(path.to_str().unwrap()).unwrap();
    assert!(resource::read_chunks(file.clone(), 0).is_err());
    let stream = resource::read_chunks(file.clone(), 1).unwrap();
    file.close().unwrap();
    assert!(stream.pull().unwrap().is_err());
    assert!(stream.pull().is_none());
    stream.close();
}

#[test]
fn mapped_streams_preserve_terminal_items_and_release_their_producer() {
    struct DropMarker(Rc<Cell<bool>>);

    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    let transform_dropped = Rc::new(Cell::new(false));
    let marker = DropMarker(Rc::clone(&transform_dropped));
    let source = resource::Stream::new(|| resource::Pull::Last(1_i64));
    let mapped = source.map(move |value| {
        let _keep_alive = &marker;
        value + 1
    });

    assert_eq!(mapped.pull(), Some(2));
    assert!(transform_dropped.get());
    assert_eq!(mapped.pull(), None);
}
