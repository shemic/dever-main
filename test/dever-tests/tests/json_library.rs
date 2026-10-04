mod support;

fn source_text(value: &str) -> String {
    format!("{value:?}").replace("\\0", "\\u{0}")
}

const ROUNDTRIP: &str = r#"
roundtrip(source: Text) (result: Text) {
  result = parsed(result(dever.json.parse(source)))
}
parsed(result: dever.json.value.ParseResult.Parsed(document)) (text: Text) recover("test observes parsed failures in its assertions") {
  text = written(result(dever.json.stringify(document)))
}
parsed(result: dever.json.value.ParseResult.Failed(position, message)) (text: Text) recover("test observes parsed failures in its assertions") {
  text = "ERROR " + int.to_text(position) + ": " + message
}
written(result: dever.json.value.WriteResult.Written(value)) (text: Text) recover("test observes written failures in its assertions") { text = value }
written(result: dever.json.value.WriteResult.Failed(node, message)) (text: Text) recover("test observes written failures in its assertions") {
  text = "INVALID " + int.to_text(node) + ": " + message
}
show(source: Text) () { dever.io.println(roundtrip(source)) }
"#;

fn roundtrips(values: &[&str]) -> Vec<String> {
    let literals = values
        .iter()
        .map(|value| source_text(value))
        .collect::<Vec<_>>()
        .join(", ");
    let source = format!("{ROUNDTRIP}\npublic main() () {{each(show, [{literals}])}}\n");
    support::stdout(&source)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn json_sources_pass_the_same_checker_as_application_packages() {
    support::checked(&support::sources("public main() () {}\n"));
}

#[test]
fn json_values_unicode_and_exact_numbers_roundtrip_natively() {
    let cases = [
        ("null", "null"),
        (" \t\r\ntrue\n", "true"),
        ("false", "false"),
        ("-0", "-0"),
        ("1.2300e+40000", "1.2300e+40000"),
        (
            "12345678901234567890123456789012345678901234567890",
            "12345678901234567890123456789012345678901234567890",
        ),
        (
            r#""\"\\\/\b\f\n\r\t\u0000\u001f""#,
            r#""\"\\/\b\f\n\r\t\u0000\u001f""#,
        ),
        (r#""\uD834\uDD1E\ud83d\ude00中文""#, "\"𝄞😀中文\""),
        (
            r#" { "z": [ 1, null, { "empty": [] } ], "a": {}, "": "value" } "#,
            r#"{"z":[1,null,{"empty":[]}],"a":{},"":"value"}"#,
        ),
        (
            "[[],{},true,false,null,0,-12.5E-7]",
            "[[],{},true,false,null,0,-12.5E-7]",
        ),
    ];
    let output = roundtrips(&cases.iter().map(|(source, _)| *source).collect::<Vec<_>>());
    assert_eq!(
        output,
        cases
            .iter()
            .map(|(_, expected)| *expected)
            .collect::<Vec<_>>()
    );
    let second = roundtrips(&output.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(second, output);
}

#[test]
fn malformed_json_is_rejected_without_default_values() {
    let cases = [
        "",
        " ",
        "+1",
        "01",
        "-01",
        "-",
        ".1",
        "1.",
        "1e",
        "1e+",
        "NaN",
        "Infinity",
        "tru",
        "True",
        "nul",
        "true false",
        "nullx",
        "[]x",
        "[",
        "{",
        "[1,]",
        "[,1]",
        "[1 2]",
        "{,}",
        "{a:1}",
        "{\"a\"}",
        "{\"a\":}",
        "{\"a\":1,}",
        "{\"a\":1 \"b\":2}",
        "[}",
        "\"",
        "\"\\",
        "\"line\nbreak\"",
        "\"\0\"",
        r#""\x20""#,
        r#""\u12""#,
        r#""\uXY00""#,
        r#""\ud800""#,
        r#""\udc00""#,
        r#""\ud800x""#,
        r#""\ud800\n""#,
        r#""\ud800\u0041""#,
        r#"{"a":1,"a":2}"#,
        r#"{"a":1,"\u0061":2}"#,
        "\u{feff}null",
        "[1\u{a0},2]",
    ];
    let output = roundtrips(&cases);
    for (source, result) in cases.iter().zip(&output) {
        assert!(
            result.starts_with("ERROR "),
            "{source:?} unexpectedly produced {result:?}"
        );
    }
    assert!(output[output.len() - 4].contains("duplicate JSON object name"));
    assert!(output[output.len() - 3].contains("duplicate JSON object name"));
}

#[test]
fn nesting_limit_and_unicode_error_offsets_are_explicit() {
    let allowed = format!("{}0{}", "[".repeat(256), "]".repeat(256));
    let exceeded = format!("{}0{}", "[".repeat(257), "]".repeat(257));
    let output = roundtrips(&[&allowed, &exceeded, "[\"😀\",]", "{\"a\":"]);
    assert_eq!(output[0], allowed);
    assert_eq!(output[1], "ERROR 256: JSON nesting exceeds 256 containers");
    assert_eq!(output[2], "ERROR 5: expected a JSON value");
    assert_eq!(output[3], "ERROR 5: unterminated JSON container");
}

#[test]
fn constructed_documents_are_validated_and_can_use_any_map_insertion_order() {
    let source = format!(
        r#"{ROUNDTRIP}
show_document(document: dever.json.value.Document) () {{
  dever.io.println(written(result(dever.json.stringify(document))))
}}
public main() () {{
  show_document(dever.json.value.Document {{root = 1
    nodes = {{1 = dever.json.value.Value.Array([0])
      0 = dever.json.value.Value.String("\"\\\n\u{{1}}😀")}}}})
  show_document(dever.json.value.Document {{root = 0
    nodes = {{0 = dever.json.value.Value.Array([0])}}}})
  show_document(dever.json.value.Document {{root = 0
    nodes = {{0 = dever.json.value.Value.Array([-1])}}}})
  show_document(dever.json.value.Document {{root = 0
    nodes = {{0 = dever.json.value.Value.Array([1])
      1 = dever.json.value.Value.Null}}}})
  show_document(dever.json.value.Document {{root = 3
    nodes = {{3 = dever.json.value.Value.Null}}}})
  show_document(dever.json.value.Document {{root = -1
    nodes = {{0 = dever.json.value.Value.Null}}}})
  show_document(dever.json.value.Document {{root = 0
    nodes = {{0 = dever.json.value.Value.Number("01")}}}})
  show_document(dever.json.value.Document {{root = 0
    nodes = {{}}}})
}}
"#
    );
    let output = support::stdout(&source);
    let rows = output.lines().collect::<Vec<_>>();
    assert_eq!(rows[0], r#"["\"\\\n\u0001😀"]"#);
    assert_eq!(
        rows[1],
        "INVALID 0: JSON child references must precede their parent"
    );
    assert_eq!(rows[2], rows[1]);
    assert_eq!(rows[3], rows[1]);
    assert_eq!(
        rows[4],
        "INVALID 0: JSON node IDs must be consecutive from zero"
    );
    assert_eq!(
        rows[5],
        "INVALID -1: JSON root does not refer to a document node"
    );
    assert_eq!(rows[6], "INVALID 0: invalid JSON number spelling");
    assert_eq!(
        rows[7],
        "INVALID 0: JSON root does not refer to a document node"
    );
}

#[test]
fn parsed_document_accessors_preserve_order_and_distinguish_json_null_from_missing() {
    let output = support::stdout(
        r#"inspect(parsed: dever.json.value.ParseResult.Parsed(document)) (result: Bool) recover("test observes inspect failures in its assertions") {
  array = dever.json.property(document, document.root, "values")
  result = inspect_array(array, document)
}
inspect(parsed: other) (result: Bool) recover("test observes inspect failures in its assertions") { result = false }
inspect_array(id: Int, document: dever.json.value.Document) (result: Bool) {
  first = dever.json.element(document, id, 0)
  second = dever.json.element(document, id, 1)
  missing = dever.json.element(document, id, 2)
  negative = dever.json.element(document, id, -1)
  result = equals(first, 0) and equals(second, 1) and missing == null and negative == null and is_null(dever.json.node(document, 0)) and dever.json.node(document, 99) == null and dever.json.property(document, document.root, "missing") == null and dever.json.element(document, document.root, 0) == null
}
inspect_array(id: null, document: dever.json.value.Document) (result: Bool) { result = false }
equals(id: Int, expected: Int) (result: Bool) { result = id == expected }
equals(id: null, expected: Int) (result: Bool) { result = false }
is_null(value: dever.json.value.Value.Null) (result: Bool) { result = true }
is_null(value: null) (result: Bool) { result = false }
is_null(value: other) (result: Bool) { result = false }
public main() (valid: Bool) { valid = inspect(result(dever.json.parse("{\"values\":[null,123]}"))) }
"#,
    );
    assert_eq!(output, "valid = true\n");
}

#[test]
fn constructed_graph_limits_reject_depth_and_exponential_expansion() {
    let nested = std::iter::repeat_n("0", 257).collect::<Vec<_>>().join(",");
    let doubled = std::iter::repeat_n("0", 23).collect::<Vec<_>>().join(",");
    let source = format!(
        r#"{ROUNDTRIP}
type Growth {{document: dever.json.value.Document
  duplicate: Bool}}
seed(duplicate: Bool) (result: Growth) {{
  result = Growth {{document = dever.json.value.Document {{root = 0
    nodes = {{0 = dever.json.value.Value.String("a")}}}}
    duplicate = duplicate}}
}}
grow(unused: Int, state: Growth) (next: Growth) {{
  id = length(state.document.nodes)
  document = state.document
  document.nodes = put(document.nodes, id, dever.json.value.Value.Array(children(state.duplicate, id - 1)))
  document.root = id
  next = state
  next.document = document
}}
children(duplicate: true, previous: Int) (result: List<Int>) {{result = [previous, previous]}}
children(duplicate: false, previous: Int) (result: List<Int>) {{result = [previous]}}
public main() (deep: Text, expanded: Text) {{
  deep = written(result(dever.json.stringify(reduce(grow, [{nested}], seed(false)).document)))
  expanded = written(result(dever.json.stringify(reduce(grow, [{doubled}], seed(true)).document)))
}}
"#
    );
    assert_eq!(
        support::stdout(&source),
        "deep = INVALID 257: JSON nesting exceeds 256 containers\nexpanded = INVALID 22: JSON output exceeds 16777216 Unicode scalars\n"
    );
}
