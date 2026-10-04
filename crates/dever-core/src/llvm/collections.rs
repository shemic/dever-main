use super::*;

pub(super) fn supported(operation: CollectionOp, sequence: Option<SequenceKind>) -> bool {
    let direct = matches!(
        operation,
        CollectionOp::Append
            | CollectionOp::First
            | CollectionOp::Get
            | CollectionOp::Put
            | CollectionOp::Remove
            | CollectionOp::Length
            | CollectionOp::Entries
            | CollectionOp::Close
    );
    let traversal = matches!(
        operation,
        CollectionOp::Each
            | CollectionOp::ParallelEach
            | CollectionOp::Filter
            | CollectionOp::Find
            | CollectionOp::Reduce
            | CollectionOp::ReduceUntil
            | CollectionOp::Sum
    ) && matches!(
        sequence,
        Some(
            SequenceKind::List
                | SequenceKind::Bytes
                | SequenceKind::Stream
                | SequenceKind::AsyncStream
                | SequenceKind::RowStream
        )
    );
    direct || traversal
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn row_pointer(&mut self, ty: &Type, value: &str) -> String {
        let pointer = self.entry_slot(ty);
        self.line(format!(
            "store {} {value}, ptr {pointer}",
            self.module.ty(ty)
        ));
        pointer
    }

    pub(super) fn pointer_array(&mut self, pointers: &[String]) -> String {
        if pointers.is_empty() {
            return "null".into();
        }
        let array = self.entry_slot_ir(&format!("[{} x ptr]", pointers.len()));
        for (index, pointer) in pointers.iter().enumerate() {
            let element = self.temp();
            self.line(format!(
                "{element} = getelementptr [{} x ptr], ptr {array}, i32 0, i32 {index}",
                pointers.len()
            ));
            self.line(format!("store ptr {pointer}, ptr {element}"));
        }
        array
    }

    pub(super) fn list_literal(&mut self, values: &[Expression], result: &Expression) -> String {
        let Type::List(element) = &result.ty else {
            unreachable!("checked List literal")
        };
        let mut pointers = Vec::with_capacity(values.len());
        for value in values {
            let row = self.expression(value).expect("validated List element");
            pointers.push(self.row_pointer(element, &row));
        }
        let array = self.pointer_array(&pointers);
        self.runtime_call(
            "dever_rt_v1_list_new",
            vec![
                format!("ptr @dever_type_{}", self.module.type_index(element)),
                format!("ptr {array}"),
                format!("i64 {}", values.len()),
            ],
            &result.ty,
            false,
            None,
            result.span,
        )
    }

    pub(super) fn map_literal(
        &mut self,
        entries: &[(Expression, Expression)],
        result: &Expression,
    ) -> String {
        let Type::Map(key_ty, value_ty) = &result.ty else {
            unreachable!("checked Map literal")
        };
        let mut keys = Vec::with_capacity(entries.len());
        let mut values = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            let key_value = self.expression(key).expect("validated Map key");
            keys.push(self.row_pointer(key_ty, &key_value));
            let mapped = self.expression(value).expect("validated Map value");
            values.push(self.row_pointer(value_ty, &mapped));
        }
        let key_array = self.pointer_array(&keys);
        let value_array = self.pointer_array(&values);
        self.runtime_call(
            "dever_rt_v1_map_new",
            vec![
                format!("ptr @dever_type_{}", self.module.type_index(key_ty)),
                format!("ptr @dever_type_{}", self.module.type_index(value_ty)),
                format!("ptr {key_array}"),
                format!("ptr {value_array}"),
                format!("i64 {}", entries.len()),
            ],
            &result.ty,
            false,
            None,
            result.span,
        )
    }

    pub(super) fn collection(
        &mut self,
        operation: CollectionOp,
        _handler: Option<crate::hir::HandlerTarget>,
        _sequence: Option<SequenceKind>,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        if operation == CollectionOp::ParallelEach {
            return self.parallel_each(
                _handler.expect("checked parallel handler"),
                _sequence.expect("checked parallel sequence"),
                arguments,
                result,
            );
        }
        if matches!(
            operation,
            CollectionOp::Each
                | CollectionOp::Filter
                | CollectionOp::Find
                | CollectionOp::Reduce
                | CollectionOp::ReduceUntil
                | CollectionOp::Sum
        ) {
            return self.traverse(
                operation,
                _handler,
                _sequence.expect("checked sequence"),
                arguments,
                result,
            );
        }
        if operation == CollectionOp::Entries {
            return self.map_entries(&arguments[0], result);
        }
        let receiver = self
            .expression(&arguments[0])
            .expect("validated collection input");
        let receiver_guard =
            contains_owned(self.module.program, &arguments[0].ty).then(|| self.guards.len() - 1);
        let mut values = vec![receiver.clone()];
        for argument in &arguments[1..] {
            values.push(
                self.expression(argument)
                    .expect("validated collection argument"),
            );
        }
        let receiver_ty = &arguments[0].ty;
        if operation == CollectionOp::Close {
            let symbol = if matches!(receiver_ty, Type::AsyncStream(_)) {
                "async_stream_close"
            } else if matches!(receiver_ty, Type::RowStream(_)) {
                "db_stream_close"
            } else {
                "stream_close"
            };
            self.runtime_call(
                &format!("dever_rt_v1_{symbol}"),
                vec![format!("ptr {receiver}")],
                &Type::Unit,
                false,
                Some("i8"),
                result.span,
            );
            return "zeroinitializer".into();
        }
        let mut typed_args = Vec::new();
        if matches!(
            operation,
            CollectionOp::Append
                | CollectionOp::First
                | CollectionOp::Get
                | CollectionOp::Put
                | CollectionOp::Remove
        ) && let Some(guard) = receiver_guard
        {
            self.disarm(guard);
        }
        typed_args.push(format!("ptr {receiver}"));
        let symbol = match operation {
            CollectionOp::Append => {
                typed_args.push(format!(
                    "ptr {}",
                    self.row_pointer(&arguments[1].ty, &values[1])
                ));
                "list_append_take"
            }
            CollectionOp::First => "list_first_take",
            CollectionOp::Get | CollectionOp::Put | CollectionOp::Remove => {
                typed_args.push(format!(
                    "ptr {}",
                    self.row_pointer(&arguments[1].ty, &values[1])
                ));
                if operation == CollectionOp::Put {
                    typed_args.push(format!(
                        "ptr {}",
                        self.row_pointer(&arguments[2].ty, &values[2])
                    ));
                }
                match operation {
                    CollectionOp::Get => "map_get_take",
                    CollectionOp::Put => "map_put_take",
                    CollectionOp::Remove => "map_remove_take",
                    _ => unreachable!(),
                }
            }
            CollectionOp::Length => match receiver_ty {
                Type::Text => "text_length",
                Type::Bytes => "bytes_length",
                Type::List(_) => "list_length",
                Type::Map(_, _) => "map_length",
                _ => unreachable!("checked Length receiver"),
            },
            _ => unreachable!("preflighted collection"),
        };
        let symbol = format!("dever_rt_v1_{symbol}");
        let row_type = match (operation, receiver_ty) {
            (CollectionOp::First, Type::List(element)) => Some(element.as_ref()),
            (CollectionOp::Get, Type::Map(_, value)) => Some(value.as_ref()),
            _ => None,
        };
        if let Some(row_type) = row_type {
            self.runtime_optional_row(&symbol, typed_args, row_type, &result.ty, result.span)
        } else {
            self.runtime_call(&symbol, typed_args, &result.ty, false, None, result.span)
        }
    }

    fn map_entries(&mut self, map: &Expression, result: &Expression) -> String {
        let Type::Map(key_ty, value_ty) = &map.ty else {
            unreachable!("checked entries input")
        };
        let Type::List(entry_ty) = &result.ty else {
            unreachable!("checked entries output")
        };
        let input = self.expression(map).expect("validated Map input");
        self.disarm(self.guards.len() - 1);
        let cursor = self.runtime_call(
            "dever_rt_v1_map_cursor_take",
            vec![format!("ptr {input}")],
            &Type::Unit,
            false,
            Some("ptr"),
            result.span,
        );
        let cursor_pointer = self.entry_slot_ir("ptr");
        self.line(format!("store ptr {cursor}, ptr {cursor_pointer}"));
        let cursor_guard = self.register_owned(OwnedKind::MapCursor, cursor_pointer);
        self.mark_live(cursor_guard);

        let list = self.runtime_call(
            "dever_rt_v1_list_new",
            vec![
                format!("ptr @dever_type_{}", self.module.type_index(entry_ty)),
                "ptr null".into(),
                "i64 0".into(),
            ],
            &result.ty,
            false,
            None,
            result.span,
        );
        let list_pointer = self.entry_slot(&result.ty);
        self.line(format!("store ptr {list}, ptr {list_pointer}"));
        let list_guard = self.register_guard(&result.ty, list_pointer.clone());
        self.mark_live(list_guard);

        let key_pointer = self.entry_slot(key_ty);
        let value_pointer = self.entry_slot(value_ty);
        let key_guard = contains_owned(self.module.program, key_ty)
            .then(|| self.register_guard(key_ty, key_pointer.clone()));
        let value_guard = contains_owned(self.module.program, value_ty)
            .then(|| self.register_guard(value_ty, value_pointer.clone()));
        let present_pointer = self.entry_slot_ir("i8");
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let loop_label = self.label("entries_next");
        let body = self.label("entries_body");
        let done = self.label("entries_done");
        self.line(format!("br label %{loop_label}"));
        self.start(&loop_label);
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_map_cursor_next(ptr {cursor}, ptr {key_pointer}, ptr {value_pointer}, ptr {present_pointer}, ptr {error})"));
        self.check_runtime_status(&status, &error, result.span);
        let present = self.temp();
        self.line(format!("{present} = load i8, ptr {present_pointer}"));
        let has_entry = self.temp();
        self.line(format!("{has_entry} = icmp ne i8 {present}, 0"));
        self.line(format!("br i1 {has_entry}, label %{body}, label %{done}"));
        self.start(&body);
        if let Some(guard) = key_guard {
            self.mark_live(guard);
        }
        if let Some(guard) = value_guard {
            self.mark_live(guard);
        }
        let key = self.temp();
        self.line(format!(
            "{key} = load {}, ptr {key_pointer}",
            self.module.ty(key_ty)
        ));
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {value_pointer}",
            self.module.ty(value_ty)
        ));
        let first = self.temp();
        self.line(format!(
            "{first} = insertvalue {} undef, {} {key}, 0",
            self.module.ty(entry_ty),
            self.module.ty(key_ty)
        ));
        let row = self.temp();
        self.line(format!(
            "{row} = insertvalue {} {first}, {} {value}, 1",
            self.module.ty(entry_ty),
            self.module.ty(value_ty)
        ));
        let row_pointer = self.row_pointer(entry_ty, &row);
        let current = self.temp();
        self.line(format!("{current} = load ptr, ptr {list_pointer}"));
        self.disarm(list_guard);
        let appended = self.runtime_call(
            "dever_rt_v1_list_append_take",
            vec![format!("ptr {current}"), format!("ptr {row_pointer}")],
            &result.ty,
            false,
            None,
            result.span,
        );
        self.line(format!("store ptr {appended}, ptr {list_pointer}"));
        self.mark_live(list_guard);
        if let Some(guard) = value_guard {
            self.release_guard(guard);
        }
        if let Some(guard) = key_guard {
            self.release_guard(guard);
        }
        self.line(format!("br label %{loop_label}"));
        self.start(&done);
        self.release_guard(cursor_guard);
        let output = self.temp();
        self.line(format!("{output} = load ptr, ptr {list_pointer}"));
        self.disarm(list_guard);
        output
    }
}
