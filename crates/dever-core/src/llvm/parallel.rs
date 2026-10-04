//! Parallel dispatch preserves the runtime's bounded producer and task owners.
use super::*;

pub(super) const DECLARATIONS: &str = "\
%dever.parallel_function = type { ptr, ptr, ptr, ptr, ptr }
declare ptr @dever_rt_v1_async_parallel_each(ptr, i32, i64, ptr, ptr, ptr)
declare i32 @dever_rt_v1_parallel_each(ptr, i32, i64, ptr, ptr, ptr, ptr)
";

impl Module<'_> {
    fn parallel_descriptor(&mut self, instance: &Specialization) -> String {
        let name = self.names[instance].clone();
        let descriptor = format!("@dever_parallel_{name}");
        if !self.parallel_functions.insert(instance.clone()) {
            return descriptor;
        }
        let function = &self.program.functions[instance.function];
        let input = asynchronous::input_type(function);
        let Type::Outputs(fields) = &input else {
            unreachable!()
        };
        let context = fields.get(1).map_or("null".into(), |field| {
            format!("@dever_type_{}", self.type_index(&field.ty))
        });
        let mut callback = format!(
            "define internal void @dever_parallel_pack_{name}(ptr %element, ptr %context, ptr %out) {{\nentry:\n"
        );
        for (index, field) in fields.iter().enumerate() {
            let source = if index == 0 { "%element" } else { "%context" };
            writeln!(callback, "  %p{index} = getelementptr {}, ptr %out, i32 0, i32 {index}\n  call void @dever_clone_{}(ptr {source}, ptr %p{index})", self.ty(&input), self.type_index(&field.ty)).unwrap();
        }
        callback.push_str("  ret void\n}\n");
        self.declarations.push_str(&callback);
        let (asynchronous, synchronous) = if specialize::suspends(self.program, instance) {
            (format!("@dever_async_{name}"), "null".into())
        } else {
            ("null".into(), format!("@dever_sync_{name}"))
        };
        writeln!(self.declarations, "{descriptor} = private constant %dever.parallel_function {{ ptr {asynchronous}, ptr {synchronous}, ptr @dever_owned_{}, ptr {context}, ptr @dever_parallel_pack_{name} }}", self.type_index(&input)).unwrap();
        descriptor
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn parallel_each(
        &mut self,
        handler: crate::hir::HandlerTarget,
        sequence: SequenceKind,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        let source = self
            .expression(&arguments[0])
            .expect("checked parallel source");
        let limit = self
            .expression(&arguments[1])
            .expect("checked parallel limit");
        let context = arguments.get(2).map_or_else(
            || "null".into(),
            |argument| {
                let value = self.expression(argument).expect("checked parallel context");
                self.row_pointer(&argument.ty, &value)
            },
        );
        let instance = Specialization {
            function: specialize::resolve_handler(handler, &self.bindings),
            handlers: Vec::new(),
        };
        let descriptor = self.module.parallel_descriptor(&instance);
        let kind = match sequence {
            SequenceKind::List => 0,
            SequenceKind::Bytes => 1,
            SequenceKind::Stream => 2,
            SequenceKind::AsyncStream => 3,
            _ => unreachable!("checked parallel sequence"),
        };
        let arguments = vec![
            format!("ptr {source}"),
            format!("i32 {kind}"),
            format!("i64 {limit}"),
            format!("ptr {descriptor}"),
            format!("ptr {context}"),
        ];
        if self.asynchronous && sequence != SequenceKind::Stream {
            let operation = self.async_operation("async_parallel_each", arguments, result.span);
            self.await_operation(&operation, &Type::Unit, result.span, true)
        } else {
            let error = self.entry_slot_ir("{ ptr, i64 }");
            let status = self.temp();
            self.line(format!(
                "{status} = call i32 @dever_rt_v1_parallel_each({}, ptr %fault, ptr {error})",
                arguments.join(", ")
            ));
            let legacy = self.poll_status(&status, &error, result.span);
            self.propagate_status(&legacy, result.span, true);
            "zeroinitializer".into()
        }
    }
}
