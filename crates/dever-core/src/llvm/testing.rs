//! One compiled suite with independently checked, namespaced case modules.
use super::*;

pub(super) fn supported(operation: Intrinsic) -> bool {
    matches!(
        operation,
        Intrinsic::TestAssert
            | Intrinsic::TestAssertEq
            | Intrinsic::TestJobDrain
            | Intrinsic::TestClockAdvance
    )
}

/// Emit the same checked suite with one C ABI main selecting exactly one case.
/// The CLI handles zero discovered cases without compiling a suite.
pub fn emit_test_executable(program: &Program, sources: &SourceMap) -> Result<String, String> {
    let mut suite = emit_test_suite(program, sources)?;
    process_entry::emit_test_main(&mut suite);
    Ok(suite)
}

/// Emit one suite; the runner starts its executable once per stable case index.
/// Fault storage is opaque, eight-byte aligned, and sized/released with the same
/// index. The rendered diagnostic is an owned runtime Text (text_release).
pub fn emit_test_suite(program: &Program, sources: &SourceMap) -> Result<String, String> {
    if program.tests.is_empty() {
        return Err("cannot compile an empty test suite".into());
    }
    let mut declarations = BTreeSet::new();
    let mut suite = String::new();
    for index in 0..program.tests.len() {
        let mut case = crate::check::test_program(program, index).map_err(|errors| {
            errors
                .iter()
                .map(|error| error.render(sources))
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        case.api_routes.clear();
        case.api_rest.clear();
        case.api_commands.clear();
        case.auth.clear();
        case.permissions.clear();
        let ir = application::emit_program(&case, sources, Some(index))?;
        for line in ir.lines() {
            if line.starts_with("declare ") && !declarations.insert(line.to_owned()) {
                continue;
            }
            writeln!(suite, "{line}").unwrap();
        }
    }
    writeln!(
        suite,
        "define i64 @dever_test_count() {{\nentry:\n  ret i64 {}\n}}",
        program.tests.len()
    )
    .unwrap();
    for (name, result, parameters, arguments, invalid) in [
        (
            "entry",
            "i32",
            "ptr %out, ptr %fault",
            "ptr %out, ptr %fault",
            "ret i32 2",
        ),
        ("fault_size", "i64", "", "", "ret i64 0"),
        (
            "outputs_release",
            "void",
            "ptr %out",
            "ptr %out",
            "ret void",
        ),
        (
            "fault_release",
            "void",
            "ptr %fault",
            "ptr %fault",
            "ret void",
        ),
        (
            "fault_render",
            "i32",
            "ptr %fault, ptr %out, ptr %error",
            "ptr %fault, ptr %out, ptr %error",
            "ret i32 1",
        ),
    ] {
        let extra = if parameters.is_empty() {
            String::new()
        } else {
            format!(", {parameters}")
        };
        writeln!(suite, "define {result} @dever_test_{name}(i64 %index{extra}) {{\nentry:\n  switch i64 %index, label %invalid [{}]\ninvalid:\n  {invalid}", (0..program.tests.len()).map(|index| format!("i64 {index}, label %case{index}")).collect::<Vec<_>>().join(" ")).unwrap();
        for index in 0..program.tests.len() {
            let target = match name {
                "entry" => "dever_application_entry".to_owned(),
                "outputs_release" => "dever_outputs_release".to_owned(),
                _ => format!("dever_test_{name}"),
            };
            writeln!(suite, "case{index}:").unwrap();
            if result == "void" {
                writeln!(
                    suite,
                    "  call void @case{index}_{target}({arguments})\n  ret void"
                )
                .unwrap();
            } else {
                writeln!(suite, "  %result{index} = call {result} @case{index}_{target}({arguments})\n  ret {result} %result{index}").unwrap();
            }
        }
        suite.push_str("}\n");
    }
    suite.push_str("define void @dever_test_text_release(ptr %text) {\nentry:\n  call void @dever_rt_v1_text_release(ptr %text)\n  ret void\n}\n");
    Ok(suite)
}

impl Module<'_> {
    pub(super) fn emit_test_body(&mut self, instance: &Specialization) -> Result<(), String> {
        let index = self.application.as_ref().unwrap().test.unwrap();
        let function = self.program.functions[instance.function].clone();
        let target = Specialization {
            function: self.program.tests[index].function,
            handlers: Vec::new(),
        };
        let mut emitter = FunctionEmitter::new(self, instance);
        let parameters = emitter.begin_generated_function(&function);
        emitter.initialize_clock(function.span);
        emitter.initialize_database(function.span, "false");
        emitter.initialize_ports(function.span);
        if emitter.module.jobs_enabled() {
            let input = emitter.entry_slot(&Type::Unit);
            emitter.invoke_system(&target, &input, "0", function.span);
        } else {
            let (status, _) = emitter.invoke_values(&target, &[], &Type::Unit, function.span);
            emitter.propagate_status(&status, function.span, false);
        }
        emitter.exit("0");
        let name = emitter.module.names[instance].clone();
        let body = emitter.finish_function(&name, &parameters);
        self.functions.push_str(&body);
        Ok(())
    }

    pub(super) fn emit_test_exports(&mut self, root: &Specialization) {
        let render = self.http_fault_message(root);
        writeln!(self.functions, "define i64 @dever_test_fault_size() {{\nentry:\n  ret i64 ptrtoint (ptr getelementptr (%dever.fault, ptr null, i32 1) to i64)\n}}\ndefine void @dever_test_fault_release(ptr %fault) {{\nentry:\n  call void @dever_fault_release(ptr %fault)\n  ret void\n}}\ndefine i32 @dever_test_fault_render(ptr %fault, ptr %out, ptr %error) {{\nentry:\n  %status = call i32 {render}(ptr %fault, ptr %out, ptr %error)\n  ret i32 %status\n}}").unwrap();
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn test_intrinsic(
        &mut self,
        operation: Intrinsic,
        arguments: &[Expression],
        values: &[String],
        span: Span,
    ) -> String {
        if operation == Intrinsic::TestClockAdvance {
            let clock = self.temp();
            self.line(format!("{clock} = load ptr, ptr @dever_job_clock"));
            self.wire_write_at(
                "dever_rt_v1_job_clock_advance",
                vec![format!("ptr {clock}"), format!("i64 {}", values[0])],
                span,
            );
            return "zeroinitializer".into();
        }
        if operation == Intrinsic::TestJobDrain {
            if self.module.jobs_enabled() {
                let session = self.temp();
                self.line(format!("{session} = load ptr, ptr @dever_job_session"));
                let operation = self.async_operation(
                    "job_drain",
                    vec![format!("ptr {session}"), format!("i64 {}", values[0])],
                    span,
                );
                return self.await_operation(&operation, &Type::Int, span, true);
            }
            let low = self.compare("icmp sge", &Type::Int, &values[0], "1");
            let high = self.compare("icmp sle", &Type::Int, &values[0], "10000");
            let valid = self.reduce_bool("and", vec![low, high], "1");
            self.application_require(
                &valid,
                "test Job drain limit must be between 1 and 10000",
                span,
            );
            return "0".into();
        }
        let valid = if operation == Intrinsic::TestAssert {
            values[0].clone()
        } else {
            self.equals(&arguments[0].ty, &values[0], &values[1])
                .expect("checked comparable assertion")
        };
        let success = self.label("assertion_passed");
        let failure = self.label("assertion_failed");
        self.line(format!("br i1 {valid}, label %{success}, label %{failure}"));
        self.start(&failure);
        if operation == Intrinsic::TestAssert {
            self.application_fault("assertion failed", span);
        } else {
            self.display_literal("assert_eq failed: actual = ", span);
            let owner = self.guards.len() - 1;
            let output = self.guards[owner].pointer.clone();
            self.display_value(&output, &arguments[0].ty, &values[0], span);
            self.display_append_literal(&output, ", expected = ", span);
            self.display_value(&output, &arguments[1].ty, &values[1], span);
            let text = self.temp();
            self.line(format!("{text} = load ptr, ptr {output}"));
            self.application_fault_text(&text, span);
        }
        self.start(&success);
        "zeroinitializer".into()
    }
}

/// Qualify only module-defined symbols. Runtime/LLVM declarations and string
/// constants are not names: treating them as text replacement corrupts payloads.
pub(super) fn namespace_module(ir: &str, index: usize) -> String {
    let mut globals = BTreeSet::new();
    let mut types = BTreeSet::new();
    for line in ir.lines() {
        let line = line.trim_start();
        let (prefix, table) = if line.starts_with('@') || line.starts_with("define ") {
            ('@', &mut globals)
        } else if line.starts_with('%') && line.contains(" = type ") {
            ('%', &mut types)
        } else {
            continue;
        };
        if let Some(start) = line.find(prefix) {
            let name = line[start + 1..]
                .chars()
                .take_while(|character| identifier(*character))
                .collect::<String>();
            table.insert(name);
        }
    }
    let mut output = String::with_capacity(ir.len());
    let mut characters = ir.chars().peekable();
    let mut quoted = false;
    let mut comment = false;
    while let Some(character) = characters.next() {
        if character == '\n' {
            comment = false;
        }
        if !quoted && character == ';' {
            comment = true;
        }
        if !comment && character == '"' {
            quoted = !quoted;
        }
        output.push(character);
        if quoted || comment || !matches!(character, '@' | '%') {
            continue;
        }
        let mut name = String::new();
        while characters
            .peek()
            .is_some_and(|character| identifier(*character))
        {
            name.push(characters.next().unwrap());
        }
        let names = if character == '@' { &globals } else { &types };
        if names.contains(&name) {
            write!(output, "case{index}_").unwrap();
        }
        output.push_str(&name);
    }
    output
}

fn identifier(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '$' | '-')
}
