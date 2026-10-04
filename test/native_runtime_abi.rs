use dever_backend_bridge::{ABI_INVALID_INPUT, ABI_OK, ABI_RUNTIME_ERROR, AbiDecimal};
use dever_runtime::abi;
use dever_runtime::bytes::Bytes;
use dever_runtime::collections::{List, Map};
use dever_runtime::number::{self, DecimalValue};
use dever_runtime::text;

#[test]
fn abi_decimal_words_preserve_finite_values_and_exact_arithmetic() {
    assert_eq!(std::mem::size_of::<AbiDecimal>(), 16);
    for value in [
        DecimalValue::ZERO,
        DecimalValue::from_int(i64::MIN),
        DecimalValue::maximum(),
        DecimalValue::minimum(),
    ] {
        assert_eq!(AbiDecimal::encode(value).decode().unwrap(), value);
    }
    let left = AbiDecimal::encode(DecimalValue::parse("1.25").unwrap());
    let right = AbiDecimal::encode(DecimalValue::parse("0.75").unwrap());
    let sum = left
        .decode()
        .unwrap()
        .checked_add(right.decode().unwrap())
        .unwrap();
    assert_eq!(abi::decimal_to_text(AbiDecimal::encode(sum)).unwrap(), "2");
    assert_eq!(
        DecimalValue::from_int(1).checked_div(DecimalValue::ZERO),
        Err("Decimal division by zero")
    );
}

#[test]
fn abi_facade_preserves_numeric_text_and_error_contracts() {
    assert_eq!((ABI_OK, ABI_RUNTIME_ERROR, ABI_INVALID_INPUT), (0, 1, 2));
    assert_eq!(number::int_add(i64::MAX, 1), Err("Int overflow"));
    assert_eq!(number::int_div(1, 0), Err("Int division by zero"));
    assert_eq!(abi::int_to_text(i64::MIN), i64::MIN.to_string());
    assert_eq!(abi::float_to_text(f64::INFINITY), "inf");
    assert_eq!(abi::float_to_text(f64::NAN), "NaN");
    assert_eq!(
        abi::text_from_utf8("中文".as_bytes()).unwrap(),
        "中文".as_bytes()
    );
    assert!(abi::text_from_utf8(&[0xff]).is_err());
}

#[test]
fn managed_values_reuse_unicode_bytes_and_decimal_semantics() {
    assert_eq!(text::slice("a中b", 1, 2), Some("中".into()));
    assert_eq!(text::codepoint("中"), Some('中' as i64));
    assert_eq!(text::index_of("a中b", "b"), Some(2));
    assert_eq!(text::split("a,,b", ",").values(), &["a", "", "b"]);
    assert_eq!(Bytes::from_ints(&[0, 255]).unwrap().values(), &[0, 255]);
    assert_eq!(
        Bytes::from_ints(&[256]),
        Err("byte value must be between 0 and 255")
    );
    assert_eq!(
        Bytes::new(vec![1, 2, 3]).slice(1, 3).unwrap().values(),
        &[2, 3]
    );
    assert_eq!(number::parse_decimal("1.00"), number::parse_decimal("1.0"),);
}

#[test]
fn managed_collections_preserve_cow_order_and_duplicate_key_errors() {
    let original = List::new(vec![String::from("one")]);
    let shared = original.clone();
    let appended = original.append(String::from("two"));
    assert_eq!(shared.values(), &["one"]);
    assert_eq!(appended.into_values().collect::<Vec<_>>(), ["one", "two"]);

    let original = Map::new(vec![(String::from("a"), 1), (String::from("b"), 2)]).unwrap();
    let shared = original.clone();
    let changed = original
        .put(String::from("a"), 3)
        .remove(&String::from("b"));
    assert_eq!(
        shared
            .into_entries()
            .into_values()
            .map(|entry| (entry.key, entry.value))
            .collect::<Vec<_>>(),
        [("a".into(), 1), ("b".into(), 2)]
    );
    assert_eq!(changed.into_get(&String::from("a")), Some(3));
    assert_eq!(Map::new(vec![(1, 1), (1, 2)]), Err("duplicate Map key"));
}
