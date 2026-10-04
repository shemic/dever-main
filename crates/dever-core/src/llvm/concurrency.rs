//! Checked Task/Group/Channel and blocking boundaries share coroutine polling.
use super::*;

impl FunctionEmitter<'_, '_> {
    pub(super) fn task_selection(
        &mut self,
        operation: Intrinsic,
        handler: Option<crate::hir::HandlerTarget>,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        let mut values = Vec::with_capacity(arguments.len());
        let mut task_guards = Vec::new();
        for argument in arguments {
            values.push(
                self.expression(argument)
                    .expect("checked task selection argument"),
            );
            if matches!(argument.ty, Type::Task(_)) {
                task_guards.push(self.guards.len() - 1);
            }
        }
        if operation == Intrinsic::TaskRace {
            let tasks = self.pointer_array(&values);
            let operation = self.async_operation(
                "async_task_race",
                vec![format!("ptr {tasks}"), format!("i64 {}", values.len())],
                result.span,
            );
            // A rejected race leaves every wrapper caller-owned.
            for guard in task_guards {
                self.disarm(guard);
            }
            return self.await_operation(&operation, &result.ty, result.span, true);
        }
        // Argument evaluation may fail; transfer only after every argument exists.
        for guard in task_guards {
            self.disarm(guard);
        }
        let operation = self.temp();
        self.line(format!(
            "{operation} = call ptr @dever_rt_v1_async_task_wait_timeout(ptr {}, i64 {})",
            values[0], values[1]
        ));
        let (status, output, present, error) = self.poll_raw(&operation, &result.ty);
        let legacy = self.poll_status(&status, &error, result.span);
        self.propagate_status(&legacy, result.span, true);
        let flag = self.temp();
        self.line(format!("{flag} = load i8, ptr {present}"));
        let completed = self.temp();
        self.line(format!("{completed} = icmp ne i8 {flag}, 0"));
        let fallback = self.label("timeout_fallback");
        let done = self.label("timeout_done");
        self.line(format!(
            "br i1 {completed}, label %{done}, label %{fallback}"
        ));
        self.start(&fallback);
        let (value, ty) =
            self.invoke_handler(handler.expect("checked timeout fallback"), &[], result.span);
        self.line(format!(
            "store {} {value}, ptr {output}",
            self.module.ty(&ty)
        ));
        self.line(format!("br label %{done}"));
        self.start(&done);
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {output}",
            self.module.ty(&result.ty)
        ));
        value
    }

    fn concurrency_arguments(
        &mut self,
        arguments: &[CallArgument],
    ) -> Result<Vec<(Type, String)>, String> {
        arguments
            .iter()
            .filter_map(|argument| match argument {
                CallArgument::Value(value) => Some(
                    self.expression(value)
                        .map(|result| (value.ty.clone(), result)),
                ),
                CallArgument::Handler(_) => None,
            })
            .collect()
    }

    pub(super) fn concurrency(&mut self, expression: &Expression) -> Result<String, String> {
        let span = expression.span;
        let result = match &expression.kind {
            ExpressionKind::RunCall {
                group,
                target,
                arguments,
            } => {
                let group = group
                    .as_ref()
                    .map(|group| self.expression(group))
                    .transpose()?;
                let instance = self.call_instance(*target, arguments);
                let asynchronous = specialize::suspends(self.module.program, &instance);
                let values = self.concurrency_arguments(arguments)?;
                let (input, guard) = self.pack_input(&instance, &values, !asynchronous);
                let mut args = Vec::new();
                if let Some(group) = &group {
                    args.push(format!("ptr {group}"));
                }
                let descriptor = if asynchronous { "async" } else { "sync" };
                args.extend([
                    format!("ptr @dever_{descriptor}_{}", self.module.names[&instance]),
                    format!("ptr {input}"),
                ]);
                let symbol = match (group.is_some(), asynchronous) {
                    (false, true) => "async_task_run",
                    (false, false) => "async_task_run_sync",
                    (true, true) => "async_group_run",
                    (true, false) => "async_group_run_sync",
                };
                let operation = self.async_operation(symbol, args, span);
                if let Some(guard) = guard {
                    self.disarm(guard);
                }
                self.await_operation(&operation, &expression.ty, span, group.is_some())
            }
            ExpressionKind::ParallelCall { target, arguments }
            | ExpressionKind::BlockingCall { target, arguments } => {
                let instance = self.call_instance(*target, arguments);
                let values = self.concurrency_arguments(arguments)?;
                let symbol = if matches!(expression.kind, ExpressionKind::ParallelCall { .. }) {
                    "async_parallel"
                } else {
                    "async_blocking"
                };
                self.invoke_sync_boundary(&instance, &values, symbol, &expression.ty, span)
            }
            ExpressionKind::AwaitTask(value)
            | ExpressionKind::StopTask(value)
            | ExpressionKind::AwaitGroup(value)
            | ExpressionKind::StopGroup(value) => {
                let handle = self.expression(value)?;
                // The ABI consumes this one wrapper even when polling fails.
                self.disarm(self.guards.len() - 1);
                let symbol = match expression.kind {
                    ExpressionKind::AwaitTask(_) => "async_task_wait",
                    ExpressionKind::StopTask(_) => "async_task_stop",
                    ExpressionKind::AwaitGroup(_) => "async_group_wait",
                    _ => "async_group_stop",
                };
                let operation = self.temp();
                self.line(format!(
                    "{operation} = call ptr @dever_rt_v1_{symbol}(ptr {handle})"
                ));
                self.await_operation(&operation, &expression.ty, span, true)
            }
            ExpressionKind::Group(limit) => {
                let limit = self.expression(limit)?;
                self.runtime_call(
                    "dever_rt_v1_async_group_new",
                    vec![format!("i64 {limit}")],
                    &Type::Group,
                    false,
                    None,
                    span,
                )
            }
            ExpressionKind::Channel { capacity, .. } => {
                let capacity = self.expression(capacity)?;
                self.runtime_call(
                    "dever_rt_v1_async_channel_new",
                    vec![format!("i64 {capacity}")],
                    &expression.ty,
                    false,
                    None,
                    span,
                )
            }
            ExpressionKind::ChannelSend { channel, value } => {
                let channel = self.expression(channel)?;
                let value_ir = self.expression(value)?;
                let guard = self.own_value(
                    &value.ty,
                    &value_ir,
                    contains_owned(self.module.program, &value.ty),
                );
                let input = self.guards[guard].pointer.clone();
                let operation = self.async_operation(
                    "async_channel_send",
                    vec![
                        format!("ptr {channel}"),
                        format!("ptr @dever_owned_{}", self.module.type_index(&value.ty)),
                        format!("ptr {input}"),
                    ],
                    span,
                );
                self.disarm(guard);
                self.await_operation(&operation, &Type::Unit, span, false)
            }
            ExpressionKind::ChannelReceive(channel) => {
                let Type::Channel(element) = &channel.ty else {
                    unreachable!("checked receive")
                };
                let handle = self.expression(channel)?;
                let operation = self.temp();
                self.line(format!(
                    "{operation} = call ptr @dever_rt_v1_async_channel_receive(ptr {handle})"
                ));
                let (status, row, present, error) = self.poll_raw(&operation, element);
                let legacy = self.poll_status(&status, &error, span);
                self.propagate_status(&legacy, span, false);
                self.optional_row(&expression.ty, element, &row, &present)
            }
            ExpressionKind::ChannelClose(channel) => {
                let channel = self.expression(channel)?;
                let operation = self.temp();
                self.line(format!(
                    "{operation} = call ptr @dever_rt_v1_async_channel_close(ptr {channel})"
                ));
                self.await_operation(&operation, &Type::Unit, span, false)
            }
            _ => unreachable!("checked concurrency expression"),
        };
        Ok(result)
    }

    pub(super) fn task_sleep(&mut self, duration: &str, expression: &Expression) -> String {
        let operation = self.temp();
        self.line(format!(
            "{operation} = call ptr @dever_rt_v1_async_sleep(i64 {duration})"
        ));
        let (status, _, _, error) = self.poll_raw(&operation, &Type::Unit);
        let Type::Named(id) = expression.ty else {
            unreachable!("checked sleep result")
        };
        let Shape::Choice(variants) = &self.module.program.types[id].shape else {
            unreachable!()
        };
        let done_tag = variants
            .iter()
            .position(|variant| variant.name == "Done")
            .expect("sleep Done");
        let failed_tag = variants
            .iter()
            .position(|variant| variant.name == "Failed")
            .expect("sleep Failed");
        let slot = self.entry_slot(&expression.ty);
        let done = self.label("sleep_done");
        let failed = self.label("sleep_failed");
        let invalid = self.label("sleep_invalid");
        let result = self.label("sleep_result");
        self.line(format!(
            "switch i32 {status}, label %{invalid} [i32 1, label %{done} i32 3, label %{failed}]"
        ));
        self.start(&done);
        self.write_variant(id, done_tag, &[], &slot);
        self.line(format!("br label %{result}"));
        self.start(&failed);
        let message = self.error_text(&error, expression.span);
        self.write_variant(id, failed_tag, &[(Type::Text, message)], &slot);
        self.line(format!("br label %{result}"));
        self.start(&invalid);
        let legacy = self.poll_status(&status, &error, expression.span);
        self.propagate_status(&legacy, expression.span, false);
        self.line("unreachable");
        self.start(&result);
        let value = self.temp();
        self.line(format!("{value} = load %T{id}, ptr {slot}"));
        value
    }

    pub(super) fn invoke_sync_boundary(
        &mut self,
        instance: &Specialization,
        values: &[(Type, String)],
        symbol: &str,
        output: &Type,
        span: Span,
    ) -> String {
        let (input, guard) = self.pack_input(instance, values, true);
        let operation = self.async_operation(
            symbol,
            vec![
                format!("ptr @dever_sync_{}", self.module.names[instance]),
                format!("ptr {input}"),
            ],
            span,
        );
        self.disarm(guard.expect("owned worker input"));
        self.await_operation(&operation, output, span, true)
    }
}
