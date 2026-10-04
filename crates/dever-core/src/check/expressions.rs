use crate::diagnostic::Diagnostic;
use crate::hir::{
    CallArgument, CallTarget, Expression as Value, ExpressionKind as Expr, HandlerTarget,
};
use crate::intrinsic::Intrinsic;
use crate::source::Span;
use crate::syntax::{self, Expression, ExpressionKind as Ast, Literal};
use crate::types::{Field, HandlerSignature, Parameter, Shape, Type, output_type};

use super::{Checked, body::Body, constants, path_name};

pub(super) struct CheckedCall {
    pub target: CallTarget,
    pub arguments: Vec<CallArgument>,
    pub outputs: Vec<Field>,
    pub pure: bool,
}

impl Body<'_, '_> {
    pub(super) fn expression(
        &mut self,
        source: &Expression,
        expected: Option<&Type>,
    ) -> Checked<Value> {
        let mut value = match &source.kind {
            Ast::Literal(literal) => constants::literal(literal, expected, source.span)?,
            Ast::Group(inner) => self.expression(inner, expected)?,
            Ast::Name(path) => self.name(path, source.span)?,
            Ast::Field { value, name } => {
                if name.text == "options" {
                    if let Ast::Name(path) = &value.kind {
                        if let Some(choice) =
                            self.context.model_choice(&path_name(path), value.span)?
                        {
                            Value {
                                kind: Expr::ChoiceOptions { choice },
                                ty: Type::Map(Box::new(Type::Named(choice)), Box::new(Type::Text)),
                                span: source.span,
                            }
                        } else {
                            let value = self.expression(value, None)?;
                            self.field(value, name)?
                        }
                    } else {
                        let value = self.expression(value, None)?;
                        self.field(value, name)?
                    }
                } else {
                    let value = self.expression(value, None)?;
                    self.field(value, name)?
                }
            }
            Ast::Unary { operator, value } => {
                self.unary(*operator, value, expected, source.span)?
            }
            Ast::Binary {
                left,
                operator,
                right,
            } => self.binary(left, *operator, right, expected, source.span)?,
            Ast::Record { name, fields } => self.record(name, fields, source.span)?,
            Ast::List(elements) => self.list(elements, expected, source.span)?,
            Ast::Map(entries) => self.map(entries, expected, source.span)?,
            Ast::Call {
                function,
                arguments,
            } => self.call(function, arguments, expected, source.span)?,
            Ast::Fail(error) => self.fail(error, source.span)?,
            Ast::CaptureResult(call) => self.capture_result(call, expected, source.span)?,
            Ast::Contextual(operation) => self.contextual(operation, source.span)?,
        };
        value.span = source.span;
        if nested_affine(&value.ty) {
            return Err(Diagnostic::error(
                "C005",
                "Task and Group values cannot be stored in another value",
                source.span,
            ));
        }
        match expected {
            Some(expected) => self.coerce(value, expected),
            None => Ok(value),
        }
    }

    pub(super) fn coerce(&self, value: Value, expected: &Type) -> Checked<Value> {
        if &value.ty == expected {
            return Ok(value);
        }
        if let Type::Nullable(base) = expected
            && !matches!(value.ty, Type::Nullable(_))
        {
            let inner = self.coerce(value, base)?;
            return Ok(Value {
                span: inner.span,
                ty: expected.clone(),
                kind: Expr::Some(Box::new(inner)),
            });
        }
        if value.ty == Type::Int && expected == &Type::Decimal {
            return Ok(constants::promote(value));
        }
        Err(Diagnostic::error(
            "C005",
            format!(
                "expected {}, found {}",
                expected.label(self.context.types),
                value.ty.label(self.context.types)
            ),
            value.span,
        ))
    }

    fn name(&mut self, path: &syntax::Path, span: Span) -> Checked<Value> {
        if path[0].text == "setting"
            && self.context.symbols.packages[self.context.owner]
                .layout
                .role()
                == Some(crate::source::SourceRole::Adapter)
        {
            let (id, function) = self
                .context
                .functions
                .iter()
                .enumerate()
                .find(|(_, function)| {
                    function.owner == self.context.owner && function.setting.is_some()
                })
                .ok_or_else(|| {
                    Diagnostic::error(
                        "C006",
                        "setting is available only in its declaring Adapter",
                        span,
                    )
                })?;
            let mut value = Value {
                kind: Expr::Call {
                    target: CallTarget::Function(id),
                    arguments: Vec::new(),
                },
                ty: Type::Named(function.setting.unwrap()),
                span,
            };
            for name in &path[1..] {
                value = self.field(value, name)?;
            }
            return Ok(value);
        }
        if path.len() > 1 && path.last().is_some_and(|name| name.text == "options") {
            let choice_name = path_name(&path[..path.len() - 1]);
            if let Some(choice) = self.context.model_choice(&choice_name, span)? {
                return Ok(self.choice_options(choice, span));
            }
        }
        if path.len() == 1 && self.handlers.contains_key(&path[0].text) {
            return Err(Diagnostic::error(
                "C005",
                "handler parameters are not runtime values",
                span,
            ));
        }
        if path.len() > 1
            && let Some((id, variant)) = self.context.variant(&path_name(path), span)?
        {
            return self.construct_variant(id, variant, &[], span);
        }
        if self.locals.contains_key(&path[0].text) {
            let local = self.read_local(&path[0].text, span)?;
            let mut value = Value {
                kind: local.known.clone().unwrap_or(Expr::Local(local.slot)),
                ty: local.ty.clone(),
                span,
            };
            for name in &path[1..] {
                value = self.field(value, name)?;
            }
            return Ok(value);
        }
        if let Some((id, variant)) = self.context.variant(&path_name(path), span)? {
            return self.construct_variant(id, variant, &[], span);
        }
        Err(Diagnostic::error(
            "C004",
            format!("unknown or uninitialized local '{}'", path_name(path)),
            span,
        ))
    }

    fn choice_options(&self, choice: usize, span: Span) -> Value {
        Value {
            kind: Expr::ChoiceOptions { choice },
            ty: Type::Map(Box::new(Type::Named(choice)), Box::new(Type::Text)),
            span,
        }
    }

    pub(super) fn field_type(&self, ty: &Type, name: &syntax::Name) -> Checked<(usize, Type)> {
        if let Type::Named(id) = ty
            && !self.context.owns_private_fields(*id)
            && let Shape::Record(fields) = &self.context.types[*id].shape
            && fields
                .iter()
                .any(|field| field.name == name.text && field.private)
        {
            return Err(Diagnostic::error(
                "C006",
                "private field is accessible only to its owner (Model: owning domain App)",
                name.span,
            ));
        }
        let fields = self.context.value_fields(ty).ok_or_else(|| {
            Diagnostic::error(
                "C005",
                "field access requires an initialized record, MapEntry or named outputs",
                name.span,
            )
        })?;
        fields
            .into_iter()
            .enumerate()
            .find(|(_, field)| field.name == name.text)
            .map(|(index, field)| (index, field.ty))
            .ok_or_else(|| {
                Diagnostic::error("C004", format!("unknown field '{}'", name.text), name.span)
            })
    }

    fn field(&self, value: Value, name: &syntax::Name) -> Checked<Value> {
        let (index, ty) = self.field_type(&value.ty, name)?;
        Ok(Value {
            kind: Expr::Field {
                value: Box::new(value),
                index,
            },
            ty,
            span: name.span,
        })
    }

    fn call(
        &mut self,
        function: &Expression,
        arguments: &[Expression],
        expected: Option<&Type>,
        span: Span,
    ) -> Checked<Value> {
        let Ast::Name(path) = &ungroup(function).kind else {
            return Err(Diagnostic::error(
                "C005",
                "only named functions are callable",
                function.span,
            ));
        };
        let name = path_name(path);
        if let Some(value) = self.upload_call(&name, arguments, span) {
            return value;
        }
        if matches!(name.as_str(), "dever.job.enqueue" | "dever.job.enqueue_at") {
            return self.enqueue_job(arguments, name.ends_with("_at"), span);
        }
        if self.context.is_test()
            && path.len() == 1
            && let Some(assertion) = self.test_assertion(&name, arguments, span)?
        {
            return Ok(assertion);
        }
        if path.len() == 1
            && let Some(handler) = self.handlers.get(&path[0].text)
        {
            let parameter = handler.parameter;
            let signature = handler.signature.clone();
            let checked = self.handler_call(parameter, &signature, arguments, span)?;
            return Ok(Value {
                kind: Expr::Call {
                    target: checked.target,
                    arguments: checked.arguments,
                },
                ty: output_type(&checked.outputs),
                span,
            });
        }
        if let Some((id, variant)) = self.context.variant(&name, function.span)? {
            return self.construct_variant(id, variant, arguments, span);
        }
        if let Some(value) = self.model_call(&name, arguments, span)? {
            return Ok(value);
        }
        if path.len() == 1 && self.locals.contains_key(&path[0].text) {
            return Err(Diagnostic::error(
                "C005",
                "local values are not callable",
                function.span,
            ));
        }
        let local_function = self.context.has_function(&name, arguments.len());
        if !local_function {
            if let Some(result) = self.named_concurrency(&name, arguments, span) {
                if self.context.symbols.packages[self.context.owner]
                    .layout
                    .role()
                    == Some(crate::source::SourceRole::Job)
                {
                    return Err(Diagnostic::error(
                        "C006",
                        "Job entries call only their same-domain App",
                        span,
                    ));
                }
                return result;
            }
            if let Some(operation) = super::collections::operation(&name) {
                if self.context.symbols.packages[self.context.owner]
                    .layout
                    .role()
                    == Some(crate::source::SourceRole::Job)
                {
                    return Err(Diagnostic::error(
                        "C006",
                        "Job entries call only their same-domain App",
                        span,
                    ));
                }
                return self.collection(operation, arguments, expected, span);
            }
        }
        if let Some(operation) = Intrinsic::from_name(&name) {
            if matches!(operation, Intrinsic::AuthOwnsUser) {
                let [argument] = arguments else {
                    return Err(Diagnostic::error(
                        "C005",
                        "dever.auth.owns_user expects one Model id",
                        span,
                    ));
                };
                let argument = self.expression(argument, None)?;
                let model_id = matches!(argument.ty, Type::Named(id)
                    if self.context.types[id].kind == crate::types::DefinitionKind::ModelId);
                if !model_id {
                    return Err(Diagnostic::error(
                        "C005",
                        "dever.auth.owns_user expects one Model id",
                        argument.span,
                    ));
                }
                return Ok(Value {
                    kind: Expr::Intrinsic {
                        operation,
                        handler: None,
                        arguments: vec![argument],
                    },
                    ty: Type::Bool,
                    span,
                });
            }
            if self.context.symbols.packages[self.context.owner]
                .layout
                .role()
                == Some(crate::source::SourceRole::Job)
            {
                return Err(Diagnostic::error(
                    "C006",
                    "Job entries call only their same-domain App",
                    span,
                ));
            }
            let mut signature = super::intrinsics::signature(operation, self.context, span)?;
            let shared_context = if matches!(
                operation,
                Intrinsic::HttpServe
                    | Intrinsic::HttpServeLive
                    | Intrinsic::HttpServeTls
                    | Intrinsic::HttpServeLiveTls
            ) && arguments.len() == signature.parameters.len() + 2
            {
                let context = self.expression(arguments.last().unwrap(), None)?;
                if !context.ty.transferable(self.context.types) {
                    return Err(Diagnostic::error(
                        "C005",
                        "HTTP handler context must be transferable",
                        context.span,
                    ));
                }
                let handler = signature.handler.as_mut().expect("HTTP handler");
                handler.parameters.push(context.ty.clone());
                handler.bounds.push(Default::default());
                Some(context)
            } else {
                None
            };
            let count = signature.parameters.len() + usize::from(signature.handler.is_some());
            if count + usize::from(shared_context.is_some()) != arguments.len() {
                return Err(Diagnostic::error(
                    "C005",
                    format!("'{name}' expects {count} argument(s)"),
                    span,
                ));
            }
            let (handler, arguments) = match &signature.handler {
                Some(expected) => (
                    Some(self.handler_reference(&arguments[0], expected)?),
                    &arguments[1..],
                ),
                None => (None, arguments),
            };
            let mut arguments: Vec<_> = arguments
                .iter()
                .zip(signature.parameters)
                .map(|(source, ty)| self.expression(source, Some(&ty)))
                .collect::<Checked<_>>()?;
            arguments.extend(shared_context);
            return Ok(Value {
                kind: Expr::Intrinsic {
                    operation,
                    handler,
                    arguments,
                },
                ty: signature.output,
                span,
            });
        }
        let id = self
            .context
            .function(&name, arguments.len(), function.span)?;
        // The standard getter retains absence; only an explicit nominal context
        // selects the trusted ID representation. Route/provider validation follows.
        if let Some(Type::Nullable(inner)) = expected
            && let Type::Named(model_id) = inner.as_ref()
            && self.context.types[*model_id].kind == crate::types::DefinitionKind::ModelId
            && let Some(operation) = match name.as_str() {
                "dever.auth.user_id" => Some(Intrinsic::AuthUserId),
                "dever.auth.tenant_id" => Some(Intrinsic::AuthTenantId),
                _ => None,
            }
        {
            return Ok(Value {
                kind: Expr::Intrinsic {
                    operation,
                    handler: None,
                    arguments: Vec::new(),
                },
                ty: expected.expect("matched explicit nullable type").clone(),
                span,
            });
        }
        let checked = self.function_call(id, arguments)?;
        Ok(Value {
            kind: Expr::Call {
                target: checked.target,
                arguments: checked.arguments,
            },
            ty: output_type(&checked.outputs),
            span,
        })
    }

    fn test_assertion(
        &mut self,
        name: &str,
        arguments: &[Expression],
        span: Span,
    ) -> Checked<Option<Value>> {
        let mut output = Type::Unit;
        let (operation, checked) = match name {
            "secret" => {
                let [input] = arguments else {
                    return Err(Diagnostic::error(
                        "C005",
                        "secret expects one Text argument",
                        span,
                    ));
                };
                output = Type::Secret;
                (
                    Intrinsic::TestSecret,
                    vec![self.expression(input, Some(&Type::Text))?],
                )
            }
            "assert" => {
                let [condition] = arguments else {
                    return Err(Diagnostic::error(
                        "C005",
                        "'assert' expects one Bool argument",
                        span,
                    ));
                };
                (
                    Intrinsic::TestAssert,
                    vec![self.expression(condition, Some(&Type::Bool))?],
                )
            }
            "assert_eq" => {
                let [actual, expected] = arguments else {
                    return Err(Diagnostic::error(
                        "C005",
                        "'assert_eq' expects two arguments",
                        span,
                    ));
                };
                let actual = self.expression(actual, None)?;
                let expected =
                    self.expression(expected, needs_context(expected).then_some(&actual.ty))?;
                if actual.ty != expected.ty {
                    return Err(Diagnostic::error(
                        "C005",
                        format!(
                            "assert_eq requires the same type on both sides; found {} and {}",
                            actual.ty.label(self.context.types),
                            expected.ty.label(self.context.types)
                        ),
                        span,
                    ));
                }
                if !actual.ty.comparable(self.context.types)
                    || !actual.ty.observable(self.context.types)
                {
                    return Err(Diagnostic::error(
                        "C005",
                        format!(
                            "assert_eq does not support {}",
                            actual.ty.label(self.context.types)
                        ),
                        span,
                    ));
                }
                (Intrinsic::TestAssertEq, vec![actual, expected])
            }
            _ => return Ok(None),
        };
        Ok(Some(Value {
            kind: Expr::Intrinsic {
                operation,
                handler: None,
                arguments: checked,
            },
            ty: output,
            span,
        }))
    }

    fn fail(&mut self, source: &Expression, span: Span) -> Checked<Value> {
        let source = ungroup(source);
        let (path, arguments) = match &source.kind {
            Ast::Name(path) => (path, &[][..]),
            Ast::Call {
                function,
                arguments,
            } => {
                let Ast::Name(path) = &ungroup(function).kind else {
                    return Err(Diagnostic::error(
                        "C005",
                        "fail requires a direct error choice variant",
                        source.span,
                    ));
                };
                (path, arguments.as_slice())
            }
            _ => {
                return Err(Diagnostic::error(
                    "C005",
                    "fail requires a direct error choice variant",
                    source.span,
                ));
            }
        };
        let Some((id, variant)) = self.context.variant(&path_name(path), source.span)? else {
            return Err(Diagnostic::error(
                "C005",
                "fail requires a qualified error choice variant",
                source.span,
            ));
        };
        let error = self.construct_failure_variant(id, variant, arguments, source.span)?;
        Ok(Value {
            kind: Expr::Fail(Box::new(error)),
            ty: Type::Unit,
            span,
        })
    }

    fn capture_result(
        &mut self,
        source: &Expression,
        expected: Option<&Type>,
        span: Span,
    ) -> Checked<Value> {
        let Some(Type::Named(choice)) = expected.map(Type::base) else {
            return Err(Diagnostic::error(
                "C005",
                "result(call) requires an expected choice type",
                span,
            ));
        };
        let Shape::Choice(variants) = &self.context.types[*choice].shape else {
            return Err(Diagnostic::error(
                "C005",
                "result(call) requires an expected choice type",
                span,
            ));
        };
        let success_variants = variants
            .iter()
            .enumerate()
            .filter(|(_, variant)| !variant.error)
            .collect::<Vec<_>>();
        let [(success, success_variant)] = success_variants.as_slice() else {
            return Err(Diagnostic::error(
                "C005",
                "a result capture choice must declare exactly one success variant",
                self.context.types[*choice].span,
            ));
        };
        let checked = self.checked_static_call(source)?;
        let outputs_match = checked.outputs.len() == success_variant.fields.len()
            && checked
                .outputs
                .iter()
                .zip(&success_variant.fields)
                .all(|(output, field)| output.ty == field.ty);
        if !outputs_match {
            return Err(Diagnostic::error(
                "C005",
                "result capture success payload must match the called function outputs",
                span,
            ));
        }
        Ok(Value {
            kind: Expr::CaptureResult {
                target: checked.target,
                arguments: checked.arguments,
                choice: *choice,
                success: *success,
            },
            ty: Type::Named(*choice),
            span,
        })
    }

    pub(super) fn checked_static_call(&mut self, source: &Expression) -> Checked<CheckedCall> {
        let Ast::Call {
            function,
            arguments,
        } = &ungroup(source).kind
        else {
            return Err(Diagnostic::error(
                "C005",
                "this operation requires a direct named function call",
                source.span,
            ));
        };
        let Ast::Name(path) = &ungroup(function).kind else {
            return Err(Diagnostic::error(
                "C005",
                "only named functions are callable",
                function.span,
            ));
        };
        if path.len() == 1 {
            if let Some(handler) = self.handlers.get(&path[0].text) {
                return self.handler_call(
                    handler.parameter,
                    &handler.signature.clone(),
                    arguments,
                    source.span,
                );
            }
            if path.len() == 1 && self.locals.contains_key(&path[0].text) {
                return Err(Diagnostic::error(
                    "C005",
                    "local values are not callable",
                    function.span,
                ));
            }
        }
        let id = self
            .context
            .function(&path_name(path), arguments.len(), function.span)?;
        self.function_call(id, arguments)
    }

    fn handler_call(
        &mut self,
        parameter: usize,
        signature: &HandlerSignature,
        arguments: &[Expression],
        span: Span,
    ) -> Checked<CheckedCall> {
        if signature.parameters.len() != arguments.len() {
            return Err(Diagnostic::error(
                "C005",
                format!("handler expects {} argument(s)", signature.parameters.len()),
                span,
            ));
        }
        let arguments = arguments
            .iter()
            .zip(&signature.parameters)
            .map(|(argument, ty)| self.expression(argument, Some(ty)).map(CallArgument::Value))
            .collect::<Checked<_>>()?;
        Ok(CheckedCall {
            target: CallTarget::Handler(HandlerTarget::Parameter(parameter)),
            arguments,
            outputs: signature.outputs.clone(),
            pure: false,
        })
    }

    fn function_call(&mut self, id: usize, arguments: &[Expression]) -> Checked<CheckedCall> {
        let function = &self.context.functions[id];
        let parameters = function.parameters.clone();
        let storage_sink = function.name == "dever.storage.put";
        if storage_sink
            && self.context.symbols.packages[self.context.owner]
                .layout
                .role()
                != Some(crate::source::SourceRole::Adapter)
        {
            return Err(Diagnostic::error(
                "C006",
                "storage.put must be called by a storage Adapter through its Port",
                function.span,
            ));
        }
        let upload_transfer = function.port.is_some() || storage_sink;
        let checked_arguments = arguments
            .iter()
            .zip(&parameters)
            .map(|(argument, parameter)| match parameter {
                Parameter::Value(ty) => {
                    if ty == &Type::Upload {
                        if !upload_transfer { return Err(Diagnostic::error("C006", "Upload ownership may transfer only through a Port to its storage Adapter", argument.span)); }
                        let Ast::Name(path) = &argument.kind else { return Err(Diagnostic::error("C005", "Upload transfer requires a single local", argument.span)); };
                        if path.len() != 1 { return Err(Diagnostic::error("C005", "Upload transfer requires a single local", argument.span)); }
                        let (slot, actual) = self.affine_local(&path[0].text, argument.span, true)?;
                        if actual != Type::Upload { return Err(Diagnostic::error("C005", "expected Upload local", argument.span)); }
                        return Ok(CallArgument::Value(Value { kind: Expr::Local(slot), ty: actual, span: argument.span }));
                    }
                    self.expression(argument, Some(ty)).map(CallArgument::Value)
                }
                Parameter::Handler(expected) => self
                    .handler_reference(argument, expected)
                    .map(CallArgument::Handler),
            })
            .collect::<Checked<_>>()?;
        Ok(CheckedCall {
            target: CallTarget::Function(id),
            arguments: checked_arguments,
            outputs: function.outputs.clone(),
            pure: function.pure,
        })
    }

    pub(super) fn handler_reference(
        &self,
        source: &Expression,
        expected: &HandlerSignature,
    ) -> Checked<HandlerTarget> {
        let (target, actual) = self.resolve_handler(source, expected.parameters.len())?;
        let inputs_match = actual.parameters == expected.parameters
            && expected
                .bounds
                .iter()
                .zip(&actual.bounds)
                .all(|(required, accepted)| crate::contracts::bounds_subset(required, accepted));
        let outputs_match = actual.outputs.len() == expected.outputs.len()
            && actual
                .outputs
                .iter()
                .zip(&expected.outputs)
                .all(|(actual, expected)| {
                    actual.name == expected.name
                        && actual.ty == expected.ty
                        && (matches!(target, HandlerTarget::Function(_))
                            || crate::contracts::bounds_subset(&actual.bounds, &expected.bounds))
                });
        // Concrete output guarantees are inferred from checked bodies by the
        // contract pass. Forwarded handlers have only their declared guarantees.
        if !inputs_match || !outputs_match {
            return Err(Diagnostic::error(
                "C005",
                "handler signature does not match the required signature",
                source.span,
            ));
        }
        Ok(target)
    }

    pub(super) fn resolve_handler(
        &self,
        source: &Expression,
        arity: usize,
    ) -> Checked<(HandlerTarget, HandlerSignature)> {
        let Ast::Name(path) = &ungroup(source).kind else {
            return Err(Diagnostic::error(
                "C005",
                "handler must be a named function or handler parameter",
                source.span,
            ));
        };
        if path.len() == 1 {
            if let Some(handler) = self.handlers.get(&path[0].text) {
                if handler.signature.parameters.len() != arity {
                    return Err(Diagnostic::error(
                        "C005",
                        format!(
                            "handler expects {} argument(s)",
                            handler.signature.parameters.len()
                        ),
                        source.span,
                    ));
                }
                return Ok((
                    HandlerTarget::Parameter(handler.parameter),
                    handler.signature.clone(),
                ));
            }
            if path.len() == 1 && self.locals.contains_key(&path[0].text) {
                return Err(Diagnostic::error(
                    "C005",
                    "local values cannot be handlers",
                    source.span,
                ));
            }
        }
        let id = self
            .context
            .function(&path_name(path), arity, source.span)?;
        let function = &self.context.functions[id];
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| match parameter {
                Parameter::Value(ty) => Ok(ty.clone()),
                Parameter::Handler(_) => Err(Diagnostic::error(
                    "C005",
                    "a handler target cannot itself require handler inputs",
                    source.span,
                )),
            })
            .collect::<Checked<Vec<_>>>()?;
        let signature = HandlerSignature {
            bounds: vec![Vec::new(); parameters.len()],
            parameters,
            outputs: function.outputs.clone(),
        };
        Ok((HandlerTarget::Function(id), signature))
    }
}

fn nested_affine(ty: &Type) -> bool {
    match ty {
        Type::Task(_) | Type::Group | Type::Upload => false,
        Type::Nullable(inner)
        | Type::List(inner)
        | Type::Stream(inner)
        | Type::AsyncStream(inner)
        | Type::RowStream(inner)
        | Type::Channel(inner) => contains_affine(inner),
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            contains_affine(key) || contains_affine(value)
        }
        Type::Outputs(fields) => fields.iter().any(|field| contains_affine(&field.ty)),
        _ => false,
    }
}

fn contains_affine(ty: &Type) -> bool {
    matches!(ty, Type::Task(_) | Type::Group | Type::Upload) || nested_affine(ty)
}

pub(super) fn needs_context(expression: &Expression) -> bool {
    match &expression.kind {
        Ast::Literal(Literal::Null) => true,
        Ast::List(elements) => elements.iter().all(needs_context),
        Ast::Map(entries) => entries.is_empty(),
        Ast::Group(inner) => needs_context(inner),
        Ast::CaptureResult(_) => true,
        _ => false,
    }
}

pub(super) fn ungroup(mut expression: &Expression) -> &Expression {
    while let Ast::Group(inner) = &expression.kind {
        expression = inner;
    }
    expression
}

pub(super) fn is_null(expression: &Expression) -> bool {
    matches!(ungroup(expression).kind, Ast::Literal(Literal::Null))
}
