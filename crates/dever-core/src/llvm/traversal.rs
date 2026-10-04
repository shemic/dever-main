use super::*;
use crate::hir::HandlerTarget;

impl FunctionEmitter<'_, '_> {
    pub(super) fn invoke_handler(
        &mut self,
        handler: HandlerTarget,
        arguments: &[(Type, String)],
        span: Span,
    ) -> (String, Type) {
        let target = Specialization {
            function: specialize::resolve_handler(handler, &self.bindings),
            handlers: Vec::new(),
        };
        let output_type = output_type(&self.module.program.functions[target.function].outputs);
        let (status, output) = self.invoke_values(&target, arguments, &output_type, span);
        let failed = self.temp();
        self.line(format!("{failed} = icmp ne i32 {status}, 0"));
        let fault = self.label("handler_fault");
        let okay = self.label("handler_ok");
        self.line(format!("br i1 {failed}, label %{fault}, label %{okay}"));
        self.start(&fault);
        self.append_frame(span);
        self.exit(&status);
        self.start(&okay);
        if output_type == Type::Unit {
            ("zeroinitializer".into(), output_type)
        } else {
            let value = self.temp();
            self.line(format!(
                "{value} = load {}, ptr {output}",
                self.module.ty(&output_type)
            ));
            (value, output_type)
        }
    }

    fn append_output(
        &mut self,
        list_pointer: &str,
        list_guard: usize,
        list_ty: &Type,
        element_ty: &Type,
        element: &str,
        span: Span,
    ) {
        let row = self.row_pointer(element_ty, element);
        let current = self.temp();
        self.line(format!("{current} = load ptr, ptr {list_pointer}"));
        self.disarm(list_guard);
        let appended = self.runtime_call(
            "dever_rt_v1_list_append_take",
            vec![format!("ptr {current}"), format!("ptr {row}")],
            list_ty,
            false,
            None,
            span,
        );
        self.line(format!("store ptr {appended}, ptr {list_pointer}"));
        self.mark_live(list_guard);
    }

    fn add_numeric(&mut self, ty: &Type, left: &str, right: &str, span: Span) -> String {
        match ty {
            Type::Int => self.checked_integer("sadd", left, right, span),
            Type::Float => {
                let value = self.temp();
                self.line(format!("{value} = fadd double {left}, {right}"));
                value
            }
            Type::Decimal => {
                let mut args = self.decimal_parts(left);
                args.extend(self.decimal_parts(right));
                self.runtime_call("dever_rt_v1_decimal_add", args, ty, false, None, span)
            }
            _ => unreachable!("checked numeric Sum"),
        }
    }

    pub(super) fn traverse(
        &mut self,
        operation: CollectionOp,
        handler: Option<HandlerTarget>,
        sequence: SequenceKind,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        let source = self.expression(&arguments[0]).expect("validated sequence");
        let source_guard = self.guards.len() - 1;
        let element_ty = match &arguments[0].ty {
            Type::List(element)
            | Type::Stream(element)
            | Type::AsyncStream(element)
            | Type::RowStream(element) => element.as_ref().clone(),
            Type::Bytes => Type::Int,
            _ => unreachable!("checked sequence"),
        };
        let additional = arguments.get(1).map(|value| {
            self.expression(value)
                .expect("validated traversal argument")
        });
        let state_ty = matches!(operation, CollectionOp::Reduce | CollectionOp::ReduceUntil)
            .then(|| arguments[1].ty.clone());
        let (state_pointer, state_guard) =
            if let (Some(ty), Some(initial)) = (&state_ty, &additional) {
                let guard = contains_owned(self.module.program, ty).then(|| self.guards.len() - 1);
                let pointer = if let Some(guard) = guard {
                    self.guards[guard].pointer.clone()
                } else {
                    let pointer = self.entry_slot(ty);
                    self.line(format!(
                        "store {} {initial}, ptr {pointer}",
                        self.module.ty(ty)
                    ));
                    pointer
                };
                (Some(pointer), guard)
            } else {
                (None, None)
            };
        // Argument evaluation precedes traversal ownership, as for native loops.
        let stream_scope = if matches!(
            sequence,
            SequenceKind::AsyncStream | SequenceKind::RowStream
        ) {
            let pointer = self.row_pointer(&arguments[0].ty, &source);
            let kind = if sequence == SequenceKind::RowStream {
                OwnedKind::DatabaseStreamScope
            } else {
                OwnedKind::AsyncStreamScope
            };
            let guard = self.register_owned(kind, pointer);
            self.mark_live(guard);
            Some(guard)
        } else {
            None
        };

        let (cursor, cursor_guard, length, index_pointer) = match sequence {
            SequenceKind::List => {
                self.disarm(source_guard);
                let cursor = self.runtime_call(
                    "dever_rt_v1_list_cursor_take",
                    vec![format!("ptr {source}")],
                    &Type::Unit,
                    false,
                    Some("ptr"),
                    result.span,
                );
                let pointer = self.entry_slot_ir("ptr");
                self.line(format!("store ptr {cursor}, ptr {pointer}"));
                let guard = self.register_owned(OwnedKind::ListCursor, pointer);
                self.mark_live(guard);
                (Some(cursor), Some(guard), None, None)
            }
            SequenceKind::Bytes => {
                let length = self.runtime_call(
                    "dever_rt_v1_bytes_length",
                    vec![format!("ptr {source}")],
                    &Type::Int,
                    false,
                    None,
                    result.span,
                );
                let index = self.entry_slot(&Type::Int);
                self.line(format!("store i64 0, ptr {index}"));
                (None, None, Some(length), Some(index))
            }
            SequenceKind::Stream | SequenceKind::AsyncStream | SequenceKind::RowStream => {
                (Some(source.clone()), None, None, None)
            }
        };

        let builder = matches!(operation, CollectionOp::Filter)
            || operation == CollectionOp::Each && result.ty != Type::Unit;
        let (list_pointer, list_guard) = if builder {
            let Type::List(element) = &result.ty else {
                unreachable!("checked List result")
            };
            let list = self.runtime_call(
                "dever_rt_v1_list_new",
                vec![
                    format!("ptr @dever_type_{}", self.module.type_index(element)),
                    "ptr null".into(),
                    "i64 0".into(),
                ],
                &result.ty,
                false,
                None,
                result.span,
            );
            let pointer = self.entry_slot(&result.ty);
            self.line(format!("store ptr {list}, ptr {pointer}"));
            let guard = self.register_guard(&result.ty, pointer.clone());
            self.mark_live(guard);
            (Some(pointer), Some(guard))
        } else {
            (None, None)
        };
        let result_pointer = match operation {
            CollectionOp::Find => {
                let pointer = self.entry_slot(&result.ty);
                self.line(format!(
                    "store {} zeroinitializer, ptr {pointer}",
                    self.module.ty(&result.ty)
                ));
                Some(pointer)
            }
            CollectionOp::Sum => {
                let pointer = self.entry_slot(&result.ty);
                let zero = match result.ty {
                    Type::Int => "0".into(),
                    Type::Float => "0.0".into(),
                    Type::Decimal => self.runtime_call(
                        "dever_rt_v1_decimal_from_int",
                        vec!["i64 0".into()],
                        &Type::Decimal,
                        false,
                        None,
                        result.span,
                    ),
                    _ => unreachable!("checked Sum type"),
                };
                self.line(format!(
                    "store {} {zero}, ptr {pointer}",
                    self.module.ty(&result.ty)
                ));
                Some(pointer)
            }
            _ => None,
        };

        let element_pointer = matches!(
            sequence,
            SequenceKind::List
                | SequenceKind::Stream
                | SequenceKind::AsyncStream
                | SequenceKind::RowStream
        )
        .then(|| self.entry_slot(&element_ty));
        let element_guard = element_pointer.as_ref().and_then(|pointer| {
            contains_owned(self.module.program, &element_ty)
                .then(|| self.register_guard(&element_ty, pointer.clone()))
        });
        let loop_label = self.label("sequence_next");
        let body = self.label("sequence_body");
        let done = self.label("sequence_done");
        self.line(format!("br label %{loop_label}"));
        self.start(&loop_label);
        let element = if let Some(cursor) = &cursor {
            let output = element_pointer.as_ref().unwrap();
            let present = if matches!(
                sequence,
                SequenceKind::AsyncStream | SequenceKind::RowStream
            ) {
                let operation = if sequence == SequenceKind::RowStream {
                    self.database_operation(
                        "db_stream_pull",
                        vec![format!("ptr {cursor}")],
                        result.span,
                    )
                } else {
                    let operation = self.temp();
                    self.line(format!(
                        "{operation} = call ptr @dever_rt_v1_async_stream_pull(ptr {cursor})"
                    ));
                    operation
                };
                let (status, row, present, error) = self.poll_raw(&operation, &element_ty);
                let legacy = self.poll_status(&status, &error, result.span);
                self.propagate_status(&legacy, result.span, false);
                // Only the presence branch may load the physical element row.
                let flag = self.temp();
                self.line(format!("{flag} = load i8, ptr {present}"));
                let available = self.temp();
                self.line(format!("{available} = icmp ne i8 {flag}, 0"));
                let copy = self.label("stream_copy_row");
                let copied = self.label("stream_row_ready");
                self.line(format!("br i1 {available}, label %{copy}, label %{copied}"));
                self.start(&copy);
                let value = self.temp();
                self.line(format!(
                    "{value} = load {}, ptr {row}",
                    self.module.ty(&element_ty)
                ));
                self.line(format!(
                    "store {} {value}, ptr {output}",
                    self.module.ty(&element_ty)
                ));
                self.line(format!("br label %{copied}"));
                self.start(&copied);
                present
            } else {
                let present = self.entry_slot_ir("i8");
                let error = self.entry_slot_ir("{ ptr, i64 }");
                let status = self.temp();
                let symbol = if sequence == SequenceKind::Stream {
                    "stream_pull"
                } else {
                    "list_cursor_next"
                };
                self.line(format!("{status} = call i32 @dever_rt_v1_{symbol}(ptr {cursor}, ptr {output}, ptr {present}, ptr {error})"));
                self.check_runtime_status(&status, &error, result.span);
                present
            };
            let flag = self.temp();
            self.line(format!("{flag} = load i8, ptr {present}"));
            let available = self.temp();
            self.line(format!("{available} = icmp ne i8 {flag}, 0"));
            self.line(format!("br i1 {available}, label %{body}, label %{done}"));
            self.start(&body);
            if let Some(guard) = element_guard {
                self.mark_live(guard);
            }
            let value = self.temp();
            self.line(format!(
                "{value} = load {}, ptr {output}",
                self.module.ty(&element_ty)
            ));
            value
        } else {
            let index_pointer = index_pointer.as_ref().unwrap();
            let index = self.temp();
            self.line(format!("{index} = load i64, ptr {index_pointer}"));
            let available = self.temp();
            self.line(format!(
                "{available} = icmp slt i64 {index}, {}",
                length.as_ref().unwrap()
            ));
            self.line(format!("br i1 {available}, label %{body}, label %{done}"));
            self.start(&body);
            let optional = self.runtime_call(
                "dever_rt_v1_bytes_at",
                vec![format!("ptr {source}"), format!("i64 {index}")],
                &Type::Nullable(Box::new(Type::Int)),
                true,
                None,
                result.span,
            );
            let next = self.temp();
            self.line(format!("{next} = add i64 {index}, 1"));
            self.line(format!("store i64 {next}, ptr {index_pointer}"));
            let value = self.temp();
            self.line(format!(
                "{value} = extractvalue {{ i1, i64 }} {optional}, 1"
            ));
            value
        };

        let projected = if let Some(handler) = handler {
            let mut parameters = vec![(element_ty.clone(), element.clone())];
            if let Some(state_ty) = &state_ty {
                let value = self.temp();
                self.line(format!(
                    "{value} = load {}, ptr {}",
                    self.module.ty(state_ty),
                    state_pointer.as_ref().unwrap()
                ));
                parameters.push((state_ty.clone(), value));
            } else if let Some(context) = &additional {
                parameters.push((arguments[1].ty.clone(), context.clone()));
            }
            Some(self.invoke_handler(handler, &parameters, result.span))
        } else {
            None
        };
        match operation {
            CollectionOp::Each => {
                if result.ty != Type::Unit {
                    let (value, ty) = projected.as_ref().unwrap();
                    let guard = contains_owned(self.module.program, ty)
                        .then(|| self.own_value(ty, value, false));
                    self.append_output(
                        list_pointer.as_ref().unwrap(),
                        list_guard.unwrap(),
                        &result.ty,
                        ty,
                        value,
                        result.span,
                    );
                    if let Some(guard) = guard {
                        self.release_guard(guard);
                    }
                }
            }
            CollectionOp::Filter | CollectionOp::Find => {
                let condition = &projected.as_ref().unwrap().0;
                let selected = self.label("sequence_selected");
                let next = self.label("sequence_continue");
                self.line(format!(
                    "br i1 {condition}, label %{selected}, label %{next}"
                ));
                self.start(&selected);
                if operation == CollectionOp::Filter {
                    self.append_output(
                        list_pointer.as_ref().unwrap(),
                        list_guard.unwrap(),
                        &result.ty,
                        &element_ty,
                        &element,
                        result.span,
                    );
                    self.line(format!("br label %{next}"));
                } else {
                    let found = if element_ty == result.ty {
                        element.clone()
                    } else {
                        let first = self.temp();
                        self.line(format!(
                            "{first} = insertvalue {} undef, i1 1, 0",
                            self.module.ty(&result.ty)
                        ));
                        let wrapped = self.temp();
                        self.line(format!(
                            "{wrapped} = insertvalue {} {first}, {} {element}, 1",
                            self.module.ty(&result.ty),
                            self.module.ty(&element_ty)
                        ));
                        wrapped
                    };
                    self.line(format!(
                        "store {} {found}, ptr {}",
                        self.module.ty(&result.ty),
                        result_pointer.as_ref().unwrap()
                    ));
                    if let Some(guard) = element_guard {
                        self.disarm(guard);
                    }
                    self.line(format!("br label %{done}"));
                }
                self.start(&next);
            }
            CollectionOp::Reduce | CollectionOp::ReduceUntil => {
                let (step, step_ty) = projected.as_ref().unwrap();
                let next_state = if operation == CollectionOp::ReduceUntil {
                    let state = self.temp();
                    self.line(format!(
                        "{state} = extractvalue {} {step}, 0",
                        self.module.ty(step_ty)
                    ));
                    state
                } else {
                    step.clone()
                };
                if let Some(guard) = state_guard {
                    self.release_guard(guard);
                }
                self.line(format!(
                    "store {} {next_state}, ptr {}",
                    self.module.ty(state_ty.as_ref().unwrap()),
                    state_pointer.as_ref().unwrap()
                ));
                if let Some(guard) = state_guard {
                    self.mark_live(guard);
                }
                if operation == CollectionOp::ReduceUntil {
                    let stop = self.temp();
                    self.line(format!(
                        "{stop} = extractvalue {} {step}, 1",
                        self.module.ty(step_ty)
                    ));
                    if let Some(guard) = element_guard {
                        self.release_guard(guard);
                    }
                    self.line(format!("br i1 {stop}, label %{done}, label %{loop_label}"));
                }
            }
            CollectionOp::Sum => {
                let value = projected
                    .as_ref()
                    .map_or(&element, |projected| &projected.0);
                let pointer = result_pointer.as_ref().unwrap();
                let current = self.temp();
                self.line(format!(
                    "{current} = load {}, ptr {pointer}",
                    self.module.ty(&result.ty)
                ));
                let sum = self.add_numeric(&result.ty, &current, value, result.span);
                self.line(format!(
                    "store {} {sum}, ptr {pointer}",
                    self.module.ty(&result.ty)
                ));
            }
            _ => unreachable!("preflighted traversal"),
        }
        if operation != CollectionOp::ReduceUntil {
            if let Some(guard) = element_guard {
                self.release_guard(guard);
            }
            self.line(format!("br label %{loop_label}"));
        }
        self.start(&done);
        if let Some(guard) = stream_scope {
            self.release_guard(guard);
        }
        if let Some(guard) = cursor_guard {
            self.release_guard(guard);
        }
        if let (Some(pointer), Some(guard)) = (&list_pointer, list_guard) {
            let output = self.temp();
            self.line(format!("{output} = load ptr, ptr {pointer}"));
            self.disarm(guard);
            return output;
        }
        if let Some(pointer) = &state_pointer {
            let value = self.temp();
            self.line(format!(
                "{value} = load {}, ptr {pointer}",
                self.module.ty(state_ty.as_ref().unwrap())
            ));
            if let Some(guard) = state_guard {
                self.disarm(guard);
            }
            return value;
        }
        if let Some(pointer) = &result_pointer {
            let value = self.temp();
            self.line(format!(
                "{value} = load {}, ptr {pointer}",
                self.module.ty(&result.ty)
            ));
            return value;
        }
        "zeroinitializer".into()
    }
}
