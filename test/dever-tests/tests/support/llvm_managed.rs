use crate::llvm;
use std::process::Output;
use std::time::Duration;

#[path = "llvm_module.rs"]
mod module;

pub fn execute(source: &str, output_type: &str, checks: &str) {
    execute_bounded(source, output_type, checks, 64, Duration::from_secs(5), &[]);
}

/// Expensive protocol/crypto fixtures retain allocation checks with explicit bounds.
pub fn execute_bounded(
    source: &str,
    output_type: &str,
    checks: &str,
    iterations: usize,
    timeout: Duration,
    arguments: &[&str],
) -> Output {
    let mut ir = llvm::lower(source);
    // The emitter supplies the actual nominal and fault layouts.
    ir.push_str(&format!(
        "\ndefine i32 @dever_test_run() {{\nentry:\n  %out = alloca {output_type}\n  %fault = alloca %dever.fault\n  %status = call i32 @dever_entry(ptr %out, ptr %fault)\n{checks}\n  call void @dever_outputs_release(ptr %out)\n  call void @dever_fault_release(ptr %fault)\n  %exit = select i1 %passed, i32 0, i32 1\n  ret i32 %exit\n}}\n"
    ));
    module::execute(&ir, source, iterations, timeout, arguments, None)
}

pub fn boolean(source: &str) {
    boolean_bounded(source, 64, Duration::from_secs(5), &[]);
}

pub fn boolean_bounded(
    source: &str,
    iterations: usize,
    timeout: Duration,
    arguments: &[&str],
) -> Output {
    // Dever equality requires T? on both sides, not implicit T-to-T? coercion.
    let source = format!(
        "maybe_int(value: Int) (answer: Int?) {{ answer = value }}
maybe_text(value: Text) (answer: Text?) {{ answer = value }}
maybe_decimal(value: Decimal) (answer: Decimal?) {{ answer = value }}
maybe_float(value: Float) (answer: Float?) {{ answer = value }}
maybe_list(value: List<Text>) (answer: List<Text>?) {{ answer = value }}
{source}"
    );
    execute_bounded(
        &source,
        "{ i1 }",
        "  %okay = icmp eq i32 %status, 0
  br i1 %okay, label %success, label %failed
success:
  %answer = load i1, ptr %out
  br label %done
failed:
  br label %done
done:
  %passed = phi i1 [ %answer, %success ], [ false, %failed ]",
        iterations,
        timeout,
        arguments,
    )
}

pub fn runtime_fault(source: &str, message: &str) {
    execute(source, "{ i1 }", &module::fault_check(3, Some(message)));
}
