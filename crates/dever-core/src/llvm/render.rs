//! Typed display only at terminal request diagnostics; faults stay owned and typed.
use super::*;

impl Module<'_> {
    fn related_renderer(&mut self, inner: &Type, instance: &Specialization, span: Span) -> String {
        let index = self.type_index(inner);
        let name = format!("dever_render_related_{index}");
        if !self.related_renderers.insert(index) {
            return name;
        }
        let mut emitter = FunctionEmitter::new(self, instance);
        emitter.asynchronous = false;
        emitter.start("entry");
        emitter.display_literal("Related.Loaded(", span);
        let owner = emitter.guards.len() - 1;
        let output = emitter.guards[owner].pointer.clone();
        let value = emitter.runtime_call(
            "dever_rt_v1_db_related_get",
            vec!["ptr %input".into()],
            inner,
            false,
            None,
            span,
        );
        emitter.own_value(inner, &value, false);
        emitter.display_value(&output, inner, &value, span);
        emitter.display_append_literal(&output, ")", span);
        let result = emitter.temp();
        emitter.line(format!("{result} = load ptr, ptr {output}"));
        emitter.line(format!("store ptr {result}, ptr %out"));
        emitter.disarm(owner);
        let body = emitter.finish_protocol_callback(&name, "ptr %input, ptr %out, ptr %error");
        self.declarations.push_str(&body);
        name
    }
    pub(super) fn emit_location_renderer(&mut self) {
        let mut entries = Vec::new();
        for span in self.locations.clone() {
            let source = self.sources.get(span.source);
            let (line, column) = source.position(span.start);
            let (literal, length) =
                self.text_literal(&format!("{}:{line}:{column}: ", source.path().display()));
            entries.push(format!("{{ ptr, i64 }} {{ ptr {literal}, i64 {length} }}"));
        }
        let length = entries.len();
        writeln!(
            self.declarations,
            "@dever_location_strings = private constant [{length} x {{ ptr, i64 }}] [{}]",
            entries.join(", ")
        )
        .unwrap();
        writeln!(self.declarations, "define internal i32 @dever_location_text(i32 %index, ptr %out, ptr %error) {{\nentry:\n  %slot = getelementptr [{length} x {{ ptr, i64 }}], ptr @dever_location_strings, i32 0, i32 %index\n  %value = load {{ ptr, i64 }}, ptr %slot\n  %bytes = extractvalue {{ ptr, i64 }} %value, 0\n  %length = extractvalue {{ ptr, i64 }} %value, 1\n  %status = call i32 @dever_rt_v1_text_new(ptr %bytes, i64 %length, ptr %out, ptr %error)\n  ret i32 %status\n}}").unwrap();
    }

    pub(super) fn http_fault_message(&mut self, instance: &Specialization) -> String {
        let name = format!("dever_http_fault_{}", self.names[instance]);
        let span = self.program.functions[instance.function].span;
        let failures = specialize::failures(self.program, instance);
        let mut emitter = FunctionEmitter::new(self, instance);
        emitter.asynchronous = false;
        emitter.start("entry");
        emitter.display_literal("", span);
        let guard = emitter.guards.len() - 1;
        let output = emitter.guards[guard].pointer.clone();
        let depth_ptr = emitter.temp();
        emitter.line(format!(
            "{depth_ptr} = getelementptr %dever.fault, ptr %input, i32 0, i32 2"
        ));
        let depth = emitter.temp();
        emitter.line(format!("{depth} = load i32, ptr {depth_ptr}"));
        let index = emitter.entry_slot_ir("i32");
        emitter.line(format!("store i32 {depth}, ptr {index}"));
        let next = emitter.label("trace_next");
        let frame = emitter.label("trace_frame");
        let origin = emitter.label("trace_origin");
        emitter.line(format!("br label %{next}"));
        emitter.start(&next);
        let position = emitter.temp();
        emitter.line(format!("{position} = load i32, ptr {index}"));
        let has_frame = emitter.temp();
        emitter.line(format!("{has_frame} = icmp ne i32 {position}, 0"));
        emitter.line(format!(
            "br i1 {has_frame}, label %{frame}, label %{origin}"
        ));
        emitter.start(&frame);
        let previous = emitter.temp();
        emitter.line(format!("{previous} = sub i32 {position}, 1"));
        emitter.line(format!("store i32 {previous}, ptr {index}"));
        let pointer = emitter.temp();
        emitter.line(format!(
            "{pointer} = getelementptr %dever.fault, ptr %input, i32 0, i32 3, i32 {previous}"
        ));
        emitter.display_location(&output, &pointer, span);
        emitter.line(format!("br label %{next}"));
        emitter.start(&origin);
        let pointer = emitter.temp();
        emitter.line(format!(
            "{pointer} = getelementptr %dever.fault, ptr %input, i32 0, i32 1"
        ));
        emitter.display_location(&output, &pointer, span);
        let kind_ptr = emitter.temp();
        emitter.line(format!(
            "{kind_ptr} = getelementptr %dever.fault, ptr %input, i32 0, i32 0"
        ));
        let kind = emitter.temp();
        emitter.line(format!("{kind} = load i32, ptr {kind_ptr}"));
        let done = emitter.label("message_ready");
        emitter.line("switch i32 ".to_owned() + &kind + ", label %invalid [i32 1, label %overflow i32 2, label %division i32 3, label %runtime i32 4, label %business]");
        emitter.start("invalid");
        emitter.line("unreachable");
        for (label, text) in [
            ("overflow", "Int overflow"),
            ("division", "Int division by zero"),
        ] {
            emitter.start(label);
            emitter.display_append_literal(&output, text, span);
            emitter.line(format!("br label %{done}"));
        }
        emitter.start("runtime");
        let message_ptr = emitter.temp();
        emitter.line(format!(
            "{message_ptr} = getelementptr %dever.fault, ptr %input, i32 0, i32 4"
        ));
        let message = emitter.temp();
        emitter.line(format!(
            "{message} = load {{ ptr, i64 }}, ptr {message_ptr}"
        ));
        let bytes = emitter.temp();
        emitter.line(format!(
            "{bytes} = extractvalue {{ ptr, i64 }} {message}, 0"
        ));
        let length = emitter.temp();
        emitter.line(format!(
            "{length} = extractvalue {{ ptr, i64 }} {message}, 1"
        ));
        let first = emitter.guards.len();
        let text = emitter.runtime_call(
            "dever_rt_v1_text_new",
            vec![format!("ptr {bytes}"), format!("i64 {length}")],
            &Type::Text,
            false,
            None,
            span,
        );
        emitter.own_value(&Type::Text, &text, false);
        emitter.display_append(&output, &text, span);
        emitter.release_from(first);
        emitter.line(format!("br label %{done}"));
        emitter.start("business");
        let type_ptr = emitter.temp();
        emitter.line(format!(
            "{type_ptr} = getelementptr %dever.fault, ptr %input, i32 0, i32 5"
        ));
        let type_id = emitter.temp();
        emitter.line(format!("{type_id} = load i32, ptr {type_ptr}"));
        let types = failures
            .iter()
            .map(|failure| failure.ty)
            .collect::<BTreeSet<_>>();
        emitter.line(format!(
            "switch i32 {type_id}, label %invalid [{}]",
            types
                .iter()
                .map(|id| format!("i32 {}, label %business{id}", id + 1))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        for id in types {
            emitter.start(&format!("business{id}"));
            let payload_ptr = emitter.temp();
            emitter.line(format!(
                "{payload_ptr} = getelementptr %dever.fault, ptr %input, i32 0, i32 7"
            ));
            let payload = emitter.temp();
            emitter.line(format!("{payload} = load %T{id}, ptr {payload_ptr}"));
            let variants = failures
                .iter()
                .filter(|failure| failure.ty == id)
                .map(|failure| failure.variant)
                .collect::<BTreeSet<_>>();
            emitter.display_named(&output, id, &payload, span, Some(&variants));
            emitter.line(format!("br label %{done}"));
        }
        emitter.start(&done);
        let cause_ptr = emitter.temp();
        emitter.line(format!(
            "{cause_ptr} = getelementptr %dever.fault, ptr %input, i32 0, i32 8"
        ));
        let cause = emitter.temp();
        emitter.line(format!("{cause} = load ptr, ptr {cause_ptr}"));
        let present = emitter.temp();
        emitter.line(format!("{present} = icmp ne ptr {cause}, null"));
        let append = emitter.label("message_cause");
        let complete = emitter.label("message_complete");
        emitter.line(format!(
            "br i1 {present}, label %{append}, label %{complete}"
        ));
        emitter.start(&append);
        emitter.display_append_literal(&output, "; ", span);
        emitter.display_append(&output, &cause, span);
        emitter.line(format!("br label %{complete}"));
        emitter.start(&complete);
        let text = emitter.temp();
        emitter.line(format!("{text} = load ptr, ptr {output}"));
        emitter.line(format!("store ptr {text}, ptr %out"));
        emitter.disarm(guard);
        let callback = emitter.finish_protocol_callback(&name, "ptr %input, ptr %out, ptr %error");
        self.declarations.push_str(&callback);
        format!("@{name}")
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn display_literal(&mut self, text: &str, span: Span) -> String {
        let (bytes, length) = self.module.text_literal(text);
        let value = self.runtime_call(
            "dever_rt_v1_text_new",
            vec![format!("ptr {bytes}"), format!("i64 {length}")],
            &Type::Text,
            false,
            None,
            span,
        );
        self.own_value(&Type::Text, &value, false);
        value
    }

    fn display_append(&mut self, output: &str, text: &str, span: Span) {
        let previous = self.temp();
        self.line(format!("{previous} = load ptr, ptr {output}"));
        let joined = self.runtime_call(
            "dever_rt_v1_text_concat",
            vec![format!("ptr {previous}"), format!("ptr {text}")],
            &Type::Text,
            false,
            None,
            span,
        );
        self.line(format!(
            "call void @dever_rt_v1_text_release(ptr {previous})"
        ));
        self.line(format!("store ptr {joined}, ptr {output}"));
    }

    pub(super) fn display_append_literal(&mut self, output: &str, text: &str, span: Span) {
        let first = self.guards.len();
        let text = self.display_literal(text, span);
        self.display_append(output, &text, span);
        self.release_from(first);
    }

    fn display_location(&mut self, output: &str, pointer: &str, span: Span) {
        let first = self.guards.len();
        let location = self.temp();
        self.line(format!("{location} = load i32, ptr {pointer}"));
        let text = self.runtime_call(
            "dever_location_text",
            vec![format!("i32 {location}")],
            &Type::Text,
            false,
            None,
            span,
        );
        self.own_value(&Type::Text, &text, false);
        self.display_append(output, &text, span);
        self.release_from(first);
    }

    pub(super) fn display_value(&mut self, output: &str, ty: &Type, value: &str, span: Span) {
        assert!(
            ty.observable(&self.module.program.types),
            "checked diagnostic payload must be observable"
        );
        let first = self.guards.len();
        match ty {
            Type::Text | Type::Json | Type::Id => self.display_append(output, value, span),
            Type::Int
            | Type::DateTime
            | Type::Date
            | Type::Time
            | Type::Duration
            | Type::Float
            | Type::Decimal
            | Type::Uuid => {
                let text = match ty {
                    Type::Decimal => {
                        let parts = self.decimal_parts(value);
                        self.render_text("dever_rt_v1_decimal_to_text", parts, span)
                    }
                    Type::Float => self.render_text(
                        "dever_rt_v1_float_to_text",
                        vec![format!("double {value}")],
                        span,
                    ),
                    Type::Uuid => self.runtime_call(
                        "dever_rt_v1_uuid_to_text",
                        vec![format!("ptr {value}")],
                        &Type::Text,
                        false,
                        None,
                        span,
                    ),
                    _ => self.render_text(
                        "dever_rt_v1_int_to_text",
                        vec![format!("i64 {value}")],
                        span,
                    ),
                };
                self.own_value(&Type::Text, &text, false);
                self.display_append(output, &text, span);
            }
            Type::Bool => {
                let yes = self.label("display_true");
                let no = self.label("display_false");
                let done = self.label("display_bool");
                self.line(format!("br i1 {value}, label %{yes}, label %{no}"));
                self.start(&yes);
                self.display_append_literal(output, "true", span);
                self.line(format!("br label %{done}"));
                self.start(&no);
                self.display_append_literal(output, "false", span);
                self.line(format!("br label %{done}"));
                self.start(&done);
            }
            Type::Nullable(inner) => {
                let present = self.temp();
                self.line(format!(
                    "{present} = extractvalue {} {value}, 0",
                    self.module.ty(ty)
                ));
                let some = self.label("display_some");
                let null = self.label("display_null");
                let done = self.label("display_optional");
                self.line(format!("br i1 {present}, label %{some}, label %{null}"));
                self.start(&null);
                self.display_append_literal(output, "null", span);
                self.line(format!("br label %{done}"));
                self.start(&some);
                let payload = self.temp();
                self.line(format!(
                    "{payload} = extractvalue {} {value}, 1",
                    self.module.ty(ty)
                ));
                self.display_value(output, inner, &payload, span);
                self.line(format!("br label %{done}"));
                self.start(&done);
            }
            Type::Named(id) if self.module.program.types[*id].kind == DefinitionKind::ModelId => {
                self.display_value(output, &Type::Int, value, span);
            }
            Type::Named(id) => self.display_named(output, *id, value, span, None),
            Type::List(_) | Type::Bytes | Type::Map(_, _) => {
                self.display_collection(output, ty, value, span)
            }
            Type::MapEntry(key, mapped) => {
                self.display_append_literal(output, "{key = ", span);
                for (index, field) in [key.as_ref(), mapped.as_ref()].iter().enumerate() {
                    if index == 1 {
                        self.display_append_literal(output, ", value = ", span);
                    }
                    let part = self.temp();
                    self.line(format!(
                        "{part} = extractvalue {} {value}, {index}",
                        self.module.ty(ty)
                    ));
                    self.display_value(output, field, &part, span);
                }
                self.display_append_literal(output, "}", span);
            }
            Type::File | Type::Socket | Type::Listener => {
                self.display_append_literal(output, "<resource>", span)
            }
            Type::Stream(_) => self.display_append_literal(output, "<stream>", span),
            Type::AsyncStream(_) | Type::RowStream(_) => {
                self.display_append_literal(output, "<async-stream>", span)
            }
            Type::Related(inner) => {
                let loaded = self.temp();
                self.line(format!("{loaded} = icmp ne ptr {value}, null"));
                let some = self.label("display_related_loaded");
                let none = self.label("display_related_unloaded");
                let done = self.label("display_related_done");
                self.line(format!("br i1 {loaded}, label %{some}, label %{none}"));
                self.start(&none);
                self.display_append_literal(output, "Related.Unloaded", span);
                self.line(format!("br label %{done}"));
                self.start(&some);
                let renderer = self.module.related_renderer(inner, self.instance, span);
                let text = self.runtime_call(
                    &renderer,
                    vec![format!("ptr {value}")],
                    &Type::Text,
                    false,
                    None,
                    span,
                );
                self.own_value(&Type::Text, &text, false);
                self.display_append(output, &text, span);
                self.line(format!("br label %{done}"));
                self.start(&done);
            }
            Type::HttpReply => self.display_append_literal(output, "HttpReply(<resource>)", span),
            Type::WebSocket => self.display_append_literal(output, "WebSocket(<resource>)", span),
            Type::ClientTls => self.display_append_literal(output, "ClientTls(<resource>)", span),
            Type::ServerTls => self.display_append_literal(output, "ServerTls(<resource>)", span),
            Type::HttpClient => self.display_append_literal(output, "HttpClient(<resource>)", span),
            Type::Channel(_) => self.display_append_literal(output, "Channel(<resource>)", span),
            _ => unreachable!("checked observable error payload"),
        }
        self.release_from(first);
    }

    fn display_named(
        &mut self,
        output: &str,
        id: usize,
        value: &str,
        span: Span,
        selected: Option<&BTreeSet<usize>>,
    ) {
        let definition = self.module.program.types[id].clone();
        match definition.shape {
            Shape::Record(fields) => {
                self.display_append_literal(output, &format!("{} {{ ", definition.name), span);
                for (index, field) in fields.iter().enumerate() {
                    self.display_append_literal(
                        output,
                        &format!("{}{} = ", if index == 0 { "" } else { ", " }, field.name),
                        span,
                    );
                    let part = self.temp();
                    self.line(format!("{part} = extractvalue %T{id} {value}, {index}"));
                    self.display_value(output, &field.ty, &part, span);
                }
                self.display_append_literal(output, " }", span);
            }
            Shape::Choice(variants) => {
                let row = self.row_pointer(&Type::Named(id), value);
                let tag = self.temp();
                self.line(format!("{tag} = extractvalue %T{id} {value}, 0"));
                let labels = variants
                    .iter()
                    .map(|_| self.label("display_variant"))
                    .collect::<Vec<_>>();
                let invalid = self.label("display_invalid");
                let done = self.label("display_choice");
                self.line(format!(
                    "switch i32 {tag}, label %{invalid} [{}]",
                    labels
                        .iter()
                        .enumerate()
                        .filter(
                            |(index, _)| selected.is_none_or(|selected| selected.contains(index))
                        )
                        .map(|(index, label)| format!("i32 {index}, label %{label}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                ));
                self.start(&invalid);
                self.line("unreachable");
                for (index, variant) in variants.iter().enumerate() {
                    if selected.is_some_and(|selected| !selected.contains(&index)) {
                        continue;
                    }
                    self.start(&labels[index]);
                    self.display_append_literal(
                        output,
                        &format!("{}.{}", definition.name, variant.name),
                        span,
                    );
                    if !variant.fields.is_empty() {
                        self.display_append_literal(output, "(", span);
                        let payload = self.temp();
                        self.line(format!(
                            "{payload} = getelementptr %T{id}, ptr {row}, i32 0, i32 1"
                        ));
                        for (field_index, field) in variant.fields.iter().enumerate() {
                            if field_index != 0 {
                                self.display_append_literal(output, ", ", span);
                            }
                            let pointer = self.temp();
                            self.line(format!("{pointer} = getelementptr %T{id}V{index}, ptr {payload}, i32 0, i32 {field_index}"));
                            let part = self.temp();
                            self.line(format!(
                                "{part} = load {}, ptr {pointer}",
                                self.module.ty(&field.ty)
                            ));
                            self.display_value(output, &field.ty, &part, span);
                        }
                        self.display_append_literal(output, ")", span);
                    }
                    self.line(format!("br label %{done}"));
                }
                self.start(&done);
            }
        }
    }

    fn display_collection(&mut self, output: &str, ty: &Type, value: &str, span: Span) {
        let map = matches!(ty, Type::Map(_, _));
        let bytes = *ty == Type::Bytes;
        self.display_append_literal(output, if map { "{" } else { "[" }, span);
        let types = match ty {
            Type::Bytes => vec![Type::Int],
            Type::List(element) => vec![element.as_ref().clone()],
            Type::Map(key, value) => vec![key.as_ref().clone(), value.as_ref().clone()],
            _ => unreachable!(),
        };
        let kind = if map { "map" } else { "list" };
        let cursor = if bytes {
            None
        } else {
            let retained = self.temp();
            self.line(format!(
                "{retained} = call ptr @dever_rt_v1_{kind}_retain(ptr {value})"
            ));
            let cursor = self.runtime_call(
                &format!("dever_rt_v1_{kind}_cursor_take"),
                vec![format!("ptr {retained}")],
                &Type::Unit,
                false,
                Some("ptr"),
                span,
            );
            let pointer = self.entry_slot_ir("ptr");
            self.line(format!("store ptr {cursor}, ptr {pointer}"));
            let guard = self.register_owned(
                if map {
                    OwnedKind::MapCursor
                } else {
                    OwnedKind::ListCursor
                },
                pointer,
            );
            self.mark_live(guard);
            Some((cursor, guard))
        };
        let length = bytes.then(|| {
            self.runtime_call(
                "dever_rt_v1_bytes_length",
                vec![format!("ptr {value}")],
                &Type::Int,
                false,
                None,
                span,
            )
        });
        let index = self.entry_slot_ir("i64");
        self.line(format!("store i64 0, ptr {index}"));
        let rows = types
            .iter()
            .map(|ty| {
                let row = self.entry_slot(ty);
                let guard = contains_owned(self.module.program, ty)
                    .then(|| self.register_guard(ty, row.clone()));
                (row, guard)
            })
            .collect::<Vec<_>>();
        let present = self.entry_slot_ir("i8");
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let next = self.label("display_next");
        let body = self.label("display_element");
        let separator = self.label("display_separator");
        let content = self.label("display_content");
        let done = self.label("display_collection_done");
        self.line(format!("br label %{next}"));
        self.start(&next);
        let position = self.temp();
        self.line(format!("{position} = load i64, ptr {index}"));
        let exists = self.temp();
        if let Some(length) = length {
            self.line(format!("{exists} = icmp slt i64 {position}, {length}"));
        } else {
            let mut args = vec![format!("ptr {}", cursor.as_ref().unwrap().0)];
            args.extend(rows.iter().map(|(row, _)| format!("ptr {row}")));
            args.extend([format!("ptr {present}"), format!("ptr {error}")]);
            let status = self.temp();
            self.line(format!(
                "{status} = call i32 @dever_rt_v1_{kind}_cursor_next({})",
                args.join(", ")
            ));
            self.check_runtime_status(&status, &error, span);
            let flag = self.temp();
            self.line(format!("{flag} = load i8, ptr {present}"));
            self.line(format!("{exists} = icmp ne i8 {flag}, 0"));
        }
        self.line(format!("br i1 {exists}, label %{body}, label %{done}"));
        self.start(&body);
        for (_, guard) in &rows {
            if let Some(guard) = guard {
                self.mark_live(*guard);
            }
        }
        let first = self.temp();
        self.line(format!("{first} = icmp eq i64 {position}, 0"));
        self.line(format!(
            "br i1 {first}, label %{content}, label %{separator}"
        ));
        self.start(&separator);
        self.display_append_literal(output, ", ", span);
        self.line(format!("br label %{content}"));
        self.start(&content);
        for (column, (ty, (row, _))) in types.iter().zip(&rows).enumerate() {
            if column != 0 {
                self.display_append_literal(output, " = ", span);
            }
            let element = if bytes {
                let nullable = self.runtime_call(
                    "dever_rt_v1_bytes_at",
                    vec![format!("ptr {value}"), format!("i64 {position}")],
                    &Type::Nullable(Box::new(Type::Int)),
                    true,
                    None,
                    span,
                );
                let byte = self.temp();
                self.line(format!("{byte} = extractvalue {{ i1, i64 }} {nullable}, 1"));
                byte
            } else {
                let element = self.temp();
                self.line(format!(
                    "{element} = load {}, ptr {row}",
                    self.module.ty(ty)
                ));
                element
            };
            self.display_value(output, ty, &element, span);
        }
        for (_, guard) in &rows {
            if let Some(guard) = guard {
                self.release_guard(*guard);
            }
        }
        let following = self.temp();
        self.line(format!("{following} = add i64 {position}, 1"));
        self.line(format!("store i64 {following}, ptr {index}"));
        self.line(format!("br label %{next}"));
        self.start(&done);
        if let Some((_, guard)) = cursor {
            self.release_guard(guard);
        }
        self.display_append_literal(output, if map { "}" } else { "]" }, span);
    }
}
