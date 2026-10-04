//! Concrete codecs consume the checked wire DAG; runtime nodes never enter HIR values.
use super::*;
use crate::wire::{Node, Policy, Schema};

#[derive(Clone, Copy)]
pub(super) enum FieldInput {
    Record,
    External,
    Command,
    Api,
    Rest,
    RestList,
}

pub(super) const DECLARATIONS: &str = "\
declare i32 @dever_rt_v1_wire_decode(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_cmd_decode(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_cmd_input_field(ptr, ptr, i8, ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_encode(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_is_null(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_bool(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_int(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_float(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_text(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_bytes(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_raw(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_decimal(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_uuid(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_secret(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_datetime(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_date(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_time(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_fields(ptr, ptr, i64, ptr)
declare i32 @dever_rt_v1_wire_node_field(ptr, ptr, i8, ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_list_len(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_node_list_at(ptr, i64, ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_null(ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_bool(ptr, i8, ptr)
declare i32 @dever_rt_v1_wire_encoder_int(ptr, i64, ptr)
declare i32 @dever_rt_v1_wire_encoder_float(ptr, double, ptr)
declare i32 @dever_rt_v1_wire_encoder_text(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_bytes(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_json(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_decimal(ptr, i64, i64, ptr)
declare i32 @dever_rt_v1_wire_encoder_uuid(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_datetime(ptr, i64, ptr)
declare i32 @dever_rt_v1_wire_encoder_date(ptr, i64, ptr)
declare i32 @dever_rt_v1_wire_encoder_time(ptr, i64, ptr)
declare i32 @dever_rt_v1_wire_encoder_begin_array(ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_begin_object(ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_key(ptr, ptr, ptr)
declare i32 @dever_rt_v1_wire_encoder_end(ptr, ptr)
";

impl Module<'_> {
    pub(super) fn wire_names(&mut self, names: &[String]) -> String {
        let entries = names
            .iter()
            .map(|name| {
                let (pointer, length) = self.text_literal(name);
                format!("{{ ptr, i64 }} {{ ptr {pointer}, i64 {length} }}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let name = format!("@dever_wire_names_{}", self.next_literal);
        self.next_literal += 1;
        writeln!(
            self.declarations,
            "{name} = private constant [{} x {{ ptr, i64 }}] [{entries}]",
            names.len()
        )
        .unwrap();
        name
    }

    pub(super) fn wire_codec(&mut self, ty: &Type, policy: Policy) -> String {
        let schema = Schema::build(ty, &self.program.types, policy).expect("checked wire schema");
        if let Some(name) = self.wire_codecs.get(schema.fingerprint()) {
            return name.clone();
        }
        let prefix = format!("dever_wire_{}", self.wire_codecs.len());
        self.wire_codecs
            .insert(schema.fingerprint().to_owned(), prefix.clone());
        let instance = self.instances.first().unwrap().clone();
        for (id, node) in schema.nodes.iter().enumerate() {
            let ty = node_type(&schema, id);
            let mut emitter = FunctionEmitter::new(self, &instance);
            emitter.asynchronous = false;
            emitter.protocol_callback = true;
            emitter.start("entry");
            let value = emitter.decode_wire_node(&schema, node, &prefix, "%node");
            emitter.line(format!(
                "store {} {value}, ptr %out",
                emitter.module.ty(&ty)
            ));
            let function = emitter.finish_protocol_callback(
                &format!("{prefix}_d{id}"),
                "ptr %node, ptr %out, ptr %error",
            );
            self.declarations.push_str(&function);
            if !policy.permits_secret() {
                let mut emitter = FunctionEmitter::new(self, &instance);
                emitter.asynchronous = false;
                emitter.protocol_callback = true;
                emitter.start("entry");
                let value = emitter.temp();
                emitter.line(format!(
                    "{value} = load {}, ptr %input",
                    emitter.module.ty(&ty)
                ));
                emitter.encode_wire_node(&schema, node, &prefix, &value);
                let function = emitter.finish_protocol_callback(
                    &format!("{prefix}_e{id}"),
                    "ptr %input, ptr %encoder, ptr %error",
                );
                self.declarations.push_str(&function);
            }
        }
        prefix
    }

    pub(super) fn wire_callback(&mut self, ty: &Type, policy: Policy, encode: bool) -> String {
        let schema = Schema::build(ty, &self.program.types, policy).expect("checked schema");
        let prefix = self.wire_codec(ty, policy);
        format!("{prefix}_{}{}", if encode { "e" } else { "d" }, schema.root)
    }

    /// External operation envelopes always retain field names, including a
    /// single output or an explicitly null field. Their values use the same DAG.
    pub(super) fn wire_fields_callback(
        &mut self,
        fields: &[crate::types::Field],
        name: &str,
        policy: Policy,
        encode: bool,
    ) -> String {
        let ty = Type::Outputs(fields.to_vec());
        let callbacks = fields
            .iter()
            .map(|field| {
                let callback = self.wire_callback(&field.ty, policy, encode);
                (field.name.clone(), field.ty.clone(), callback)
            })
            .collect::<Vec<_>>();
        let instance = self.instances.first().unwrap().clone();
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.asynchronous = false;
        emitter.protocol_callback = true;
        emitter.start("entry");
        let parameters = if encode {
            emitter.wire_write(
                "dever_rt_v1_wire_encoder_begin_object",
                vec!["ptr %encoder".into()],
            );
            for (index, (field, _, callback)) in callbacks.iter().enumerate() {
                let key = emitter.module.wire_names(std::slice::from_ref(field));
                emitter.wire_write(
                    "dever_rt_v1_wire_encoder_key",
                    vec!["ptr %encoder".into(), format!("ptr {key}")],
                );
                let pointer = emitter.temp();
                emitter.line(format!(
                    "{pointer} = getelementptr {}, ptr %input, i32 0, i32 {index}",
                    emitter.module.ty(&ty)
                ));
                emitter.wire_write(
                    callback,
                    vec![format!("ptr {pointer}"), "ptr %encoder".into()],
                );
            }
            emitter.wire_write("dever_rt_v1_wire_encoder_end", vec!["ptr %encoder".into()]);
            "ptr %input, ptr %encoder, ptr %error"
        } else {
            let value = emitter.decode_fields(&ty, &callbacks, "%node", FieldInput::External);
            emitter.line(format!(
                "store {} {value}, ptr %out",
                emitter.module.ty(&ty)
            ));
            "ptr %node, ptr %out, ptr %error"
        };
        let body = emitter.finish_protocol_callback(name, parameters);
        self.declarations.push_str(&body);
        name.to_owned()
    }
}

fn node_type(schema: &Schema, id: usize) -> Type {
    match &schema.nodes[id] {
        Node::Bytes => Type::Bytes,
        Node::Bool => Type::Bool,
        Node::Int => Type::Int,
        Node::Float => Type::Float,
        Node::Decimal => Type::Decimal,
        Node::Text => Type::Text,
        Node::Id => Type::Id,
        Node::Uuid => Type::Uuid,
        Node::DateTime => Type::DateTime,
        Node::Date => Type::Date,
        Node::Time => Type::Time,
        Node::Duration => Type::Duration,
        Node::Json => Type::Json,
        Node::Secret => Type::Secret,
        Node::Nullable(child) => Type::Nullable(Box::new(node_type(schema, *child))),
        Node::List(child) => Type::List(Box::new(node_type(schema, *child))),
        Node::Record { id, .. } | Node::ModelId { id, .. } => Type::Named(*id),
    }
}

impl FunctionEmitter<'_, '_> {
    fn wire_span(&self) -> Span {
        self.module.program.functions[self.instance.function].span
    }

    pub(super) fn wire_write(&mut self, symbol: &str, arguments: Vec<String>) {
        self.wire_write_at(symbol, arguments, self.wire_span());
    }

    pub(super) fn wire_write_at(&mut self, symbol: &str, mut arguments: Vec<String>, span: Span) {
        let error = self.entry_slot_ir("{ ptr, i64 }");
        arguments.push(format!("ptr {error}"));
        let status = self.temp();
        self.line(format!(
            "{status} = call i32 @{symbol}({})",
            arguments.join(", ")
        ));
        self.check_runtime_status(&status, &error, span);
    }

    fn decode_wire_child(&mut self, prefix: &str, child: usize, ty: &Type, node: &str) -> String {
        self.runtime_call(
            &format!("{prefix}_d{child}"),
            vec![format!("ptr {node}")],
            ty,
            false,
            None,
            self.wire_span(),
        )
    }

    pub(super) fn decode_fields(
        &mut self,
        ty: &Type,
        fields: &[(String, Type, String)],
        node: &str,
        input: FieldInput,
    ) -> String {
        let names = self.module.wire_names(
            &fields
                .iter()
                .map(|(name, _, _)| name.clone())
                .collect::<Vec<_>>(),
        );
        if matches!(input, FieldInput::Record | FieldInput::External) {
            self.wire_write(
                "dever_rt_v1_wire_node_fields",
                vec![
                    format!("ptr {node}"),
                    format!("ptr {names}"),
                    format!("i64 {}", fields.len()),
                ],
            );
        }
        let row = self.entry_slot(ty);
        self.line(format!(
            "store {} zeroinitializer, ptr {row}",
            self.module.ty(ty)
        ));
        let guard = self.register_guard(ty, row.clone());
        self.mark_live(guard);
        for (index, (_, field, callback)) in fields.iter().enumerate() {
            let name = self.temp();
            self.line(format!(
                "{name} = getelementptr {{ ptr, i64 }}, ptr {names}, i64 {index}"
            ));
            if *field == Type::Upload {
                let upload = self.runtime_call(
                    "dever_rt_v1_api_input_upload",
                    vec![format!("ptr {node}"), format!("ptr {name}")],
                    field,
                    false,
                    None,
                    self.wire_span(),
                );
                let target = self.temp();
                self.line(format!(
                    "{target} = getelementptr {}, ptr {row}, i32 0, i32 {index}",
                    self.module.ty(ty)
                ));
                self.line(format!("store ptr {upload}, ptr {target}"));
                continue;
            }
            let optional =
                matches!(field, Type::Nullable(_)) && !matches!(input, FieldInput::External);
            let mut arguments = vec![
                format!("ptr {node}"),
                format!("ptr {name}"),
                format!("i8 {}", u8::from(!optional)),
            ];
            let converted_query = matches!(input, FieldInput::Api)
                || matches!(input, FieldInput::RestList) && index < 2;
            if converted_query {
                arguments.push(format!(
                    "i32 {}",
                    api_query_kind(self.module.program, field)
                ));
            }
            let found = self.runtime_call(
                match input {
                    FieldInput::Record | FieldInput::External => "dever_rt_v1_wire_node_field",
                    FieldInput::Command => "dever_rt_v1_cmd_input_field",
                    FieldInput::Api => "dever_rt_v1_api_input_field",
                    FieldInput::Rest => "dever_rt_v1_api_input_raw_field",
                    FieldInput::RestList if index < 2 => "dever_rt_v1_api_input_field",
                    FieldInput::RestList => "dever_rt_v1_api_input_raw_field",
                },
                arguments,
                &Type::Nullable(Box::new(Type::Text)),
                true,
                None,
                self.wire_span(),
            );
            // CMD Inputs transfers an owned raw Text. Record Nodes remain
            // borrowed from their enclosing synchronous wire decode callback.
            let raw_guard = (!matches!(input, FieldInput::Record | FieldInput::External))
                .then(|| self.own_value(&Type::Nullable(Box::new(Type::Text)), &found, false));
            let present = self.temp();
            self.line(format!("{present} = extractvalue {{ i1, ptr }} {found}, 0"));
            let read = self.label("wire_field");
            let done = self.label("wire_field_done");
            self.line(format!("br i1 {present}, label %{read}, label %{done}"));
            self.start(&read);
            let child = self.temp();
            self.line(format!("{child} = extractvalue {{ i1, ptr }} {found}, 1"));
            let search = matches!(input, FieldInput::RestList) && index == 2;
            let decoded = if search {
                let Type::Nullable(inner) = field else {
                    unreachable!()
                };
                inner.as_ref()
            } else {
                field
            };
            let (decode, arguments) = match input {
                FieldInput::Record | FieldInput::External => {
                    (callback.as_str(), vec![format!("ptr {child}")])
                }
                FieldInput::Command | FieldInput::Api | FieldInput::Rest | FieldInput::RestList => {
                    (
                        "dever_rt_v1_wire_decode",
                        vec![
                            format!("ptr {child}"),
                            format!("ptr @dever_type_{}", self.module.type_index(decoded)),
                            format!("ptr @{callback}"),
                        ],
                    )
                }
            };
            let mut value =
                self.runtime_call(decode, arguments, decoded, false, None, self.wire_span());
            if search {
                let flag = self.temp();
                let payload = self.temp();
                self.line(format!(
                    "{flag} = insertvalue {} zeroinitializer, i1 true, 0",
                    self.module.ty(field)
                ));
                self.line(format!(
                    "{payload} = insertvalue {} {flag}, {} {value}, 1",
                    self.module.ty(field),
                    self.module.ty(decoded)
                ));
                value = payload;
            }
            let target = self.temp();
            self.line(format!(
                "{target} = getelementptr {}, ptr {row}, i32 0, i32 {index}",
                self.module.ty(ty)
            ));
            self.line(format!(
                "store {} {value}, ptr {target}",
                self.module.ty(field)
            ));
            self.line(format!("br label %{done}"));
            self.start(&done);
            if let Some(guard) = raw_guard {
                self.release_guard(guard);
            }
        }
        let value = self.temp();
        self.line(format!("{value} = load {}, ptr {row}", self.module.ty(ty)));
        self.disarm(guard);
        value
    }

    fn decode_wire_node(
        &mut self,
        schema: &Schema,
        node: &Node,
        prefix: &str,
        input: &str,
    ) -> String {
        if let Node::Record { id, fields, .. } = node {
            let fields = fields
                .iter()
                .map(|(name, child)| {
                    (
                        name.clone(),
                        node_type(schema, *child),
                        format!("{prefix}_d{child}"),
                    )
                })
                .collect::<Vec<_>>();
            return self.decode_fields(&Type::Named(*id), &fields, input, FieldInput::Record);
        }
        if let Node::Nullable(child) = node {
            let ty = Type::Nullable(Box::new(node_type(schema, *child)));
            let row = self.entry_slot(&ty);
            self.line(format!(
                "store {} zeroinitializer, ptr {row}",
                self.module.ty(&ty)
            ));
            let null = self.runtime_call(
                "dever_rt_v1_wire_node_is_null",
                vec![format!("ptr {input}")],
                &Type::Bool,
                false,
                Some("i8"),
                self.wire_span(),
            );
            let some = self.label("wire_some");
            let done = self.label("wire_optional_done");
            self.line(format!("br i1 {null}, label %{done}, label %{some}"));
            self.start(&some);
            let child_ty = node_type(schema, *child);
            let value = self.decode_wire_child(prefix, *child, &child_ty, input);
            let result = self.temp();
            self.line(format!(
                "{result} = insertvalue {} {{ i1 true, {} zeroinitializer }}, {} {value}, 1",
                self.module.ty(&ty),
                self.module.ty(&child_ty),
                self.module.ty(&child_ty)
            ));
            self.line(format!("store {} {result}, ptr {row}", self.module.ty(&ty)));
            self.line(format!("br label %{done}"));
            self.start(&done);
            let value = self.temp();
            self.line(format!("{value} = load {}, ptr {row}", self.module.ty(&ty)));
            return value;
        }
        if let Node::List(child) = node {
            return self.decode_wire_list(schema, *child, prefix, input);
        }
        let (kind, ty, physical) = match node {
            Node::Bytes => ("bytes", Type::Bytes, None),
            Node::Bool => ("bool", Type::Bool, Some("i8")),
            Node::Int | Node::ModelId { .. } | Node::Duration => ("int", Type::Int, None),
            Node::Float => ("float", Type::Float, None),
            Node::Decimal => ("decimal", Type::Decimal, None),
            Node::Text | Node::Id => ("text", Type::Text, None),
            Node::Json => ("raw", Type::Json, None),
            Node::Uuid => ("uuid", Type::Uuid, None),
            Node::Secret => ("secret", Type::Secret, None),
            Node::DateTime => ("datetime", Type::DateTime, None),
            Node::Date => ("date", Type::Date, None),
            Node::Time => ("time", Type::Time, None),
            _ => unreachable!(),
        };
        self.runtime_call(
            &format!("dever_rt_v1_wire_node_{kind}"),
            vec![format!("ptr {input}")],
            &ty,
            false,
            physical,
            self.wire_span(),
        )
    }

    fn decode_wire_list(
        &mut self,
        schema: &Schema,
        child: usize,
        prefix: &str,
        input: &str,
    ) -> String {
        let element = node_type(schema, child);
        let ty = Type::List(Box::new(element.clone()));
        let list = self.runtime_call(
            "dever_rt_v1_list_new",
            vec![
                format!("ptr @dever_type_{}", self.module.type_index(&element)),
                "ptr null".into(),
                "i64 0".into(),
            ],
            &ty,
            false,
            None,
            self.wire_span(),
        );
        let guard = self.own_value(&ty, &list, false);
        let slot = self.guards[guard].pointer.clone();
        let length = self.runtime_call(
            "dever_rt_v1_wire_node_list_len",
            vec![format!("ptr {input}")],
            &Type::Int,
            false,
            None,
            self.wire_span(),
        );
        let index = self.entry_slot_ir("i64");
        self.line(format!("store i64 0, ptr {index}"));
        let next = self.label("wire_list_next");
        let body = self.label("wire_list_item");
        let done = self.label("wire_list_done");
        self.line(format!("br label %{next}"));
        self.start(&next);
        let position = self.temp();
        self.line(format!("{position} = load i64, ptr {index}"));
        let more = self.temp();
        self.line(format!("{more} = icmp ult i64 {position}, {length}"));
        self.line(format!("br i1 {more}, label %{body}, label %{done}"));
        self.start(&body);
        let first = self.guards.len();
        let node = self.protocol_call(
            "dever_rt_v1_wire_node_list_at",
            vec![format!("ptr {input}"), format!("i64 {position}")],
            "ptr",
            self.wire_span(),
        );
        let value = self.decode_wire_child(prefix, child, &element, &node);
        self.own_value(&element, &value, false);
        let item = self.row_pointer(&element, &value);
        let old = self.temp();
        self.line(format!("{old} = load ptr, ptr {slot}"));
        self.disarm(guard);
        let appended = self.runtime_call(
            "dever_rt_v1_list_append_take",
            vec![format!("ptr {old}"), format!("ptr {item}")],
            &ty,
            false,
            None,
            self.wire_span(),
        );
        self.line(format!("store ptr {appended}, ptr {slot}"));
        self.mark_live(guard);
        self.release_from(first);
        let increment = self.temp();
        self.line(format!("{increment} = add i64 {position}, 1"));
        self.line(format!("store i64 {increment}, ptr {index}"));
        self.line(format!("br label %{next}"));
        self.start(&done);
        let value = self.temp();
        self.line(format!("{value} = load ptr, ptr {slot}"));
        self.disarm(guard);
        value
    }

    fn encode_wire_node(&mut self, schema: &Schema, node: &Node, prefix: &str, value: &str) {
        if let Node::Nullable(child) = node {
            let ty = Type::Nullable(Box::new(node_type(schema, *child)));
            let present = self.temp();
            self.line(format!(
                "{present} = extractvalue {} {value}, 0",
                self.module.ty(&ty)
            ));
            let some = self.label("wire_encode_some");
            let none = self.label("wire_encode_none");
            let done = self.label("wire_encode_done");
            self.line(format!("br i1 {present}, label %{some}, label %{none}"));
            self.start(&none);
            self.wire_write("dever_rt_v1_wire_encoder_null", vec!["ptr %encoder".into()]);
            self.line(format!("br label %{done}"));
            self.start(&some);
            let payload = self.temp();
            self.line(format!(
                "{payload} = extractvalue {} {value}, 1",
                self.module.ty(&ty)
            ));
            self.encode_wire_child(schema, *child, prefix, &payload);
            self.line(format!("br label %{done}"));
            self.start(&done);
            return;
        }
        if let Node::Record { id, fields, .. } = node {
            self.wire_write(
                "dever_rt_v1_wire_encoder_begin_object",
                vec!["ptr %encoder".into()],
            );
            for (index, (name, child)) in fields.iter().enumerate() {
                let key = self.module.wire_names(std::slice::from_ref(name));
                self.wire_write(
                    "dever_rt_v1_wire_encoder_key",
                    vec!["ptr %encoder".into(), format!("ptr {key}")],
                );
                let field = self.temp();
                self.line(format!("{field} = extractvalue %T{id} {value}, {index}"));
                self.encode_wire_child(schema, *child, prefix, &field);
            }
            self.wire_write("dever_rt_v1_wire_encoder_end", vec!["ptr %encoder".into()]);
            return;
        }
        if let Node::List(child) = node {
            self.encode_wire_list(schema, *child, prefix, value);
            return;
        }
        let mut args = vec!["ptr %encoder".into()];
        let (kind, llvm) = match node {
            Node::Bytes => ("bytes", "ptr"),
            Node::Bool => ("bool", "i8"),
            Node::Int | Node::Duration | Node::ModelId { .. } => ("int", "i64"),
            Node::Float => ("float", "double"),
            Node::Text | Node::Id => ("text", "ptr"),
            Node::Json => ("json", "ptr"),
            Node::Decimal => ("decimal", ""),
            Node::Uuid => ("uuid", "ptr"),
            Node::DateTime => ("datetime", "i64"),
            Node::Date => ("date", "i64"),
            Node::Time => ("time", "i64"),
            _ => unreachable!("checked output wire node"),
        };
        if matches!(node, Node::Decimal) {
            args.extend(self.decimal_parts(value));
        } else if matches!(node, Node::Bool) {
            let byte = self.temp();
            self.line(format!("{byte} = zext i1 {value} to i8"));
            args.push(format!("i8 {byte}"));
        } else {
            args.push(format!("{llvm} {value}"));
        }
        self.wire_write(&format!("dever_rt_v1_wire_encoder_{kind}"), args);
    }

    fn encode_wire_child(&mut self, schema: &Schema, child: usize, prefix: &str, value: &str) {
        let row = self.row_pointer(&node_type(schema, child), value);
        self.wire_write(
            &format!("{prefix}_e{child}"),
            vec![format!("ptr {row}"), "ptr %encoder".into()],
        );
    }

    fn encode_wire_list(&mut self, schema: &Schema, child: usize, prefix: &str, value: &str) {
        self.encode_list_callback(
            &node_type(schema, child),
            &format!("{prefix}_e{child}"),
            value,
        );
    }

    pub(super) fn encode_list_callback(&mut self, element: &Type, callback: &str, value: &str) {
        self.wire_write(
            "dever_rt_v1_wire_encoder_begin_array",
            vec!["ptr %encoder".into()],
        );
        let retained = self.temp();
        self.line(format!(
            "{retained} = call ptr @dever_rt_v1_list_retain(ptr {value})"
        ));
        let cursor = self.protocol_call(
            "dever_rt_v1_list_cursor_take",
            vec![format!("ptr {retained}")],
            "ptr",
            self.wire_span(),
        );
        let slot = self.entry_slot_ir("ptr");
        self.line(format!("store ptr {cursor}, ptr {slot}"));
        let cursor_guard = self.register_owned(OwnedKind::ListCursor, slot);
        self.mark_live(cursor_guard);
        let row = self.entry_slot(element);
        let guard = self.register_guard(element, row.clone());
        let present = self.entry_slot_ir("i8");
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let next = self.label("wire_encode_next");
        let body = self.label("wire_encode_item");
        let done = self.label("wire_encode_list_done");
        self.line(format!("br label %{next}"));
        self.start(&next);
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_list_cursor_next(ptr {cursor}, ptr {row}, ptr {present}, ptr {error})"));
        self.check_runtime_status(&status, &error, self.wire_span());
        let flag = self.temp();
        self.line(format!("{flag} = load i8, ptr {present}"));
        let more = self.temp();
        self.line(format!("{more} = icmp ne i8 {flag}, 0"));
        self.line(format!("br i1 {more}, label %{body}, label %{done}"));
        self.start(&body);
        self.mark_live(guard);
        self.wire_write(callback, vec![format!("ptr {row}"), "ptr %encoder".into()]);
        self.release_guard(guard);
        self.line(format!("br label %{next}"));
        self.start(&done);
        self.release_guard(cursor_guard);
        self.wire_write("dever_rt_v1_wire_encoder_end", vec!["ptr %encoder".into()]);
    }
}

fn api_query_kind(program: &Program, ty: &Type) -> u32 {
    let schema = Schema::build(ty, &program.types, Policy::ApiInput).expect("checked API schema");
    match schema.query_kind() {
        "Int" => 1,
        "Bool" => 2,
        "Float" => 3,
        _ => 0,
    }
}
