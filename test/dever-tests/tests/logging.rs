mod support;

#[test]
fn source_logging_emits_one_structured_record() {
    let output =
        support::run("public main() () { dever.log.info(\"ready\", {\"component\" = \"cms\"}) }");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.contains("\"level\":\"info\""), "{stderr}");
    assert!(stderr.contains("\"message\":\"ready\""), "{stderr}");
    assert!(stderr.contains("\"component\":\"cms\""), "{stderr}");
}
