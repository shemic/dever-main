//! Process glue reuses callable roots, typed fault rendering and their owners.
use super::*;

const DECLARATIONS: &str = "\
declare i32 @dever_rt_v1_process_init()
declare i32 @dever_rt_v1_process_stderr(ptr, i64)
declare i32 @dever_rt_v1_process_error(ptr)
";

impl Module<'_> {
    pub(super) fn emit_process_entry(&mut self, root: &Specialization) {
        let render = self.http_fault_message(root);
        self.declarations.push_str(DECLARATIONS);
        let calls = Calls {
            run: "@dever_application_entry(ptr %out, ptr %fault)".into(),
            render: format!("{render}(ptr %fault, ptr %message, ptr %error)"),
            release_outputs: "@dever_outputs_release(ptr %out)".into(),
            release_fault: "@dever_fault_release(ptr %fault)".into(),
        };
        emit_start(&mut self.functions);
        self.functions.push_str(
            "  %fault = alloca %dever.fault, align 8\n  store %dever.fault zeroinitializer, ptr %fault\n",
        );
        emit_body(&mut self.functions, &calls);
        emit_diagnostic_literals(&mut self.declarations);
    }
}

pub(super) fn emit_test_main(suite: &mut String) {
    suite.push_str(DECLARATIONS);
    suite.push_str("declare i32 @dever_rt_v1_process_test_index(i32, ptr, i64, ptr)\n");
    emit_diagnostic_literals(suite);
    let invalid = literal(
        suite,
        "dever_process_invalid_index",
        "test program requires a valid case index\n",
    );
    emit_start(suite);
    writeln!(
        suite,
        "  %index_slot = alloca i64, align 8
  %count = call i64 @dever_test_count()
  %parsed = call i32 @dever_rt_v1_process_test_index(i32 %argc, ptr %argv, i64 %count, ptr %index_slot)
  %valid = icmp eq i32 %parsed, 0
  br i1 %valid, label %selected, label %invalid
invalid:
  %written = call i32 @dever_rt_v1_process_stderr({invalid})
  call void @dever_rt_v1_log_flush()
  ret i32 1
selected:
  %index = load i64, ptr %index_slot
  %size = call i64 @dever_test_fault_size(i64 %index)
  %fault = alloca i8, i64 %size, align 8"
    )
    .unwrap();
    // The checked case entry initializes its complete concrete fault layout.
    // Size, render and release must all use this same validated stable index.
    let calls = Calls {
        run: "@dever_test_entry(i64 %index, ptr %out, ptr %fault)".into(),
        render: "@dever_test_fault_render(i64 %index, ptr %fault, ptr %message, ptr %error)".into(),
        release_outputs: "@dever_test_outputs_release(i64 %index, ptr %out)".into(),
        release_fault: "@dever_test_fault_release(i64 %index, ptr %fault)".into(),
    };
    emit_body(suite, &calls);
}

struct Calls {
    run: String,
    render: String,
    release_outputs: String,
    release_fault: String,
}

fn emit_start(output: &mut String) {
    writeln!(
        output,
        "define i32 @main(i32 %argc, ptr %argv) {{
entry:
  %initialized = call i32 @dever_rt_v1_process_init()
  %init_ok = icmp eq i32 %initialized, 0
  br i1 %init_ok, label %ready, label %init_failed
init_failed:
  %init_written = call i32 @dever_rt_v1_process_stderr(ptr @dever_process_init_failed, i64 {})
  ret i32 1
ready:",
        INIT_FAILED.len()
    )
    .unwrap();
}

/// Both roots return Unit; diagnostics are the only additional process owners.
/// Every exit after invocation releases the original fault even if rendering
/// fails, so a secondary diagnostic error cannot leak its business payload.
fn emit_body(output: &mut String, calls: &Calls) {
    writeln!(
        output,
        "  %out = alloca i8
  store i8 0, ptr %out
  %message = alloca ptr
  store ptr null, ptr %message
  %error = alloca {{ ptr, i64 }}
  store {{ ptr, i64 }} zeroinitializer, ptr %error
  %status = call i32 {}
  %failed = icmp ne i32 %status, 0
  br i1 %failed, label %report, label %cleanup
report:
  %rendered = call i32 {}
  %render_ok = icmp eq i32 %rendered, 0
  br i1 %render_ok, label %log, label %render_failed
log:
  %text = load ptr, ptr %message
  %logged = call i32 @dever_rt_v1_process_error(ptr %text)
  %log_ok = icmp eq i32 %logged, 0
  br i1 %log_ok, label %cleanup, label %log_failed
log_failed:
  %log_fallback = call i32 @dever_rt_v1_process_stderr(ptr @dever_process_log_failed, i64 {})
  br label %cleanup
render_failed:
  %render_fallback = call i32 @dever_rt_v1_process_stderr(ptr @dever_process_render_failed, i64 {})
  %error_value = load {{ ptr, i64 }}, ptr %error
  %error_bytes = extractvalue {{ ptr, i64 }} %error_value, 0
  %error_length = extractvalue {{ ptr, i64 }} %error_value, 1
  %error_written = call i32 @dever_rt_v1_process_stderr(ptr %error_bytes, i64 %error_length)
  %newline_written = call i32 @dever_rt_v1_process_stderr(ptr @dever_process_newline, i64 1)
  br label %cleanup
cleanup:
  %owned_message = load ptr, ptr %message
  call void @dever_rt_v1_text_release(ptr %owned_message)
  %owned_error = load {{ ptr, i64 }}, ptr %error
  %owned_bytes = extractvalue {{ ptr, i64 }} %owned_error, 0
  %owned_length = extractvalue {{ ptr, i64 }} %owned_error, 1
  call void @dever_rt_v1_buffer_free(ptr %owned_bytes, i64 %owned_length)
  call void {}
  call void {}
  call void @dever_rt_v1_log_flush()
  %exit = zext i1 %failed to i32
  ret i32 %exit
}}",
        calls.run,
        calls.render,
        LOG_FAILED.len(),
        RENDER_FAILED.len(),
        calls.release_outputs,
        calls.release_fault,
    )
    .unwrap();
}

const LOG_FAILED: &str = "cannot log application failure\n";
const INIT_FAILED: &str = "cannot initialize application process\n";
const RENDER_FAILED: &str = "cannot render application failure: ";

fn emit_diagnostic_literals(output: &mut String) {
    for (name, value) in [
        ("dever_process_init_failed", INIT_FAILED),
        ("dever_process_log_failed", LOG_FAILED),
        ("dever_process_render_failed", RENDER_FAILED),
        ("dever_process_newline", "\n"),
    ] {
        literal(output, name, value);
    }
}

fn literal(output: &mut String, name: &str, value: &str) -> String {
    let bytes = value
        .bytes()
        .map(|byte| format!("\\{byte:02X}"))
        .collect::<String>();
    writeln!(
        output,
        "@{name} = private constant [{} x i8] c\"{bytes}\"",
        value.len()
    )
    .unwrap();
    format!("ptr @{name}, i64 {}", value.len())
}
