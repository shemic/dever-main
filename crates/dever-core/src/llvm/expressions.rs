use super::*;

impl FunctionEmitter<'_, '_> {
    pub(super) fn atom(&mut self, atom: &Atom, ty: &Type, value: &str) -> Result<String, String> {
        if matches!(atom, Atom::Null) {
            let present = self.temp();
            self.line(format!(
                "{present} = extractvalue {} {value}, 0",
                self.module.ty(ty)
            ));
            let absent = self.temp();
            self.line(format!("{absent} = xor i1 {present}, 1"));
            return Ok(absent);
        }
        if let Type::Nullable(inner) = ty {
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
            let matched = self.atom(atom, inner, &payload)?;
            let result = self.temp();
            self.line(format!("{result} = and i1 {present}, {matched}"));
            return Ok(result);
        }
        Ok(match atom {
            Atom::Any => "1".into(),
            Atom::Bool(expected) => {
                self.compare("icmp eq", ty, value, if *expected { "1" } else { "0" })
            }
            Atom::Variant(variant) => {
                if matches!(ty, Type::Related(_)) {
                    let matched = self.temp();
                    self.line(format!(
                        "{matched} = icmp {} ptr {value}, null",
                        if *variant == 0 { "eq" } else { "ne" }
                    ));
                    return Ok(matched);
                }
                let tag = self.temp();
                self.line(format!(
                    "{tag} = extractvalue {} {value}, 0",
                    self.module.ty(ty)
                ));
                let result = self.temp();
                self.line(format!("{result} = icmp eq i32 {tag}, {variant}"));
                result
            }
            Atom::Int(low, high) => {
                let mut tests = Vec::new();
                if *low != i64::MIN {
                    tests.push(self.compare("icmp sge", ty, value, &low.to_string()));
                }
                if *high != i64::MAX {
                    tests.push(self.compare("icmp sle", ty, value, &high.to_string()));
                }
                self.reduce_bool("and", tests, "1")
            }
            Atom::Literal(literal) => {
                let first = self.guards.len();
                let expected = self.atom_constant(literal, ty);
                let equal = self.equals(ty, value, &expected)?;
                self.release_from(first);
                equal
            }
            Atom::Remainder(literals) => {
                let mut tests = Vec::new();
                for literal in literals {
                    let first = self.guards.len();
                    let expected = self.atom_constant(literal, ty);
                    let equal = self.equals(ty, value, &expected)?;
                    self.release_from(first);
                    let different = self.temp();
                    self.line(format!("{different} = xor i1 {equal}, 1"));
                    tests.push(different);
                }
                self.reduce_bool("and", tests, "1")
            }
            Atom::Decimal(low, high) => {
                let low = self.constant(&Constant::Decimal(*low));
                let high = self.constant(&Constant::Decimal(*high));
                let below = self.decimal_compare(
                    value,
                    &low,
                    BinaryOperator::GreaterEqual,
                    self.module.program.functions[self.instance.function].span,
                );
                let above = self.decimal_compare(
                    value,
                    &high,
                    BinaryOperator::LessEqual,
                    self.module.program.functions[self.instance.function].span,
                );
                self.reduce_bool("and", vec![below, above], "1")
            }
            Atom::Null => unreachable!("validated kernel domain"),
        })
    }

    fn atom_constant(&mut self, value: &Constant, ty: &Type) -> String {
        if let Constant::Text(text) = value {
            let (literal, length) = self.module.text_literal(text);
            let result = self.runtime_call(
                "dever_rt_v1_text_new",
                vec![format!("ptr {literal}"), format!("i64 {length}")],
                &Type::Text,
                false,
                None,
                self.module.program.functions[self.instance.function].span,
            );
            self.own_value(&Type::Text, &result, false);
            if *ty == Type::Uuid {
                let uuid = self.uuid_from_literal_text(
                    &result,
                    self.module.program.functions[self.instance.function].span,
                );
                self.own_value(&Type::Uuid, &uuid, false);
                uuid
            } else {
                result
            }
        } else {
            self.constant(value)
        }
    }

    fn constant(&self, constant: &Constant) -> String {
        match constant {
            Constant::Null => "zeroinitializer".into(),
            Constant::Bool(value) => u8::from(*value).to_string(),
            Constant::Int(value) => value.to_string(),
            Constant::Float(value) => format!("0x{:016X}", value.to_bits()),
            Constant::Decimal(value) => {
                let bytes = value.to_bytes();
                let low = i64::from_le_bytes(bytes[..8].try_into().unwrap());
                let high = i64::from_le_bytes(bytes[8..].try_into().unwrap());
                format!("{{ i64 {low}, i64 {high} }}")
            }
            Constant::Text(_) => unreachable!("Text needs an owned runtime handle"),
        }
    }

    pub(super) fn compare(
        &mut self,
        operation: &str,
        ty: &Type,
        left: &str,
        right: &str,
    ) -> String {
        let result = self.temp();
        self.line(format!(
            "{result} = {operation} {} {left}, {right}",
            self.module.ty(ty)
        ));
        result
    }

    pub(super) fn expression(&mut self, expression: &Expression) -> Result<String, String> {
        let first = self.guards.len();
        let mut result = self.expression_raw(expression)?;
        if contains_owned(self.module.program, &expression.ty) {
            let field_move = matches!(expression.kind, ExpressionKind::Field { .. })
                && self.moves.contains(&(expression as *const Expression))
                && crate::native::updates::place(expression).is_some();
            let borrowed = matches!(
                expression.kind,
                ExpressionKind::Field { .. }
                    | ExpressionKind::Record { .. }
                    | ExpressionKind::Variant { .. }
                    | ExpressionKind::Some(_)
            ) && !field_move
                || matches!(expression.kind, ExpressionKind::Local(_))
                    && !self.moves.contains(&(expression as *const Expression));
            let owner = self.own_value(&expression.ty, &result, borrowed);
            if borrowed {
                // A retain may return a different handle. Return the owned row
                // before releasing the temporaries from which it was cloned.
                result = self.load_owned_value(&expression.ty, owner);
            }
            self.release_between(first, owner);
        } else {
            self.release_from(first);
        }
        Ok(result)
    }

    fn expression_raw(&mut self, expression: &Expression) -> Result<String, String> {
        let result = match &expression.kind {
            ExpressionKind::Constant(Constant::Text(value)) => {
                let (literal, length) = self.module.text_literal(value);
                let text = self.runtime_call(
                    "dever_rt_v1_text_new",
                    vec![format!("ptr {literal}"), format!("i64 {length}")],
                    &Type::Text,
                    false,
                    None,
                    expression.span,
                );
                if expression.ty == Type::Uuid {
                    self.own_value(&Type::Text, &text, false);
                    self.uuid_from_literal_text(&text, expression.span)
                } else {
                    text
                }
            }
            ExpressionKind::Constant(value) => self.constant(value),
            ExpressionKind::Local(slot) => {
                let value = self.load_local(*slot);
                if self.moves.contains(&(expression as *const Expression))
                    && let Some(guard) = self.local_guards[*slot]
                {
                    self.disarm(guard);
                }
                value
            }
            ExpressionKind::Field { value, index } => {
                if self.moves.contains(&(expression as *const Expression))
                    && contains_owned(self.module.program, &expression.ty)
                    && let Some((slot, fields)) = crate::native::updates::place(expression)
                {
                    let (pointer, ty) = self.field_pointer(slot, &fields);
                    debug_assert_eq!(ty, expression.ty);
                    let result = self.temp();
                    self.line(format!(
                        "{result} = load {}, ptr {pointer}",
                        self.module.ty(&ty)
                    ));
                    self.line(format!(
                        "store {} zeroinitializer, ptr {pointer}",
                        self.module.ty(&ty)
                    ));
                    return Ok(result);
                }
                let parent = self.expression(value)?;
                let result = self.temp();
                self.line(format!(
                    "{result} = extractvalue {} {parent}, {index}",
                    self.module.ty(&value.ty)
                ));
                result
            }
            ExpressionKind::Call { target, arguments } => {
                self.call(*target, arguments, expression)?
            }
            ExpressionKind::RunCall { .. }
            | ExpressionKind::AwaitTask(_)
            | ExpressionKind::StopTask(_)
            | ExpressionKind::Group(_)
            | ExpressionKind::AwaitGroup(_)
            | ExpressionKind::StopGroup(_)
            | ExpressionKind::ParallelCall { .. }
            | ExpressionKind::BlockingCall { .. }
            | ExpressionKind::Channel { .. }
            | ExpressionKind::ChannelSend { .. }
            | ExpressionKind::ChannelReceive(_)
            | ExpressionKind::ChannelClose(_) => self.concurrency(expression)?,
            ExpressionKind::CaptureResult {
                target,
                arguments,
                choice,
                success,
            } => self.capture_result(*target, arguments, *choice, *success, expression)?,
            ExpressionKind::Fail(error) => self.fail(error, expression.span)?,
            ExpressionKind::Record { fields } => {
                let mut values = Vec::with_capacity(fields.len());
                for (index, value) in fields {
                    values.push((*index, self.expression(value)?));
                }
                let Type::Named(id) = expression.ty else {
                    unreachable!("checked record")
                };
                let Shape::Record(definition) = &self.module.program.types[id].shape else {
                    unreachable!()
                };
                let types = definition
                    .iter()
                    .map(|field| field.ty.clone())
                    .collect::<Vec<_>>();
                let mut aggregate = "undef".to_owned();
                for (index, value) in values {
                    let next = self.temp();
                    self.line(format!(
                        "{next} = insertvalue %T{id} {aggregate}, {} {value}, {index}",
                        self.module.ty(&types[index])
                    ));
                    aggregate = next;
                }
                aggregate
            }
            ExpressionKind::Variant { variant, arguments } => {
                if let Type::Related(inner) = &expression.ty {
                    if *variant == 0 {
                        return Ok("null".into());
                    }
                    let value = self.expression(&arguments[0])?;
                    let input = self.entry_slot(inner);
                    self.line(format!(
                        "store {} {value}, ptr {input}",
                        self.module.ty(inner)
                    ));
                    let related = self.protocol_call(
                        "dever_rt_v1_db_related_new",
                        vec![
                            format!("ptr @dever_type_{}", self.module.type_index(inner)),
                            format!("ptr {input}"),
                        ],
                        "ptr",
                        expression.span,
                    );
                    self.own_value(&expression.ty, &related, false);
                    return Ok(related);
                }
                let mut values = Vec::with_capacity(arguments.len());
                for value in arguments {
                    values.push(self.expression(value)?);
                }
                let Type::Named(id) = expression.ty else {
                    unreachable!("checked variant")
                };
                let Shape::Choice(variants) = &self.module.program.types[id].shape else {
                    unreachable!()
                };
                let types = variants[*variant]
                    .fields
                    .iter()
                    .map(|field| field.ty.clone())
                    .collect::<Vec<_>>();
                let alloca = self.temp();
                self.line(format!("{alloca} = alloca %T{id}"));
                self.line(format!("store %T{id} zeroinitializer, ptr {alloca}"));
                let tag = self.temp();
                self.line(format!(
                    "{tag} = getelementptr %T{id}, ptr {alloca}, i32 0, i32 0"
                ));
                self.line(format!("store i32 {variant}, ptr {tag}"));
                let payload = self.temp();
                self.line(format!(
                    "{payload} = getelementptr %T{id}, ptr {alloca}, i32 0, i32 1"
                ));
                for (index, (ty, value)) in types.iter().zip(values).enumerate() {
                    let field = self.temp();
                    self.line(format!("{field} = getelementptr %T{id}V{variant}, ptr {payload}, i32 0, i32 {index}"));
                    self.line(format!("store {} {value}, ptr {field}", self.module.ty(ty)));
                }
                let result = self.temp();
                self.line(format!("{result} = load %T{id}, ptr {alloca}"));
                result
            }
            ExpressionKind::Some(value) => {
                let inner = self.expression(value)?;
                let first = self.temp();
                self.line(format!(
                    "{first} = insertvalue {} undef, i1 1, 0",
                    self.module.ty(&expression.ty)
                ));
                let result = self.temp();
                self.line(format!(
                    "{result} = insertvalue {} {first}, {} {inner}, 1",
                    self.module.ty(&expression.ty),
                    self.module.ty(&value.ty)
                ));
                result
            }
            ExpressionKind::Unary { operator, value } => {
                let input = self.expression(value)?;
                match (operator, &value.ty) {
                    (UnaryOperator::Not, Type::Bool) => {
                        let result = self.temp();
                        self.line(format!("{result} = xor i1 {input}, 1"));
                        result
                    }
                    (UnaryOperator::Negate, Type::Float) => {
                        let result = self.temp();
                        self.line(format!("{result} = fneg double {input}"));
                        result
                    }
                    (UnaryOperator::Negate, Type::Int) => {
                        self.checked_integer("ssub", "0", &input, expression.span)
                    }
                    (UnaryOperator::Negate, Type::Decimal) => {
                        let parts = self.decimal_parts(&input);
                        self.runtime_call(
                            "dever_rt_v1_decimal_neg",
                            parts,
                            &Type::Decimal,
                            false,
                            None,
                            expression.span,
                        )
                    }
                    _ => unreachable!("checked unary type"),
                }
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => self.binary(left, *operator, right, expression.span)?,
            ExpressionKind::Promote(value) => {
                let input = self.expression(value)?;
                self.runtime_call(
                    "dever_rt_v1_decimal_from_int",
                    vec![format!("i64 {input}")],
                    &Type::Decimal,
                    false,
                    None,
                    expression.span,
                )
            }
            ExpressionKind::Intrinsic {
                operation,
                handler,
                arguments,
            } => self.intrinsic(*operation, *handler, arguments, expression),
            ExpressionKind::List(values) => self.list_literal(values, expression),
            ExpressionKind::Map(entries) => self.map_literal(entries, expression),
            ExpressionKind::Collection {
                operation,
                handler,
                sequence,
                arguments,
            } => self.collection(*operation, *handler, *sequence, arguments, expression),
            ExpressionKind::ModelOperation { model, operation } => {
                self.model_operation(*model, operation, expression)?
            }
            ExpressionKind::JobEnqueue {
                target,
                arguments,
                scheduled,
            } => self.enqueue_job(*target, arguments, *scheduled, expression.span)?,
            _ => unreachable!("validated kernel expression"),
        };
        Ok(result)
    }

    fn call(
        &mut self,
        target: CallTarget,
        arguments: &[CallArgument],
        expression: &Expression,
    ) -> Result<String, String> {
        let instance = self.call_instance(target, arguments);
        let (status, result) =
            self.invoke(&instance, arguments, &expression.ty, expression.span)?;
        let failed = self.temp();
        self.line(format!("{failed} = icmp ne i32 {status}, 0"));
        let fault = self.label("call_fault");
        let success = self.label("call_ok");
        self.line(format!("br i1 {failed}, label %{fault}, label %{success}"));
        self.start(&fault);
        self.append_frame(expression.span);
        self.exit(&status);
        self.start(&success);
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {result}",
            self.module.ty(&expression.ty)
        ));
        Ok(value)
    }

    pub(super) fn append_frame(&mut self, span: Span) {
        let location = self.module.location(span);
        let depth_ptr = self.temp();
        self.line(format!(
            "{depth_ptr} = getelementptr %dever.fault, ptr %fault, i32 0, i32 2"
        ));
        let depth = self.temp();
        self.line(format!("{depth} = load i32, ptr {depth_ptr}"));
        let frame = self.temp();
        self.line(format!(
            "{frame} = getelementptr %dever.fault, ptr %fault, i32 0, i32 3, i32 {depth}"
        ));
        self.line(format!("store i32 {location}, ptr {frame}"));
        let next = self.temp();
        self.line(format!("{next} = add i32 {depth}, 1"));
        self.line(format!("store i32 {next}, ptr {depth_ptr}"));
    }

    pub(super) fn write_fault(&mut self, code: &str, span: Span) {
        let location = self.module.location(span);
        for (index, value) in [
            (0, code.to_owned()),
            (1, location.to_string()),
            (2, "0".into()),
        ] {
            let pointer = self.temp();
            self.line(format!(
                "{pointer} = getelementptr %dever.fault, ptr %fault, i32 0, i32 {index}"
            ));
            self.line(format!("store i32 {value}, ptr {pointer}"));
        }
        self.exit("1");
    }

    pub(super) fn checked_integer(
        &mut self,
        operation: &str,
        left: &str,
        right: &str,
        span: Span,
    ) -> String {
        let pair = self.temp();
        self.line(format!("{pair} = call {{ i64, i1 }} @llvm.{operation}.with.overflow.i64(i64 {left}, i64 {right})"));
        let value = self.temp();
        self.line(format!("{value} = extractvalue {{ i64, i1 }} {pair}, 0"));
        let overflow = self.temp();
        self.line(format!("{overflow} = extractvalue {{ i64, i1 }} {pair}, 1"));
        let fault = self.label("overflow");
        let success = self.label("arithmetic_ok");
        self.line(format!(
            "br i1 {overflow}, label %{fault}, label %{success}"
        ));
        self.start(&fault);
        self.write_fault(&(FaultCode::Overflow as i32).to_string(), span);
        self.start(&success);
        value
    }

    fn integer_division(&mut self, operation: &str, left: &str, right: &str, span: Span) -> String {
        let zero = self.compare("icmp eq", &Type::Int, right, "0");
        let minimum = self.compare("icmp eq", &Type::Int, left, &i64::MIN.to_string());
        let negative_one = self.compare("icmp eq", &Type::Int, right, "-1");
        let overflow = self.temp();
        self.line(format!("{overflow} = and i1 {minimum}, {negative_one}"));
        let invalid = self.temp();
        self.line(format!("{invalid} = or i1 {zero}, {overflow}"));
        let fault = self.label("division_fault");
        let success = self.label("division_ok");
        self.line(format!("br i1 {invalid}, label %{fault}, label %{success}"));
        self.start(&fault);
        let code = self.temp();
        self.line(format!(
            "{code} = select i1 {zero}, i32 {}, i32 {}",
            FaultCode::DivisionByZero as i32,
            FaultCode::Overflow as i32
        ));
        self.write_fault(&code, span);
        self.start(&success);
        let result = self.temp();
        self.line(format!("{result} = {operation} i64 {left}, {right}"));
        result
    }

    fn decimal_compare(
        &mut self,
        left: &str,
        right: &str,
        operation: BinaryOperator,
        span: Span,
    ) -> String {
        let mut arguments = self.decimal_parts(left);
        arguments.extend(self.decimal_parts(right));
        let order = self.runtime_call(
            "dever_rt_v1_decimal_compare",
            arguments,
            &Type::Int,
            false,
            Some("i32"),
            span,
        );
        let comparison = match operation {
            BinaryOperator::Less => "slt",
            BinaryOperator::LessEqual => "sle",
            BinaryOperator::Greater => "sgt",
            BinaryOperator::GreaterEqual => "sge",
            _ => unreachable!("ordered Decimal comparison"),
        };
        let result = self.temp();
        self.line(format!("{result} = icmp {comparison} i32 {order}, 0"));
        result
    }

    fn binary(
        &mut self,
        left: &Expression,
        operation: BinaryOperator,
        right: &Expression,
        span: Span,
    ) -> Result<String, String> {
        let lhs = self.expression(left)?;
        if matches!(operation, BinaryOperator::And | BinaryOperator::Or) {
            let first = self.block.clone();
            let rhs_block = self.label("short_rhs");
            let done = self.label("short_done");
            if operation == BinaryOperator::And {
                self.line(format!("br i1 {lhs}, label %{rhs_block}, label %{done}"));
            } else {
                self.line(format!("br i1 {lhs}, label %{done}, label %{rhs_block}"));
            }
            self.start(&rhs_block);
            let rhs = self.expression(right)?;
            let last = self.block.clone();
            self.line(format!("br label %{done}"));
            self.start(&done);
            let result = self.temp();
            self.line(format!(
                "{result} = phi i1 [{lhs}, %{first}], [{rhs}, %{last}]"
            ));
            return Ok(result);
        }
        let rhs = self.expression(right)?;
        let result = match operation {
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                let equal = self.equals(&left.ty, &lhs, &rhs)?;
                if operation == BinaryOperator::Equal {
                    equal
                } else {
                    let different = self.temp();
                    self.line(format!("{different} = xor i1 {equal}, 1"));
                    different
                }
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if matches!(left.ty, Type::Decimal) {
                    return Ok(self.decimal_compare(&lhs, &rhs, operation, span));
                }
                if matches!(left.ty, Type::Text) {
                    let order = self.runtime_call(
                        "dever_rt_v1_text_compare",
                        vec![format!("ptr {lhs}"), format!("ptr {rhs}")],
                        &Type::Int,
                        false,
                        Some("i32"),
                        span,
                    );
                    let comparison = match operation {
                        BinaryOperator::Less => "slt",
                        BinaryOperator::LessEqual => "sle",
                        BinaryOperator::Greater => "sgt",
                        BinaryOperator::GreaterEqual => "sge",
                        _ => unreachable!(),
                    };
                    let result = self.temp();
                    self.line(format!("{result} = icmp {comparison} i32 {order}, 0"));
                    return Ok(result);
                }
                let comparison = match (&left.ty, operation) {
                    (
                        Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration,
                        BinaryOperator::Less,
                    ) => "icmp slt",
                    (
                        Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration,
                        BinaryOperator::LessEqual,
                    ) => "icmp sle",
                    (
                        Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration,
                        BinaryOperator::Greater,
                    ) => "icmp sgt",
                    (
                        Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration,
                        BinaryOperator::GreaterEqual,
                    ) => "icmp sge",
                    (Type::Float, BinaryOperator::Less) => "fcmp olt",
                    (Type::Float, BinaryOperator::LessEqual) => "fcmp ole",
                    (Type::Float, BinaryOperator::Greater) => "fcmp ogt",
                    (Type::Float, BinaryOperator::GreaterEqual) => "fcmp oge",
                    _ => unreachable!("checked ordered comparison"),
                };
                self.compare(comparison, &left.ty, &lhs, &rhs)
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::IntegerDivide
            | BinaryOperator::Remainder => match &left.ty {
                Type::Int => match operation {
                    BinaryOperator::Add => self.checked_integer("sadd", &lhs, &rhs, span),
                    BinaryOperator::Subtract => self.checked_integer("ssub", &lhs, &rhs, span),
                    BinaryOperator::Multiply => self.checked_integer("smul", &lhs, &rhs, span),
                    BinaryOperator::IntegerDivide => {
                        self.integer_division("sdiv", &lhs, &rhs, span)
                    }
                    BinaryOperator::Remainder => self.integer_division("srem", &lhs, &rhs, span),
                    _ => unreachable!("checked Int operation"),
                },
                Type::Float => {
                    let operation = match operation {
                        BinaryOperator::Add => "fadd",
                        BinaryOperator::Subtract => "fsub",
                        BinaryOperator::Multiply => "fmul",
                        BinaryOperator::Divide => "fdiv",
                        BinaryOperator::Remainder => "frem",
                        _ => unreachable!("checked Float operation"),
                    };
                    let result = self.temp();
                    self.line(format!("{result} = {operation} double {lhs}, {rhs}"));
                    result
                }
                Type::Decimal => {
                    let symbol = match operation {
                        BinaryOperator::Add => "decimal_add",
                        BinaryOperator::Subtract => "decimal_sub",
                        BinaryOperator::Multiply => "decimal_mul",
                        BinaryOperator::Divide => "decimal_div",
                        _ => unreachable!("checked Decimal operation"),
                    };
                    let mut arguments = self.decimal_parts(&lhs);
                    arguments.extend(self.decimal_parts(&rhs));
                    self.runtime_call(
                        &format!("dever_rt_v1_{symbol}"),
                        arguments,
                        &Type::Decimal,
                        false,
                        None,
                        span,
                    )
                }
                Type::Text => self.runtime_call(
                    "dever_rt_v1_text_concat",
                    vec![format!("ptr {lhs}"), format!("ptr {rhs}")],
                    &Type::Text,
                    false,
                    None,
                    span,
                ),
                _ => unreachable!("checked arithmetic type"),
            },
            BinaryOperator::And | BinaryOperator::Or => unreachable!(),
        };
        Ok(result)
    }

    pub(super) fn equals(&mut self, ty: &Type, left: &str, right: &str) -> Result<String, String> {
        match ty {
            Type::Bool | Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration => {
                Ok(self.compare("icmp eq", ty, left, right))
            }
            Type::Float => Ok(self.compare("fcmp oeq", ty, left, right)),
            Type::Decimal => {
                let mut arguments = self.decimal_parts(left);
                arguments.extend(self.decimal_parts(right));
                let raw = self.temp();
                self.line(format!(
                    "{raw} = call i8 @dever_rt_v1_decimal_equal({})",
                    arguments.join(", ")
                ));
                let equal = self.temp();
                self.line(format!("{equal} = icmp ne i8 {raw}, 0"));
                Ok(equal)
            }
            Type::Text
            | Type::Json
            | Type::Id
            | Type::Uuid
            | Type::Bytes
            | Type::List(_)
            | Type::Related(_)
            | Type::Map(_, _) => {
                let kind = match ty {
                    Type::Text | Type::Json | Type::Id => "text",
                    Type::Uuid => "uuid",
                    Type::Bytes => "bytes",
                    Type::List(_) => "list",
                    Type::Related(_) => "db_related",
                    Type::Map(_, _) => "map",
                    _ => unreachable!(),
                };
                let raw = self.temp();
                self.line(format!(
                    "{raw} = call i8 @dever_rt_v1_{kind}_equal(ptr {left}, ptr {right})"
                ));
                let equal = self.temp();
                self.line(format!("{equal} = icmp ne i8 {raw}, 0"));
                Ok(equal)
            }
            Type::Nullable(inner) => {
                let left_present = self.temp();
                self.line(format!(
                    "{left_present} = extractvalue {} {left}, 0",
                    self.module.ty(ty)
                ));
                let right_present = self.temp();
                self.line(format!(
                    "{right_present} = extractvalue {} {right}, 0",
                    self.module.ty(ty)
                ));
                let same_presence =
                    self.compare("icmp eq", &Type::Bool, &left_present, &right_present);
                let both = self.temp();
                self.line(format!("{both} = and i1 {left_present}, {right_present}"));
                let skip = self.block.clone();
                let compare = self.label("some_eq");
                let done = self.label("nullable_eq");
                self.line(format!("br i1 {both}, label %{compare}, label %{done}"));
                self.start(&compare);
                let lhs = self.temp();
                self.line(format!(
                    "{lhs} = extractvalue {} {left}, 1",
                    self.module.ty(ty)
                ));
                let rhs = self.temp();
                self.line(format!(
                    "{rhs} = extractvalue {} {right}, 1",
                    self.module.ty(ty)
                ));
                let equal = self.equals(inner, &lhs, &rhs)?;
                let compared = self.block.clone();
                self.line(format!("br label %{done}"));
                self.start(&done);
                let result = self.temp();
                self.line(format!(
                    "{result} = phi i1 [{same_presence}, %{skip}], [{equal}, %{compared}]"
                ));
                Ok(result)
            }
            Type::Named(id) if self.module.program.types[*id].kind == DefinitionKind::ModelId => {
                Ok(self.compare("icmp eq", ty, left, right))
            }
            Type::Named(id) => match &self.module.program.types[*id].shape {
                Shape::Record(fields) => {
                    let types = fields
                        .iter()
                        .map(|field| field.ty.clone())
                        .collect::<Vec<_>>();
                    self.equals_fields(&types, ty, left, right)
                }
                Shape::Choice(variants) => {
                    let types = variants
                        .iter()
                        .map(|variant| {
                            variant
                                .fields
                                .iter()
                                .map(|field| field.ty.clone())
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    self.equals_choice(*id, &types, left, right)
                }
            },
            Type::Outputs(fields) => {
                let types = fields
                    .iter()
                    .map(|field| field.ty.clone())
                    .collect::<Vec<_>>();
                self.equals_fields(&types, ty, left, right)
            }
            Type::MapEntry(key, value) => self.equals_fields(
                &[key.as_ref().clone(), value.as_ref().clone()],
                ty,
                left,
                right,
            ),
            Type::Unit => Ok("1".into()),
            // Resources are not source-comparable. Internal collection descriptors
            // still need a total callback for their concrete handle storage.
            Type::File
            | Type::Secret
            | Type::ClientTls
            | Type::ServerTls
            | Type::HttpClient
            | Type::HttpReply
            | Type::WebSocket
            | Type::Upload
            | Type::Stream(_)
            | Type::AsyncStream(_)
            | Type::RowStream(_)
            | Type::Socket
            | Type::Listener
            | Type::Task(_)
            | Type::Group
            | Type::Channel(_) => Ok(self.compare("icmp eq", ty, left, right)),
        }
    }

    fn equals_fields(
        &mut self,
        fields: &[Type],
        ty: &Type,
        left: &str,
        right: &str,
    ) -> Result<String, String> {
        let mut checks = Vec::new();
        for (index, field) in fields.iter().enumerate() {
            let lhs = self.temp();
            self.line(format!(
                "{lhs} = extractvalue {} {left}, {index}",
                self.module.ty(ty)
            ));
            let rhs = self.temp();
            self.line(format!(
                "{rhs} = extractvalue {} {right}, {index}",
                self.module.ty(ty)
            ));
            checks.push(self.equals(field, &lhs, &rhs)?);
        }
        Ok(self.reduce_bool("and", checks, "1"))
    }

    fn equals_choice(
        &mut self,
        id: usize,
        variants: &[Vec<Type>],
        left: &str,
        right: &str,
    ) -> Result<String, String> {
        let left_tag = self.temp();
        self.line(format!("{left_tag} = extractvalue %T{id} {left}, 0"));
        let right_tag = self.temp();
        self.line(format!("{right_tag} = extractvalue %T{id} {right}, 0"));
        let same = self.temp();
        self.line(format!("{same} = icmp eq i32 {left_tag}, {right_tag}"));
        let mismatch = self.label("choice_mismatch");
        let dispatch = self.label("choice_dispatch");
        let done = self.label("choice_done");
        self.line(format!(
            "br i1 {same}, label %{dispatch}, label %{mismatch}"
        ));
        self.start(&mismatch);
        self.line(format!("br label %{done}"));
        self.start(&dispatch);
        let arms = variants
            .iter()
            .enumerate()
            .map(|(index, _)| format!("i32 {index}, label %choice{id}_v{index}_{}", self.next))
            .collect::<Vec<_>>();
        let base = self.next;
        self.next += 1;
        self.line(format!(
            "switch i32 {left_tag}, label %{mismatch} [{}]",
            arms.join(" ")
        ));
        let mut results = vec![("0".to_owned(), mismatch)];
        for (variant, fields) in variants.iter().enumerate() {
            self.start(&format!("choice{id}_v{variant}_{base}"));
            let lhs = self.temp();
            self.line(format!("{lhs} = alloca %T{id}"));
            self.line(format!("store %T{id} {left}, ptr {lhs}"));
            let rhs = self.temp();
            self.line(format!("{rhs} = alloca %T{id}"));
            self.line(format!("store %T{id} {right}, ptr {rhs}"));
            let lhs_payload = self.temp();
            self.line(format!(
                "{lhs_payload} = getelementptr %T{id}, ptr {lhs}, i32 0, i32 1"
            ));
            let rhs_payload = self.temp();
            self.line(format!(
                "{rhs_payload} = getelementptr %T{id}, ptr {rhs}, i32 0, i32 1"
            ));
            let mut checks = Vec::new();
            for (index, ty) in fields.iter().enumerate() {
                let left_ptr = self.temp();
                self.line(format!("{left_ptr} = getelementptr %T{id}V{variant}, ptr {lhs_payload}, i32 0, i32 {index}"));
                let right_ptr = self.temp();
                self.line(format!("{right_ptr} = getelementptr %T{id}V{variant}, ptr {rhs_payload}, i32 0, i32 {index}"));
                let left_value = self.temp();
                self.line(format!(
                    "{left_value} = load {}, ptr {left_ptr}",
                    self.module.ty(ty)
                ));
                let right_value = self.temp();
                self.line(format!(
                    "{right_value} = load {}, ptr {right_ptr}",
                    self.module.ty(ty)
                ));
                checks.push(self.equals(ty, &left_value, &right_value)?);
            }
            let equal = self.reduce_bool("and", checks, "1");
            let end = self.block.clone();
            self.line(format!("br label %{done}"));
            results.push((equal, end));
        }
        self.start(&done);
        let result = self.temp();
        let incoming = results
            .iter()
            .map(|(value, block)| format!("[{value}, %{block}]"))
            .collect::<Vec<_>>()
            .join(", ");
        self.line(format!("{result} = phi i1 {incoming}"));
        Ok(result)
    }
}
