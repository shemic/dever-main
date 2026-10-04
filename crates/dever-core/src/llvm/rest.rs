//! REST adapters reuse typed Model query, bind, row, and transaction lowering.
use super::*;
use crate::hir::{ApiRest, ModelCondition, ModelOperation, ModelOrder, QueryPlan};
use crate::model::{DatabaseOwner, ModelFieldType, RestOwnerSource, RestValueSource};
use crate::native::orm as sql;

#[derive(Clone)]
pub(super) struct Handler {
    pub function: usize,
    pub route: usize,
    pub method: &'static str,
    pub detail: bool,
    pub inputs: Vec<Input>,
}

#[derive(Clone)]
pub(super) struct Input {
    pub name: String,
    pub ty: Type,
    pub field: Option<usize>,
}

pub(super) fn prepare(program: &mut Program) -> (Vec<Handler>, Option<usize>) {
    if program.api_rest.is_empty() {
        return (Vec::new(), None);
    }
    let span = program.api_rest[0].span;
    let properties = program
        .types
        .iter()
        .find(|definition| definition.kind == DefinitionKind::ModelId)
        .expect("REST Model id properties")
        .properties
        .clone();
    let error = program.types.len();
    program.types.push(crate::types::Definition {
        name: "<generated REST Error>".into(),
        owner: 0,
        public: false,
        span,
        shape: Shape::Choice(
            ["Invalid", "Forbidden", "NotFound"]
                .iter()
                .map(|name| crate::types::Variant {
                    name: (*name).into(),
                    fields: if *name == "Invalid" {
                        vec![crate::types::Field {
                            name: "message".into(),
                            ty: Type::Text,
                            private: false,
                            bounds: Vec::new(),
                        }]
                    } else {
                        Vec::new()
                    },
                    error: true,
                    label: None,
                })
                .collect(),
        ),
        properties,
        kind: DefinitionKind::Regular,
    });
    let mut handlers = Vec::new();
    for (route_index, route) in program.api_rest.clone().iter().enumerate() {
        let model = &program.models[route.model];
        let Shape::Record(fields) = program.types[model.record].shape.clone() else {
            unreachable!();
        };
        for (method, detail) in [
            ("GET", false),
            ("POST", false),
            ("GET", true),
            ("PUT", true),
            ("DELETE", true),
        ] {
            let mut inputs = Vec::new();
            if detail {
                inputs.push(Input {
                    name: "id".into(),
                    ty: Type::Int,
                    field: None,
                });
            }
            if method == "GET" && !detail {
                for name in ["page", "size"] {
                    inputs.push(Input {
                        name: name.into(),
                        ty: Type::Nullable(Box::new(Type::Int)),
                        field: None,
                    });
                }
                if let Some(search) = &route.contract.search {
                    inputs.push(Input {
                        name: fields[search.field].name.clone(),
                        ty: fields[search.field].ty.clone().nullable(),
                        field: Some(search.field),
                    });
                }
            }
            let writes = match method {
                "POST" => &route.contract.create[..],
                "PUT" => &route.contract.replace[..],
                _ => &[],
            };
            for write in writes {
                if matches!(
                    write.source,
                    RestValueSource::Input
                        | RestValueSource::Binding {
                            uses_input: true,
                            ..
                        }
                ) {
                    inputs.push(Input {
                        name: fields[write.field].name.clone(),
                        ty: fields[write.field].ty.clone(),
                        field: Some(write.field),
                    });
                }
            }
            let mut function = application::generated_function(
                format!("<generated REST {route_index} {method} {detail}>"),
                route.span,
            );
            function.parameters = inputs
                .iter()
                .map(|input| Parameter::Value(input.ty.clone()))
                .collect();
            function.outputs = vec![crate::types::Field {
                name: "encoded".into(),
                ty: Type::Text,
                private: false,
                bounds: Vec::new(),
            }];
            let bindings = writes
                .iter()
                .filter_map(|write| match &write.source {
                    RestValueSource::Binding { expression, .. } => Some(expression.clone()),
                    _ => None,
                })
                .chain(
                    route
                        .contract
                        .search
                        .iter()
                        .map(|search| search.expression.clone()),
                )
                .collect::<Vec<_>>();
            // Metadata-only body preserves every actual expression dependency;
            // emit_rest_body owns the compiler-generated orchestration.
            function.clauses = vec![crate::hir::Clause {
                patterns: Vec::new(),
                bindings: Vec::new(),
                locals: fields.iter().map(|field| field.ty.clone()).collect(),
                outputs: Vec::new(),
                body: bindings.into_iter().map(Statement::Call).collect(),
                terminates: false,
            }];
            let id = application::append_generated(program, function);
            program.database_effects[id].insert(DatabaseOwner::Model(route.model));
            program.failures[id].extend(crate::capture::database_errors(&program.types));
            program.failures[id]
                .extend((0..3).map(|variant| crate::hir::Failure { ty: error, variant }));
            let instance = Specialization {
                function: id,
                handlers: Vec::new(),
            };
            let reachable = specialize::execution_reachable(&program.functions, [instance])
                .expect("checked REST binding call graph");
            for root in reachable {
                let failures = specialize::failures(program, &root);
                program.failures[id].extend(failures);
            }
            let mut direct = BTreeSet::new();
            crate::check::visit(&program.functions[id], |expression| {
                if let ExpressionKind::Fail(error) = &expression.kind
                    && let Type::Named(ty) = error.ty
                    && let ExpressionKind::Variant { variant, .. } = error.kind
                {
                    direct.insert(crate::hir::Failure { ty, variant });
                }
            });
            program.failures[id].extend(direct);
            handlers.push(Handler {
                function: id,
                route: route_index,
                method,
                detail,
                inputs,
            });
        }
    }
    (handlers, Some(error))
}

impl Module<'_> {
    pub(super) fn rest_fault_message(&mut self) {
        let Some(id) = self
            .application
            .as_ref()
            .and_then(|application| application.rest_error)
        else {
            return;
        };
        writeln!(self.declarations, "define internal ptr @dever_rest_fault_message(ptr %fault) {{\nentry:\n  %code_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 0\n  %code = load i32, ptr %code_ptr\n  %business = icmp eq i32 %code, 4\n  %type_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 5\n  %type = load i32, ptr %type_ptr\n  %matches = icmp eq i32 %type, {}\n  %variant_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 6\n  %variant = load i32, ptr %variant_ptr\n  %invalid = icmp eq i32 %variant, 0\n  %nominal = and i1 %business, %matches\n  %selected = and i1 %nominal, %invalid\n  br i1 %selected, label %message, label %absent\nmessage:\n  %payload = getelementptr %dever.fault, ptr %fault, i32 0, i32 7\n  %field = getelementptr %T{id}, ptr %payload, i32 0, i32 1\n  %text = load ptr, ptr %field\n  ret ptr %text\nabsent:\n  ret ptr null\n}}", id + 1).unwrap();
    }
    pub(super) fn emit_rest_body(
        &mut self,
        instance: &Specialization,
        handler: &Handler,
    ) -> Result<(), String> {
        let route = self.program.api_rest[handler.route].clone();
        let function = self.program.functions[instance.function].clone();
        let mut emitter = FunctionEmitter::new(self, instance);
        let parameters = emitter.begin_generated_function(&function);
        let encoded = emitter.rest_response(handler, &route)?;
        let owner = emitter.own_value(&Type::Text, &encoded, false);
        if handler.method != "GET" {
            emitter.commit_command_transaction();
        }
        emitter.line(format!("store ptr {encoded}, ptr %out"));
        emitter.disarm(owner);
        emitter.exit("0");
        let name = emitter.module.names[instance].clone();
        let body = emitter.finish_function(&name, &parameters);
        self.functions.push_str(&body);
        Ok(())
    }

    pub(super) fn rest_handler(&mut self, index: usize, handler: &Handler) -> String {
        let instance = Specialization {
            function: handler.function,
            handlers: Vec::new(),
        };
        let input = asynchronous::input_type(&self.program.functions[handler.function]);
        let decoded_inputs = &handler.inputs[usize::from(handler.detail)..];
        let decoded_type = Type::Outputs(
            decoded_inputs
                .iter()
                .enumerate()
                .map(|(index, input)| crate::types::Field {
                    name: format!("p{index}"),
                    ty: input.ty.clone(),
                    private: false,
                    bounds: Vec::new(),
                })
                .collect(),
        );
        let list = handler.method == "GET" && !handler.detail;
        let fields = decoded_inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                let decoded = if list && index == 2 {
                    let Type::Nullable(inner) = &input.ty else {
                        unreachable!()
                    };
                    inner.as_ref()
                } else {
                    &input.ty
                };
                (
                    input.name.clone(),
                    input.ty.clone(),
                    self.wire_callback(decoded, crate::wire::Policy::ApiInput, false),
                )
            })
            .collect::<Vec<_>>();
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.asynchronous = false;
        emitter.protocol_callback = true;
        emitter.start("entry");
        let value = emitter.decode_fields(
            &decoded_type,
            &fields,
            "%inputs",
            if list {
                wire::FieldInput::RestList
            } else {
                wire::FieldInput::Rest
            },
        );
        let owner = emitter.own_value(&decoded_type, &value, false);
        let mut row = "zeroinitializer".to_owned();
        if handler.detail {
            let next = emitter.temp();
            emitter.line(format!(
                "{next} = insertvalue {} {row}, i64 %detail_id, 0",
                emitter.module.ty(&input)
            ));
            row = next;
        }
        for (index, field) in decoded_inputs.iter().enumerate() {
            let child = emitter.temp();
            emitter.line(format!(
                "{child} = extractvalue {} {value}, {index}",
                emitter.module.ty(&decoded_type)
            ));
            if let Some(field_index) = field.field {
                let model = &emitter.module.program.models
                    [emitter.module.program.api_rest[handler.route].model];
                let storage = model.fields[field_index].ty.clone();
                emitter.rest_text_bounds(&field.name, &field.ty, &child, &storage, false);
            }
            let next = emitter.temp();
            emitter.line(format!(
                "{next} = insertvalue {} {row}, {} {child}, {}",
                emitter.module.ty(&input),
                emitter.module.ty(&field.ty),
                index + usize::from(handler.detail)
            ));
            row = next;
        }
        emitter.line(format!(
            "store {} {row}, ptr %out",
            emitter.module.ty(&input)
        ));
        emitter.disarm(owner);
        let callback = format!("dever_rest_decode_{index}");
        let body = emitter.finish_protocol_callback(
            &callback,
            "ptr %inputs, i64 %detail_id, ptr %out, ptr %error",
        );
        self.declarations.push_str(&body);
        let name = &self.names[&instance];
        let descriptor = format!("@dever_rest_handler_{index}");
        writeln!(self.declarations, "{descriptor} = private constant %dever.api_handler {{ ptr @dever_async_{name}, ptr null, ptr @dever_owned_{}, ptr @{callback}, ptr @dever_rest_fault_status, ptr @dever_rest_fault_message }}", self.type_index(&input)).unwrap();
        descriptor
    }

    pub(super) fn rest_encoder(&mut self, route_index: usize, page: bool) -> String {
        let name = format!("dever_rest_encode_{route_index}_{}", u8::from(page));
        if !self.rest_encoders.insert((route_index, page)) {
            return name;
        }
        let route = self.program.api_rest[route_index].clone();
        let model = self.program.models[route.model].clone();
        let id = if page { model.page } else { model.record };
        let Shape::Record(fields) = self.program.types[id].shape.clone() else {
            unreachable!();
        };
        let selected = if page {
            (0..4).collect::<Vec<_>>()
        } else {
            route.contract.read.clone()
        };
        let mut codecs = Vec::new();
        for field in &selected {
            codecs.push(if page && *field == 0 {
                self.rest_encoder(route_index, false)
            } else {
                self.wire_callback(&fields[*field].ty, crate::wire::Policy::Output, true)
            });
        }
        let instance = self.instances.first().unwrap().clone();
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.asynchronous = false;
        emitter.protocol_callback = true;
        emitter.start("entry");
        let value = emitter.temp();
        emitter.line(format!("{value} = load %T{id}, ptr %input"));
        emitter.wire_write(
            "dever_rt_v1_wire_encoder_begin_object",
            vec!["ptr %encoder".into()],
        );
        for (index, codec) in selected.iter().zip(codecs) {
            let key = emitter.module.wire_names(&[fields[*index].name.clone()]);
            emitter.wire_write(
                "dever_rt_v1_wire_encoder_key",
                vec!["ptr %encoder".into(), format!("ptr {key}")],
            );
            let child = emitter.temp();
            emitter.line(format!("{child} = extractvalue %T{id} {value}, {index}"));
            if page && *index == 0 {
                emitter.encode_list_callback(&Type::Named(model.record), &codec, &child);
            } else {
                let row = emitter.row_pointer(&fields[*index].ty, &child);
                emitter.wire_write(&codec, vec![format!("ptr {row}"), "ptr %encoder".into()]);
            }
        }
        emitter.wire_write("dever_rt_v1_wire_encoder_end", vec!["ptr %encoder".into()]);
        let body = emitter.finish_protocol_callback(&name, "ptr %input, ptr %encoder, ptr %error");
        self.declarations.push_str(&body);
        name
    }
}

impl FunctionEmitter<'_, '_> {
    fn rest_response(&mut self, handler: &Handler, route: &ApiRest) -> Result<String, String> {
        let span = route.span;
        let model = self.module.program.models[route.model].clone();
        let Shape::Record(fields) = self.module.program.types[model.record].shape.clone() else {
            unreachable!();
        };
        // Bindings own one compiler-assigned input local. Other decoded values
        // live in separate slots so evaluating a binding cannot alias them.
        let empty = self.entry_slot(&Type::Unit);
        self.locals.push((Type::Unit, empty));
        let mut inputs = Vec::new();
        for (index, input) in handler.inputs.iter().enumerate() {
            inputs.push(self.rest_local(&input.ty, &self.parameter(index), span));
        }
        let mut writes = self.rest_write_values(handler, route, &inputs)?;
        if handler.method == "GET" && !handler.detail && route.contract.search.is_some() {
            inputs[2] = self.rest_search_value(route, &inputs[2])?;
        }
        let owner = if let Some((field, source)) = route.contract.owner {
            let symbol = match source {
                RestOwnerSource::User => "auth_user_id",
                RestOwnerSource::Tenant => "auth_tenant_id",
            };
            let optional = self.runtime_call(
                &format!("dever_rt_v1_{symbol}"),
                Vec::new(),
                &Type::Nullable(Box::new(Type::Int)),
                true,
                None,
                span,
            );
            let present = self.temp();
            let id = self.temp();
            self.line(format!(
                "{present} = extractvalue {{ i1, i64 }} {optional}, 0"
            ));
            let found = self.label("rest_owner");
            let missing = self.label("rest_owner_missing");
            self.line(format!("br i1 {present}, label %{found}, label %{missing}"));
            self.start(&missing);
            self.rest_fail(1, span);
            self.start(&found);
            self.line(format!("{id} = extractvalue {{ i1, i64 }} {optional}, 1"));
            Some((field, self.rest_local(&fields[field].ty, &id, span)))
        } else {
            None
        };
        let mut condition = owner
            .as_ref()
            .map(|(field, value)| ModelCondition::Compare {
                field: *field,
                operator: BinaryOperator::Equal,
                value: value.clone(),
            });
        if handler.detail {
            let id = ModelCondition::Compare {
                field: 0,
                operator: BinaryOperator::Equal,
                value: inputs[0].clone(),
            };
            condition = Some(match condition {
                Some(owner) => ModelCondition::And(Box::new(id), Box::new(owner)),
                None => id,
            });
        }
        let constant = |value| Expression {
            kind: ExpressionKind::Constant(Constant::Int(value)),
            ty: Type::Int,
            span,
        };
        let query = QueryPlan {
            condition,
            order: vec![ModelOrder {
                field: 0,
                descending: true,
            }],
            relations: Vec::new(),
            after: None,
            page: constant(1),
            size: constant(20),
        };
        if handler.method == "POST"
            && let Some(owner) = owner
        {
            writes.push(owner);
        }
        if handler.method != "GET" {
            self.begin_transaction_scope(self.instance, &Type::Unit, span);
        }
        let model_value = if handler.method == "GET" && !handler.detail {
            (
                Type::Named(model.page),
                self.rest_list(route, &inputs, query)?,
            )
        } else if handler.method == "DELETE" {
            let result = Expression {
                ty: Type::Int,
                span,
                kind: ExpressionKind::Constant(Constant::Null),
            };
            let affected =
                self.model_operation(route.model, &ModelOperation::Delete { query }, &result)?;
            self.rest_require_affected(&affected, span);
            // The wrapper owns the encoded response across commit and transfers
            // it to its caller. Do not register a second owner for this Text.
            let (pointer, length) = self.module.text_literal("null");
            return Ok(self.runtime_call(
                "dever_rt_v1_text_new",
                vec![format!("ptr {pointer}"), format!("i64 {length}")],
                &Type::Text,
                false,
                None,
                span,
            ));
        } else if handler.method == "POST" {
            let result = Expression {
                ty: Type::Named(model.record),
                span,
                kind: ExpressionKind::Constant(Constant::Null),
            };
            (
                result.ty.clone(),
                self.model_operation(
                    route.model,
                    &ModelOperation::Create { values: writes },
                    &result,
                )?,
            )
        } else {
            let (statement, values) = if handler.method == "PUT" {
                let mut expressions = writes
                    .iter()
                    .map(|(field, expression)| (expression, &model.fields[*field].ty))
                    .collect::<Vec<_>>();
                let assignments =
                    sql::update_assignments(&model, writes.iter().map(|(field, _)| *field), 1);
                let condition =
                    sql::condition_sql(&model, query.condition.as_ref(), &mut expressions);
                let values = self.model_bind(&expressions)?;
                (
                    self.module.planned_sql(&format!(
                        "UPDATE {} SET {assignments}{condition} RETURNING {}",
                        sql::quoted(&model.table),
                        sql::select_columns(&model)
                    )),
                    values,
                )
            } else {
                self.model_select(&model, &query, " LIMIT 1")?
            };
            let rows = self.model_query(route.model, &statement, &values, "0", span);
            let value = self.model_first(
                &rows,
                &model,
                None,
                Some((7, "REST record was not found".into())),
                span,
            );
            (Type::Named(model.record), value)
        };
        self.own_value(&model_value.0, &model_value.1, false);
        let row = self.row_pointer(&model_value.0, &model_value.1);
        let encoder = self
            .module
            .rest_encoder(handler.route, handler.method == "GET" && !handler.detail);
        Ok(self.runtime_call(
            "dever_rt_v1_wire_encode",
            vec![
                format!("ptr {row}"),
                format!("ptr @dever_type_{}", self.module.type_index(&model_value.0)),
                format!("ptr @{encoder}"),
            ],
            &Type::Text,
            false,
            None,
            span,
        ))
    }

    fn rest_write_values(
        &mut self,
        handler: &Handler,
        route: &ApiRest,
        inputs: &[Expression],
    ) -> Result<Vec<(usize, Expression)>, String> {
        let model = self.module.program.models[route.model].clone();
        let Shape::Record(fields) = self.module.program.types[model.record].shape.clone() else {
            unreachable!();
        };
        let write_fields = match handler.method {
            "POST" => &route.contract.create[..],
            "PUT" => &route.contract.replace[..],
            _ => &[],
        };
        let mut writes = Vec::new();
        for write in write_fields {
            let input = handler
                .inputs
                .iter()
                .position(|input| input.field == Some(write.field))
                .map(|index| &inputs[index]);
            let value = match &write.source {
                RestValueSource::Input => input.unwrap().clone(),
                RestValueSource::Binding { expression, .. } => {
                    self.rest_binding(expression, input)?
                }
            };
            if let RestValueSource::Binding { uses_input, .. } = write.source {
                let rendered = self.expression(&value)?;
                self.rest_text_bounds(
                    &fields[write.field].name,
                    &value.ty,
                    &rendered,
                    &model.fields[write.field].ty,
                    uses_input,
                );
            }
            writes.push((write.field, value));
        }
        Ok(writes)
    }

    fn rest_search_value(
        &mut self,
        route: &ApiRest,
        input: &Expression,
    ) -> Result<Expression, String> {
        let search = route.contract.search.as_ref().unwrap();
        let model = self.module.program.models[route.model].clone();
        let Shape::Record(fields) = self.module.program.types[model.record].shape.clone() else {
            unreachable!();
        };
        let field = &fields[search.field];
        let value = self.expression(input)?;
        let slot = self.entry_slot(&input.ty);
        self.line(format!(
            "store {} zeroinitializer, ptr {slot}",
            self.module.ty(&input.ty)
        ));
        let present = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {value}, 0",
            self.module.ty(&input.ty)
        ));
        let bind = self.label("rest_search_binding");
        let done = self.label("rest_search_bound");
        self.line(format!("br i1 {present}, label %{bind}, label %{done}"));
        self.start(&bind);
        let payload = self.temp();
        self.line(format!(
            "{payload} = extractvalue {} {value}, 1",
            self.module.ty(&input.ty)
        ));
        let local = self.rest_local(&field.ty, &payload, route.span);
        let value = self.rest_binding(&search.expression, Some(&local))?;
        let rendered = self.expression(&value)?;
        self.rest_text_bounds(
            &field.name,
            &field.ty,
            &rendered,
            &model.fields[search.field].ty,
            true,
        );
        let flag = self.temp();
        let row = self.temp();
        self.line(format!(
            "{flag} = insertvalue {} zeroinitializer, i1 true, 0",
            self.module.ty(&input.ty)
        ));
        self.line(format!(
            "{row} = insertvalue {} {flag}, {} {rendered}, 1",
            self.module.ty(&input.ty),
            self.module.ty(&field.ty)
        ));
        self.line(format!(
            "store {} {row}, ptr {slot}",
            self.module.ty(&input.ty)
        ));
        self.line(format!("br label %{done}"));
        self.start(&done);
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {slot}",
            self.module.ty(&input.ty)
        ));
        let owner = self.own_value(&input.ty, &value, true);
        let value = self.load_owned_value(&input.ty, owner);
        Ok(self.rest_local(&input.ty, &value, route.span))
    }

    fn rest_list(
        &mut self,
        route: &ApiRest,
        inputs: &[Expression],
        mut query: QueryPlan,
    ) -> Result<String, String> {
        let span = route.span;
        let model = self.module.program.models[route.model].clone();
        let Shape::Record(fields) = self.module.program.types[model.record].shape.clone() else {
            unreachable!();
        };
        query.page = self.rest_default_int(&inputs[0], 1)?;
        query.size = self.rest_default_int(&inputs[1], 20)?;
        let maximum = self.model_maximum(route.model, span);
        let page = self.expression(&query.page)?;
        let size = self.expression(&query.size)?;
        let p_slot = self.entry_slot(&Type::Int);
        let s_slot = self.entry_slot(&Type::Int);
        let offset = self.entry_slot(&Type::Int);
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_api_pagination(i64 {page}, i64 {size}, i64 {maximum}, ptr {p_slot}, ptr {s_slot}, ptr {offset}, ptr {error})"));
        self.rest_input_status(&status, &error);
        let result = Expression {
            ty: Type::Named(model.page),
            span,
            kind: ExpressionKind::Constant(Constant::Null),
        };
        let Some(search) = &route.contract.search else {
            return self.model_page(route.model, &model, &query, &result);
        };
        let input = &inputs[2];
        let value = self.expression(input)?;
        let present = self.temp();
        self.line(format!(
            "{present} = extractvalue {} {value}, 0",
            self.module.ty(&input.ty)
        ));
        let filtered = self.label("rest_search");
        let all = self.label("rest_unfiltered");
        let done = self.label("rest_list_done");
        let slot = self.entry_slot(&result.ty);
        self.line(format!("br i1 {present}, label %{filtered}, label %{all}"));
        self.start(&filtered);
        let input_value = self.temp();
        self.line(format!(
            "{input_value} = extractvalue {} {value}, 1",
            self.module.ty(&input.ty)
        ));
        let value = self.rest_local(&fields[search.field].ty, &input_value, span);
        let filter = ModelCondition::Compare {
            field: search.field,
            operator: BinaryOperator::Equal,
            value,
        };
        let mut filtered_query = query.clone();
        filtered_query.condition = Some(match query.condition.clone() {
            Some(owner) => ModelCondition::And(Box::new(owner), Box::new(filter)),
            None => filter,
        });
        let value = self.model_page(route.model, &model, &filtered_query, &result)?;
        self.line(format!(
            "store {} {value}, ptr {slot}",
            self.module.ty(&result.ty)
        ));
        self.line(format!("br label %{done}"));
        self.start(&all);
        let value = self.model_page(route.model, &model, &query, &result)?;
        self.line(format!(
            "store {} {value}, ptr {slot}",
            self.module.ty(&result.ty)
        ));
        self.line(format!("br label %{done}"));
        self.start(&done);
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {slot}",
            self.module.ty(&result.ty)
        ));
        Ok(value)
    }

    fn rest_default_int(
        &mut self,
        expression: &Expression,
        default: i64,
    ) -> Result<Expression, String> {
        let value = self.expression(expression)?;
        let present = self.temp();
        let number = self.temp();
        let selected = self.temp();
        self.line(format!("{present} = extractvalue {{ i1, i64 }} {value}, 0"));
        self.line(format!("{number} = extractvalue {{ i1, i64 }} {value}, 1"));
        self.line(format!(
            "{selected} = select i1 {present}, i64 {number}, i64 {default}"
        ));
        Ok(self.rest_local(&Type::Int, &selected, expression.span))
    }

    fn rest_require_affected(&mut self, affected: &str, span: Span) {
        let found = self.temp();
        let success = self.label("rest_deleted");
        let missing = self.label("rest_not_found");
        self.line(format!("{found} = icmp sgt i64 {affected}, 0"));
        self.line(format!("br i1 {found}, label %{success}, label %{missing}"));
        self.start(&missing);
        self.rest_fail(2, span);
        self.start(&success);
    }

    fn rest_fail(&mut self, variant: usize, span: Span) {
        let id = self
            .module
            .application
            .as_ref()
            .unwrap()
            .rest_error
            .unwrap();
        self.fail(
            &Expression {
                ty: Type::Named(id),
                span,
                kind: ExpressionKind::Variant {
                    variant,
                    arguments: Vec::new(),
                },
            },
            span,
        )
        .unwrap();
        self.line("unreachable");
    }

    fn rest_local(&mut self, ty: &Type, value: &str, span: Span) -> Expression {
        let slot = self.row_pointer(ty, value);
        let index = self.locals.len();
        self.locals.push((ty.clone(), slot));
        Expression {
            ty: ty.clone(),
            kind: ExpressionKind::Local(index),
            span,
        }
    }

    fn rest_binding(
        &mut self,
        expression: &Expression,
        input: Option<&Expression>,
    ) -> Result<Expression, String> {
        let previous = self.locals.first().cloned();
        if let Some(input) = input {
            let value = self.expression(input)?;
            let row = self.row_pointer(&input.ty, &value);
            self.locals[0] = (input.ty.clone(), row);
        }
        let value = self.expression(expression)?;
        if let Some(previous) = previous {
            self.locals[0] = previous;
        }
        Ok(self.rest_local(&expression.ty, &value, expression.span))
    }

    fn rest_text_bounds(
        &mut self,
        name: &str,
        ty: &Type,
        value: &str,
        storage: &ModelFieldType,
        source_fault: bool,
    ) {
        let ModelFieldType::Text { minimum, maximum } = storage else {
            return;
        };
        let name = self.module.wire_names(&[name.into()]);
        let text = if matches!(ty, Type::Nullable(_)) {
            self.nullable_handle(ty, value)
        } else {
            value.into()
        };
        let error = self.entry_slot_ir("{ ptr, i64 }");
        let status = self.temp();
        self.line(format!("{status} = call i32 @dever_rt_v1_api_text_bounds(ptr {name}, ptr {text}, i64 {minimum}, i64 {}, i8 {}, ptr {error})", maximum.unwrap_or(0), u8::from(maximum.is_some())));
        if source_fault {
            self.rest_input_status(&status, &error);
        } else {
            self.check_runtime_status(
                &status,
                &error,
                self.module.program.functions[self.instance.function].span,
            );
        }
    }

    fn rest_input_status(&mut self, status: &str, error: &str) {
        let failed = self.temp();
        let invalid = self.label("rest_invalid");
        let done = self.label("rest_valid");
        self.line(format!("{failed} = icmp eq i32 {status}, 1"));
        self.line(format!("br i1 {failed}, label %{invalid}, label %{done}"));
        self.start(&invalid);
        let pointer = self.temp();
        let length = self.temp();
        let buffer = self.temp();
        self.line(format!("{buffer} = load {{ ptr, i64 }}, ptr {error}"));
        self.line(format!(
            "{pointer} = extractvalue {{ ptr, i64 }} {buffer}, 0"
        ));
        self.line(format!(
            "{length} = extractvalue {{ ptr, i64 }} {buffer}, 1"
        ));
        let span = self.module.program.functions[self.instance.function].span;
        let text = self.runtime_call(
            "dever_rt_v1_text_new",
            vec![format!("ptr {pointer}"), format!("i64 {length}")],
            &Type::Text,
            false,
            None,
            span,
        );
        self.own_value(&Type::Text, &text, false);
        self.line(format!(
            "call void @dever_rt_v1_buffer_free(ptr {pointer}, i64 {length})"
        ));
        let message = self.rest_local(&Type::Text, &text, span);
        let id = self
            .module
            .application
            .as_ref()
            .unwrap()
            .rest_error
            .unwrap();
        self.fail(
            &Expression {
                ty: Type::Named(id),
                span,
                kind: ExpressionKind::Variant {
                    variant: 0,
                    arguments: vec![message],
                },
            },
            span,
        )
        .unwrap();
        self.line("unreachable");
        self.start(&done);
        self.check_runtime_status(
            status,
            error,
            self.module.program.functions[self.instance.function].span,
        );
    }
}
