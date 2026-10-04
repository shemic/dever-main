use dever_backend_bridge::{LinkKind, Target, emit_object, link};
use std::fs;

#[path = "dever-tests/tests/support/llvm.rs"]
mod llvm;
use llvm::lower;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "dever-tests/tests/support/process.rs"]
mod process;
#[path = "dever-tests/tests/support/temp.rs"]
mod temp;

const AGGREGATE_SOURCE: &str = r#"type Pair {
  enabled: Bool
  amount: Float
  count: Int
}
type Payload {
  Value(value: Pair)
  Empty
}
type Envelope {
  Present(value: Payload)
  Missing
}
increment(value: Int) (answer: Int) { answer = value + 1 }
apply(route: handler(value: Int) (answer: Int), value: Int) (answer: Int) { answer = route(value) }
score(value: true) (answer: Int) { answer = 1 }
score(value: false) (answer: Int) { answer = 0 }
read(value: Payload.Value(payload)) (answer: Int) { answer = payload.count + score(payload.enabled and payload.amount == 3.5e0) }
read(value: Payload.Empty) (answer: Int) { answer = 0 }
unwrap(value: Envelope.Present(payload)) (answer: Int) { answer = read(payload) }
unwrap(value: Envelope.Missing) (answer: Int) { answer = -1 }
optional(value: Int) (answer: Int?) { answer = value }
absent() (answer: Int?) { answer = null }
public main() (answer: Int) {
  pair = Pair {
    count = apply(increment, 40)
    enabled = true
    amount = 3.5e0
  }
  first = Payload.Value(pair)
  second = Payload.Value(pair)
  equal = first == second and first != Payload.Empty and optional(1) == optional(1) and absent() == absent() and optional(1) != absent()
  answer = unwrap(Envelope.Present(first)) + score(equal)
}
"#;

#[test]
fn checked_dever_functions_emit_and_link_on_all_six_targets() {
    let source = r#"double(value: Int) (answer: Int) { answer = value * 2 }
public main() (answer: Int) { answer = double(21) }
"#;
    let directory = temp::TemporaryDirectory::new();
    let ir = lower(source);
    for (index, target) in Target::ALL.into_iter().enumerate() {
        let object = directory.path().join(format!("{index}.o"));
        fs::write(&object, emit_object(&ir, target).unwrap()).unwrap();
        let output = directory.path().join(format!("{index}.image"));
        link(target, LinkKind::Shared, &[object], &output, None).unwrap();
        assert!(fs::metadata(output).unwrap().len() > 128);
    }
}

#[test]
fn checked_aggregate_functions_emit_real_objects_on_all_six_targets() {
    let ir = lower(AGGREGATE_SOURCE);
    for target in Target::ALL {
        assert!(emit_object(&ir, target).unwrap().len() > 128);
    }
}

#[test]
fn windows_aggregate_link_requires_target_runtime_support() {
    // LLVM emits the real Windows stack-probe and floating-point CRT references.
    // A signed target runtime pack must provide them; no fixture stubs replace it.
    let directory = temp::TemporaryDirectory::new();
    let object = directory.path().join("aggregate.obj");
    fs::write(
        &object,
        emit_object(&lower(AGGREGATE_SOURCE), Target::WindowsX86_64).unwrap(),
    )
    .unwrap();
    let output = directory.path().join("aggregate.dll");
    let error = link(
        Target::WindowsX86_64,
        LinkKind::Shared,
        &[object],
        &output,
        None,
    )
    .unwrap_err();
    assert!(error.contains("__chkstk"), "{error}");
    assert!(error.contains("_fltused"), "{error}");
    assert!(!output.exists());
}

// The driver is freestanding and test-owned. Foreign targets above are linked,
// not executed; the executable checks below only apply to this native host.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    fn execute(source: &str, output_type: &str, checks: &str) {
        let mut ir = lower(source);
        ir.push_str(&format!(
            "\ndefine void @_start() noreturn {{\nentry:\n  %out = alloca {output_type}\n  %fault = alloca %dever.fault\n  %status = call i32 @dever_entry(ptr %out, ptr %fault)\n{checks}\n  %exit = select i1 %passed, i64 0, i64 1\n  call void asm sideeffect \"syscall\", \"{{rax}},{{rdi}},~{{rcx}},~{{r11}},~{{memory}}\"(i64 60, i64 %exit)\n  unreachable\n}}\n"
        ));
        let directory = temp::TemporaryDirectory::new();
        let object = directory.path().join("entry.o");
        fs::write(&object, emit_object(&ir, Target::LinuxX86_64).unwrap()).unwrap();
        let output = directory.path().join("program");
        link(
            Target::LinuxX86_64,
            LinkKind::Executable,
            &[object],
            &output,
            Some("_start"),
        )
        .unwrap();
        let status = process::status(
            Command::new(output)
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(status.success(), "native kernel failed: {status}\n{source}");
    }

    fn integer(source: &str, expected: i64) {
        execute(
            source,
            "{ i64 }",
            &format!(
                "  %okay = icmp eq i32 %status, 0\n  br i1 %okay, label %success, label %failed\nsuccess:\n  %actual = load i64, ptr %out\n  %correct = icmp eq i64 %actual, {expected}\n  br label %done\nfailed:\n  br label %done\ndone:\n  %passed = phi i1 [ %correct, %success ], [ false, %failed ]"
            ),
        );
    }

    #[test]
    fn clauses_nullable_choices_and_record_values_execute_without_rustc() {
        integer(
            r#"type Selection {
  Found(value: Int)
  Missing
}
type Pair {
  first: Int
  second: Int
}
choose(value: Selection.Found(payload)) (answer: Int) { answer = payload }
choose(value: Selection.Missing) (answer: Int) { answer = -1 }
choose(value: null) (answer: Int) { answer = -2 }
public main() (answer: Int) {
  original = Pair {
    second = 2
    first = 1
  }
  changed = original
  changed.first = 99
  answer = original.first + changed.first + choose(Selection.Found(7)) + choose(Selection.Missing) + choose(null)
}
"#,
            104,
        );
    }

    #[test]
    fn short_circuit_does_not_execute_a_faulting_right_operand() {
        integer(
            r#"truth(value: Bool) (answer: Bool) { answer = value }
probe(left: Int, right: Int) (answer: Bool) { answer = left // right > 0 }
score(value: true) (answer: Int) { answer = 42 }
score(value: false) (answer: Int) { answer = 0 }
public main() (answer: Int) { answer = score(truth(false) and probe(1, 0) or truth(true)) }
"#,
            42,
        );
    }

    #[test]
    fn static_handlers_and_nested_aligned_choice_payloads_execute() {
        integer(AGGREGATE_SOURCE, 43);
    }

    #[test]
    fn multiple_outputs_preserve_order_and_ieee_float_semantics() {
        execute(
            r#"divide(left: Float, right: Float) (answer: Float) { answer = left / right }
public main() (first: Int, second: Float, equal: Bool) {
  first = 7
  second = divide(6.0e0, 2.0e0)
  nan = divide(0.0e0, 0.0e0)
  equal = nan == nan
}
"#,
            "{ i64, double, i1 }",
            "  %okay = icmp eq i32 %status, 0
  br i1 %okay, label %success, label %failed
success:
  %first_ptr = getelementptr { i64, double, i1 }, ptr %out, i32 0, i32 0
  %second_ptr = getelementptr { i64, double, i1 }, ptr %out, i32 0, i32 1
  %equal_ptr = getelementptr { i64, double, i1 }, ptr %out, i32 0, i32 2
  %first = load i64, ptr %first_ptr
  %second = load double, ptr %second_ptr
  %equal = load i1, ptr %equal_ptr
  %first_ok = icmp eq i64 %first, 7
  %second_ok = fcmp oeq double %second, 3.0
  %nan_ok = xor i1 %equal, true
  %values_ok = and i1 %first_ok, %second_ok
  %correct = and i1 %values_ok, %nan_ok
  br label %done
failed:
  br label %done
done:
  %passed = phi i1 [ %correct, %success ], [ false, %failed ]",
        );
    }

    #[test]
    fn integer_overflow_and_division_faults_return_checked_statuses() {
        for operation in [
            "left + right",
            "left - right",
            "left * right",
            "left // right",
            "left % right",
        ] {
            let (left, right, fault) = match operation {
                "left + right" => (i64::MAX, 1, 1),
                "left - right" => (i64::MIN, 1, 1),
                "left * right" => (i64::MAX, 2, 1),
                _ => (1, 0, 2),
            };
            let source = format!(
                "calculate(left: Int, right: Int) (answer: Int) {{ answer = {operation} }}\npublic main() (answer: Int) {{ answer = calculate({left}, {right}) }}"
            );
            execute(
                &source,
                "{ i64 }",
                &format!(
                    "  %failed = icmp eq i32 %status, 1\n  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0\n  %code = load i32, ptr %code_ptr\n  %correct = icmp eq i32 %code, {fault}\n  %passed = and i1 %failed, %correct"
                ),
            );
        }
        for (operation, parameters, arguments) in [
            ("-left", "left: Int", i64::MIN.to_string()),
            (
                "left // right",
                "left: Int, right: Int",
                format!("{}, -1", i64::MIN),
            ),
            (
                "left % right",
                "left: Int, right: Int",
                format!("{}, -1", i64::MIN),
            ),
        ] {
            let source = format!(
                "calculate({parameters}) (answer: Int) {{ answer = {operation} }}\npublic main() (answer: Int) {{ answer = calculate({arguments}) }}"
            );
            execute(
                &source,
                "{ i64 }",
                "  %failed = icmp eq i32 %status, 1\n  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0\n  %code = load i32, ptr %code_ptr\n  %correct = icmp eq i32 %code, 1\n  %passed = and i1 %failed, %correct",
            );
        }
    }

    #[test]
    fn numeric_faults_retain_origin_and_every_caller_source_span() {
        let source = "divide(left: Int, right: Int) (answer: Int) { answer = left // right }\nwrap(left: Int, right: Int) (answer: Int) { answer = divide(left, right) }\npublic main() (answer: Int) { answer = wrap(1, 0) }";
        let origin = source.find("left // right").unwrap();
        let inner_call = source.find("divide(left, right)").unwrap();
        let outer_call = source.find("wrap(1, 0)").unwrap();
        execute(source, "{ i64 }", &format!(
            "  %failed = icmp eq i32 %status, 1
  %origin_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 1
  %origin_index = load i32, ptr %origin_ptr
  %origin_span = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %origin_index, i32 1
  %origin = load i64, ptr %origin_span
  %origin_ok = icmp eq i64 %origin, {origin}
  %depth_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 2
  %depth = load i32, ptr %depth_ptr
  %depth_ok = icmp eq i32 %depth, 2
  %frame0_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 3, i32 0
  %frame1_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 3, i32 1
  %frame0 = load i32, ptr %frame0_ptr
  %frame1 = load i32, ptr %frame1_ptr
  %span0 = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %frame0, i32 1
  %span1 = getelementptr {{ i64, i64, i64 }}, ptr @dever_source_spans, i32 %frame1, i32 1
  %start0 = load i64, ptr %span0
  %start1 = load i64, ptr %span1
  %inner_ok = icmp eq i64 %start0, {inner_call}
  %outer_ok = icmp eq i64 %start1, {outer_call}
  %chain_ok = and i1 %inner_ok, %outer_ok
  %location_ok = and i1 %origin_ok, %depth_ok
  %fault_ok = and i1 %failed, %location_ok
  %passed = and i1 %fault_ok, %chain_ok"
        ));
    }
}
