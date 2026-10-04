//! Owned protocol records and canonical ABI fields.
use super::*;

impl FunctionEmitter<'_, '_> {
    pub(super) fn protocol_to_runtime(&mut self, ty: &Type, value: &str, span: Span) -> String {
        let Type::Named(id) = ty else { unreachable!() };
        let name = self.module.program.types[*id].name.clone();
        if name == "dever.websocket.Message" {
            return self.websocket_to_runtime(*id, value, span);
        }
        let request = matches!(
            name.as_str(),
            "dever.http.Request" | "dever.http.StreamRequest"
        );
        let stream = name == "dever.http.StreamRequest";
        let mut fields = Vec::new();
        for name in if request {
            &["method", "target"][..]
        } else {
            &["status"][..]
        } {
            fields.push((
                if request { "ptr" } else { "i64" },
                self.record_field(ty, value, name).1,
            ));
        }
        let (headers_ty, headers) = self.record_field(ty, value, "headers");
        fields.push(("ptr", self.headers_to_runtime(&headers_ty, &headers, span)));
        let (body_ty, body) = self.record_field(ty, value, "body");
        fields.push(("ptr", body));
        let fields = self.protocol_struct(&fields);
        let kind = if stream {
            "http_stream_request"
        } else if request {
            "http_request"
        } else {
            "http_response"
        };
        let mut args = vec![format!("ptr {fields}")];
        if stream {
            let Type::AsyncStream(event) = body_ty else {
                unreachable!()
            };
            let Type::Named(id) = event.as_ref() else {
                unreachable!()
            };
            args.push(format!("ptr {}", self.module.upload_decoder(*id)));
        }
        let handle = self.protocol_call(&format!("dever_rt_v1_{kind}_new"), args, "ptr", span);
        self.protocol_owner(kind, &handle);
        handle
    }

    pub(super) fn protocol_to_language(&mut self, ty: &Type, handle: &str, span: Span) -> String {
        let Type::Named(id) = ty else { unreachable!() };
        let definition = self.module.program.types[*id].clone();
        if definition.name == "dever.websocket.Message" {
            self.protocol_owner("ws_message", handle);
            return self.websocket_to_language(*id, handle, span);
        }
        let Shape::Record(fields) = definition.shape else {
            unreachable!()
        };
        let request = definition.name == "dever.http.Request";
        let stream = definition.name == "dever.http.StreamResponse";
        let kind = if request {
            "http_request"
        } else if stream {
            "http_stream_response"
        } else {
            "http_response"
        };
        self.protocol_owner(kind, handle);
        let mut args = vec![format!("ptr {handle}")];
        if stream {
            let Type::AsyncStream(event) =
                &fields.iter().find(|field| field.name == "body").unwrap().ty
            else {
                unreachable!()
            };
            let Type::Named(id) = event.as_ref() else {
                unreachable!()
            };
            args.push(format!("ptr {}", self.module.read_event(*id)));
            args.push(format!(
                "ptr @dever_owned_{}",
                self.module.type_index(event)
            ));
        }
        let layout = if request {
            "{ ptr, ptr, ptr, ptr }"
        } else {
            "{ i64, ptr, ptr }"
        };
        let decoded = self.protocol_call(&format!("dever_rt_v1_{kind}_fields"), args, layout, span);
        let names = if request {
            &["method", "target", "headers", "body"][..]
        } else {
            &["status", "headers", "body"][..]
        };
        let mut values = Vec::new();
        let mut transfers = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let field_ty = &fields.iter().find(|field| field.name == *name).unwrap().ty;
            let value = self.temp();
            self.line(format!(
                "{value} = extractvalue {layout} {decoded}, {index}"
            ));
            if *name == "headers" {
                self.protocol_owner("http_headers", &value);
                values.push((*name, value));
            } else {
                if contains_owned(self.module.program, field_ty) {
                    transfers.push(self.own_value(field_ty, &value, false));
                }
                values.push((*name, value));
            }
        }
        // Every getter field is owned before a subsequent conversion can fail.
        let header_index = values
            .iter()
            .position(|(name, _)| *name == "headers")
            .unwrap();
        let header_ty = &fields
            .iter()
            .find(|field| field.name == "headers")
            .unwrap()
            .ty;
        let headers = self.headers_to_language(header_ty, &values[header_index].1, span);
        transfers.push(self.own_value(header_ty, &headers, false));
        values[header_index].1 = headers;
        let result = self.record_value(ty, &values);
        for guard in transfers {
            self.disarm(guard);
        }
        result
    }

    pub(super) fn sse_event(&mut self, ty: &Type, value: &str) -> String {
        let event = self.record_field(ty, value, "event").1;
        let data = self.record_field(ty, value, "data").1;
        let (id_ty, id) = self.record_field(ty, value, "id");
        let id = self.optional_payload(&id_ty, &id, "null").1;
        let (retry_ty, retry) = self.record_field(ty, value, "retry_ms");
        let (present, retry) = self.optional_payload(&retry_ty, &retry, "0");
        let flag = self.temp();
        self.line(format!("{flag} = zext i1 {present} to i8"));
        self.protocol_struct(&[
            ("ptr", event),
            ("ptr", data),
            ("ptr", id),
            ("i64", retry),
            ("i8", flag),
        ])
    }

    fn optional_payload(&mut self, ty: &Type, value: &str, absent: &str) -> (String, String) {
        let Type::Nullable(inner) = ty else {
            unreachable!()
        };
        let present = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {value}, 0",
            self.module.ty(ty)
        ));
        let payload = self.temp();
        self.line(format!(
            "{payload} = extractvalue {} {value}, 1",
            self.module.ty(ty)
        ));
        let selected = self.temp();
        self.line(format!(
            "{selected} = select i1 {present}, {} {payload}, {} {absent}",
            self.module.ty(inner),
            self.module.ty(inner)
        ));
        (present, selected)
    }

    fn websocket_to_runtime(&mut self, id: usize, value: &str, span: Span) -> String {
        let Shape::Choice(variants) = self.module.program.types[id].shape.clone() else {
            unreachable!()
        };
        let source = self.row_pointer(&Type::Named(id), value);
        let tag = self.temp();
        self.line(format!("{tag} = extractvalue %T{id} {value}, 0"));
        let output = self.entry_slot_ir("ptr");
        let done = self.label("ws_encoded");
        let invalid = self.label("ws_invalid_tag");
        let labels = variants
            .iter()
            .map(|_| self.label("ws_encode"))
            .collect::<Vec<_>>();
        self.line(format!(
            "switch i32 {tag}, label %{invalid} [{}]",
            labels
                .iter()
                .enumerate()
                .map(|(i, label)| format!("i32 {i}, label %{label}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        self.start(&invalid);
        self.line("unreachable");
        for (index, variant) in variants.iter().enumerate() {
            self.start(&labels[index]);
            let pointer = self.temp();
            self.line(format!(
                "{pointer} = getelementptr %T{id}, ptr {source}, i32 0, i32 1"
            ));
            let payload = self.temp();
            self.line(format!("{payload} = load ptr, ptr {pointer}"));
            let kind = ["Text", "Binary", "Ping", "Pong"]
                .iter()
                .position(|name| *name == variant.name)
                .unwrap();
            let handle = self.protocol_call(
                "dever_rt_v1_ws_message_new",
                vec![format!("i32 {kind}"), format!("ptr {payload}")],
                "ptr",
                span,
            );
            self.line(format!("store ptr {handle}, ptr {output}"));
            self.line(format!("br label %{done}"));
        }
        self.start(&done);
        let handle = self.temp();
        self.line(format!("{handle} = load ptr, ptr {output}"));
        self.protocol_owner("ws_message", &handle);
        handle
    }

    fn websocket_to_language(&mut self, id: usize, handle: &str, span: Span) -> String {
        let fields = self.protocol_call(
            "dever_rt_v1_ws_message_fields",
            vec![format!("ptr {handle}")],
            "{ i32, ptr }",
            span,
        );
        let tag = self.temp();
        self.line(format!("{tag} = extractvalue {{ i32, ptr }} {fields}, 0"));
        let payload = self.temp();
        self.line(format!(
            "{payload} = extractvalue {{ i32, ptr }} {fields}, 1"
        ));
        let result = self.entry_slot(&Type::Named(id));
        let Shape::Choice(variants) = self.module.program.types[id].shape.clone() else {
            unreachable!()
        };
        let done = self.label("ws_decoded");
        let invalid = self.label("ws_invalid_kind");
        let labels = (0..4).map(|_| self.label("ws_decode")).collect::<Vec<_>>();
        self.line(format!(
            "switch i32 {tag}, label %{invalid} [{}]",
            labels
                .iter()
                .enumerate()
                .map(|(i, label)| format!("i32 {i}, label %{label}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        self.start(&invalid);
        self.line("unreachable");
        for (kind, name) in ["Text", "Binary", "Ping", "Pong"].iter().enumerate() {
            self.start(&labels[kind]);
            let (variant, definition) = variants
                .iter()
                .enumerate()
                .find(|(_, variant)| variant.name == *name)
                .unwrap();
            self.write_variant(
                id,
                variant,
                &[(definition.fields[0].ty.clone(), payload.clone())],
                &result,
            );
            self.line(format!("br label %{done}"));
        }
        self.start(&done);
        let value = self.temp();
        self.line(format!("{value} = load %T{id}, ptr {result}"));
        value
    }
    pub(super) fn protocol_owner(&mut self, kind: &'static str, handle: &str) -> usize {
        let slot = self.entry_slot_ir("ptr");
        self.line(format!("store ptr {handle}, ptr {slot}"));
        let guard = self.register_owned(OwnedKind::Protocol(kind), slot);
        self.mark_live(guard);
        guard
    }

    pub(super) fn protocol_call(
        &mut self,
        symbol: &str,
        args: Vec<String>,
        layout: &str,
        span: Span,
    ) -> String {
        self.runtime_call(symbol, args, &Type::Unit, false, Some(layout), span)
    }

    fn record_field(&mut self, ty: &Type, value: &str, name: &str) -> (Type, String) {
        let Type::Named(id) = ty else {
            unreachable!("checked protocol record")
        };
        let Shape::Record(fields) = &self.module.program.types[*id].shape else {
            unreachable!()
        };
        let (index, field) = fields
            .iter()
            .enumerate()
            .find(|(_, field)| field.name == name)
            .expect("checked protocol field");
        let field_ty = field.ty.clone();
        let extracted = self.temp();
        self.line(format!(
            "{extracted} = extractvalue {} {value}, {index}",
            self.module.ty(ty)
        ));
        (field_ty, extracted)
    }

    fn record_value(&mut self, ty: &Type, fields: &[(&str, String)]) -> String {
        let Type::Named(id) = ty else { unreachable!() };
        let Shape::Record(definition) = &self.module.program.types[*id].shape else {
            unreachable!()
        };
        let definition = definition.clone();
        let mut value = "zeroinitializer".to_string();
        for (index, field) in definition.iter().enumerate() {
            let (_, content) = fields
                .iter()
                .find(|(name, _)| *name == field.name)
                .expect("complete protocol fields");
            let next = self.temp();
            self.line(format!(
                "{next} = insertvalue {} {value}, {} {content}, {index}",
                self.module.ty(ty),
                self.module.ty(&field.ty)
            ));
            value = next;
        }
        value
    }

    fn protocol_struct(&mut self, fields: &[(&str, String)]) -> String {
        let layout = format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(ty, _)| *ty)
                .collect::<Vec<_>>()
                .join(", ")
        );
        let slot = self.entry_slot_ir(&layout);
        let mut value = "zeroinitializer".to_string();
        for (index, (ty, content)) in fields.iter().enumerate() {
            let next = self.temp();
            self.line(format!(
                "{next} = insertvalue {layout} {value}, {ty} {content}, {index}"
            ));
            value = next;
        }
        self.line(format!("store {layout} {value}, ptr {slot}"));
        slot
    }

    pub(super) fn protocol_limits(&mut self, ty: &Type, value: &str, names: &[&str]) -> String {
        let mut fields = Vec::new();
        for name in names {
            fields.push(("i64", self.record_field(ty, value, name).1));
        }
        self.protocol_struct(&fields)
    }

    pub(super) fn http_limits(&mut self, ty: &Type, value: &str) -> String {
        let mut fields = ["header_bytes", "body_bytes", "timeout_ms", "connections"]
            .iter()
            .map(|name| ("i64", self.record_field(ty, value, name).1))
            .collect::<Vec<_>>();
        let (http2_ty, http2) = self.record_field(ty, value, "http2");
        let Type::Nullable(inner) = &http2_ty else {
            unreachable!()
        };
        let present = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {http2}, 0",
            self.module.ty(&http2_ty)
        ));
        let pointer = self.entry_slot_ir("ptr");
        self.line(format!("store ptr null, ptr {pointer}"));
        let some = self.label("http2_limits");
        let done = self.label("http_limits");
        self.line(format!("br i1 {present}, label %{some}, label %{done}"));
        self.start(&some);
        let content = self.temp();
        self.line(format!(
            "{content} = extractvalue {} {http2}, 1",
            self.module.ty(&http2_ty)
        ));
        let limits = self.protocol_limits(
            inner,
            &content,
            &["streams", "stream_window_bytes", "connection_window_bytes"],
        );
        self.line(format!("store ptr {limits}, ptr {pointer}"));
        self.line(format!("br label %{done}"));
        self.start(&done);
        let http2 = self.temp();
        self.line(format!("{http2} = load ptr, ptr {pointer}"));
        fields.push(("ptr", http2));
        self.protocol_struct(&fields)
    }

    pub(super) fn headers_to_runtime(&mut self, ty: &Type, value: &str, span: Span) -> String {
        let Type::List(element) = ty else {
            unreachable!()
        };
        let headers = self.protocol_call("dever_rt_v1_http_headers_new", vec![], "ptr", span);
        self.protocol_owner("http_headers", &headers);
        let retained = self.temp();
        self.line(format!(
            "{retained} = call ptr @dever_rt_v1_list_retain(ptr {value})"
        ));
        let cursor = self.protocol_call(
            "dever_rt_v1_list_cursor_take",
            vec![format!("ptr {retained}")],
            "ptr",
            span,
        );
        let cursor_slot = self.entry_slot_ir("ptr");
        self.line(format!("store ptr {cursor}, ptr {cursor_slot}"));
        let cursor_guard = self.register_owned(OwnedKind::ListCursor, cursor_slot);
        self.mark_live(cursor_guard);
        let row = self.entry_slot(element);
        let guard = self.register_guard(element, row.clone());
        let present = self.entry_slot_ir("i8");
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let next = self.label("header_next");
        let body = self.label("header_value");
        let done = self.label("headers_ready");
        self.line(format!("br label %{next}"));
        self.start(&next);
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_list_cursor_next(ptr {cursor}, ptr {row}, ptr {present}, ptr {error})"));
        self.check_runtime_status(&status, &error, span);
        let flag = self.temp();
        self.line(format!("{flag} = load i8, ptr {present}"));
        let exists = self.temp();
        self.line(format!("{exists} = icmp ne i8 {flag}, 0"));
        self.line(format!("br i1 {exists}, label %{body}, label %{done}"));
        self.start(&body);
        self.mark_live(guard);
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {row}",
            self.module.ty(element)
        ));
        let name = self.record_field(element, &value, "name").1;
        let value = self.record_field(element, &value, "value").1;
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_http_headers_push(ptr {headers}, ptr {name}, ptr {value}, ptr {error})"));
        self.check_runtime_status(&status, &error, span);
        self.release_guard(guard);
        self.line(format!("br label %{next}"));
        self.start(&done);
        self.release_guard(cursor_guard);
        headers
    }

    fn headers_to_language(&mut self, ty: &Type, headers: &str, span: Span) -> String {
        let Type::List(element) = ty else {
            unreachable!()
        };
        let list = self.runtime_call(
            "dever_rt_v1_list_new",
            vec![
                format!("ptr @dever_type_{}", self.module.type_index(element)),
                "ptr null".into(),
                "i64 0".into(),
            ],
            ty,
            false,
            None,
            span,
        );
        let list_guard = self.own_value(ty, &list, false);
        let list_slot = self.guards[list_guard].pointer.clone();
        let length = self.runtime_call(
            "dever_rt_v1_http_headers_length",
            vec![format!("ptr {headers}")],
            &Type::Int,
            false,
            None,
            span,
        );
        let index = self.entry_slot_ir("i64");
        self.line(format!("store i64 0, ptr {index}"));
        let next = self.label("header_decode_next");
        let body = self.label("header_decode");
        let done = self.label("headers_decoded");
        self.line(format!("br label %{next}"));
        self.start(&next);
        let position = self.temp();
        self.line(format!("{position} = load i64, ptr {index}"));
        let exists = self.temp();
        self.line(format!("{exists} = icmp slt i64 {position}, {length}"));
        self.line(format!("br i1 {exists}, label %{body}, label %{done}"));
        self.start(&body);
        let first = self.guards.len();
        let fields = self.protocol_call(
            "dever_rt_v1_http_headers_get",
            vec![format!("ptr {headers}"), format!("i64 {position}")],
            "{ ptr, ptr }",
            span,
        );
        let name = self.temp();
        self.line(format!("{name} = extractvalue {{ ptr, ptr }} {fields}, 0"));
        self.own_value(&Type::Text, &name, false);
        let value = self.temp();
        self.line(format!("{value} = extractvalue {{ ptr, ptr }} {fields}, 1"));
        self.own_value(&Type::Bytes, &value, false);
        let header = self.record_value(element, &[("name", name), ("value", value)]);
        let row = self.row_pointer(element, &header);
        let previous = self.temp();
        self.line(format!("{previous} = load ptr, ptr {list_slot}"));
        self.disarm(list_guard);
        let appended = self.runtime_call(
            "dever_rt_v1_list_append_take",
            vec![format!("ptr {previous}"), format!("ptr {row}")],
            ty,
            false,
            None,
            span,
        );
        self.line(format!("store ptr {appended}, ptr {list_slot}"));
        self.mark_live(list_guard);
        self.release_from(first);
        let incremented = self.temp();
        self.line(format!("{incremented} = add i64 {position}, 1"));
        self.line(format!("store i64 {incremented}, ptr {index}"));
        self.line(format!("br label %{next}"));
        self.start(&done);
        let list = self.temp();
        self.line(format!("{list} = load ptr, ptr {list_slot}"));
        self.disarm(list_guard);
        list
    }
}
