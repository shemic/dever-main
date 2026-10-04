mod support;

use dever_runtime::{number, text, time};
use std::process::Command;

#[test]
fn unicode_text_uses_scalar_positions_and_explicit_invalid_ranges() {
    let value = "a中😀e\u{301}";
    assert_eq!(text::codepoint("😀"), Some(0x1f600));
    assert_eq!(text::codepoint("ab"), None);
    assert_eq!(text::from_codepoint(0x1f600).as_deref(), Some("😀"));
    assert_eq!(text::from_codepoint(0xd800), None);
    assert_eq!(text::from_codepoint(i64::MAX), None);
    assert_eq!(text::at(value, 2).as_deref(), Some("😀"));
    assert_eq!(text::at(value, 5), None);
    assert_eq!(text::at(value, -1), None);
    assert_eq!(text::slice(value, 1, 3).as_deref(), Some("中😀"));
    assert_eq!(text::slice(value, 5, 5).as_deref(), Some(""));
    assert_eq!(text::slice(value, 3, 2), None);
    assert_eq!(text::slice(value, 5, 6), None);
    assert_eq!(text::index_of(value, "e"), Some(3));
    assert_eq!(text::index_of(value, "x"), None);
    assert_eq!(text::split("a中", "").values(), &["a", "中"]);
    assert!(text::split("", "").values().is_empty());
    assert_eq!(text::split(",a,,", ",").values(), &["", "a", "", ""]);
}

#[test]
fn numeric_parsing_preserves_precision_boundaries_and_ieee_values() {
    assert_eq!(number::parse_int("-9223372036854775808"), Some(i64::MIN));
    assert_eq!(number::parse_int("9223372036854775808"), None);
    assert_eq!(number::parse_int(" 2"), None);
    assert_eq!(
        number::parse_decimal("0.10000000000000000000000000000000001"),
        None
    );
    assert_eq!(number::parse_decimal("1.25e2").unwrap().to_string(), "125");
    assert_eq!(number::parse_decimal("NaN"), None);
    assert_eq!(number::parse_decimal(" 1.2"), None);
    assert_eq!(
        number::parse_float("-0").unwrap().to_bits(),
        (-0.0f64).to_bits()
    );
    assert!(number::parse_float("NaN").unwrap().is_nan());
    assert_eq!(number::parse_float("-inf"), Some(f64::NEG_INFINITY));
    assert_eq!(number::parse_float("1_000"), None);
}

#[test]
fn official_text_and_number_sources_execute_as_normal_packages() {
    let output = support::stdout(
        r#"public main() (joined: Text, empty: Text, character: Text?, slice: Text?, position: Int?, absent: Text?, pieces: List<Text>, replaced: Text, predicates: Bool, number: Int?, invalid: Decimal?, exact: Text, converted: Text, floating: Text) {
  joined = text.join(["", "中", ""], "|")
  empty = text.join([], "|")
  character = text.at("a😀中", 1)
  slice = text.slice("a😀中", 1, 3)
  position = text.index_of("a😀中", "中")
  absent = text.slice("a", 0, 2)
  pieces = text.split("a,,b", ",")
  replaced = text.replace("中a中", "中", "字")
  predicates = text.starts_with("abc", "a") and text.ends_with("abc", "c") and text.contains("abc", "b") and text.is_empty("")
  number = int.parse("-17")
  invalid = decimal.parse("bad")
  exact = decimal.to_text(1.25)
  converted = int.to_text(-17)
  floating = float.to_text(float.from_int(8))
}
"#,
    );
    assert_eq!(
        output,
        "joined = |中|\nempty = \ncharacter = 😀\nslice = 😀中\nposition = 2\nabsent = null\npieces = [a, , b]\nreplaced = 字a字\npredicates = true\nnumber = -17\ninvalid = null\nexact = 1.25\nconverted = -17\nfloating = 8\n"
    );
}

#[test]
fn process_inputs_are_explicit_and_do_not_require_build_tools() {
    let sources = support::sources(
        r#"public main() (args: dever.process.ArgumentsResult) {
  args = result(dever.process.arguments())
}
"#,
    );
    let program = support::checked(&sources);
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let native = dever_core::native::compile(&program, &sources, "main.main", &compiler).unwrap();
    let output = Command::new(native.executable())
        .env_clear()
        .args(["first", "arg with spaces"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.contains("Read([first, arg with spaces])"),
        "{output}"
    );
}

#[test]
fn clock_failures_are_results() {
    let before = time::monotonic_nanos().unwrap();
    time::sleep(0).unwrap();
    assert!(time::monotonic_nanos().unwrap() >= before);
    assert!(time::unix_millis().unwrap() > 0);
    assert!(time::sleep(-1).is_err());
    assert_eq!(
        support::stdout(
            r#"valid(result: dever.time.ClockResult.Read(value)) (answer: Bool) recover("test observes valid failures in its assertions") { answer = value >= 0 }
valid(result: other) (answer: Bool) recover("test observes valid failures in its assertions") { answer = false }
public main() (wall: Bool, elapsed: Bool, invalid: dever.time.SleepResult) {
  wall = valid(result(dever.time.unix_millis()))
  elapsed = valid(result(dever.time.monotonic_nanos()))
  invalid = result(dever.time.sleep(-1))
}
"#
        ),
        "wall = true\nelapsed = true\ninvalid = dever.time.SleepResult.Failed(sleep duration must not be negative)\n"
    );
}
