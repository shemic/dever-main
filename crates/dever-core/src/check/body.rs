use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Diagnostic;
use crate::hir::{self, Atom, Binding, Constant, Domain, ExpressionKind, Function, Projection};
use crate::source::Span;
use crate::syntax::{FunctionClause, InputKind, Pattern, StatementKind};
use crate::types::{HandlerSignature, Parameter, Shape, Type};

use super::{Checked, Context, path_name};

pub(super) struct Local {
    pub slot: usize,
    pub ty: Type,
    pub initialized: bool,
    pub last_write: Option<Span>,
    pub read: bool,
    pub known: Option<ExpressionKind>,
    pub affine_consumed: bool,
}

pub(super) struct Body<'a, 'b> {
    pub context: &'a Context<'b>,
    pub locals: BTreeMap<String, Local>,
    pub handlers: BTreeMap<String, HandlerLocal>,
    pub slots: Vec<Type>,
    pub warnings: &'a mut Vec<Diagnostic>,
}

pub(super) struct HandlerLocal {
    pub parameter: usize,
    pub signature: HandlerSignature,
}

pub(super) fn expression(
    source: &crate::syntax::Expression,
    input: &Type,
    expected: &Type,
    context: &Context<'_>,
    warnings: &mut Vec<Diagnostic>,
) -> Checked<hir::Expression> {
    let mut checker = Body {
        context,
        locals: BTreeMap::new(),
        handlers: BTreeMap::new(),
        slots: Vec::new(),
        warnings,
    };
    checker.declare("input", input.clone(), true, source.span)?;
    checker.expression(source, Some(expected))
}

pub(super) fn check(
    source: &FunctionClause,
    patterns: Vec<Domain>,
    function: &Function,
    context: &Context<'_>,
    warnings: &mut Vec<Diagnostic>,
) -> Checked<hir::Clause> {
    let mut checker = Body {
        context,
        locals: BTreeMap::new(),
        handlers: BTreeMap::new(),
        slots: Vec::new(),
        warnings,
    };
    let mut bindings = Vec::new();
    let mut domains = patterns.iter();
    for (input_index, (input, parameter)) in
        source.inputs.iter().zip(&function.parameters).enumerate()
    {
        let Parameter::Value(parameter) = parameter else {
            let Parameter::Handler(signature) = parameter else {
                unreachable!()
            };
            if checker.locals.contains_key(&input.name.text)
                || checker.handlers.contains_key(&input.name.text)
            {
                return Err(Diagnostic::error(
                    "C002",
                    format!("duplicate local binding '{}'", input.name.text),
                    input.name.span,
                ));
            }
            checker.handlers.insert(
                input.name.text.clone(),
                HandlerLocal {
                    parameter: input_index,
                    signature: signature.clone(),
                },
            );
            continue;
        };
        let InputKind::Value(input_pattern) = &input.kind else {
            unreachable!("checked parameter kind")
        };
        let domain = domains.next().expect("checked value parameter domain");
        let non_null = matches!(parameter, Type::Nullable(_))
            && !domain.iter().any(|atom| matches!(atom, Atom::Null));
        let ty = if non_null {
            parameter.base().clone()
        } else {
            parameter.clone()
        };
        let slot = checker.declare(&input.name.text, ty, true, input.name.span)?;
        checker
            .locals
            .get_mut(&input.name.text)
            .expect("declared input")
            .known = match domain.as_slice() {
            [Atom::Null] => Some(ExpressionKind::Constant(Constant::Null)),
            [Atom::Bool(value)] => Some(ExpressionKind::Constant(Constant::Bool(*value))),
            [Atom::Int(lo, hi)] if lo == hi => Some(ExpressionKind::Constant(Constant::Int(*lo))),
            [Atom::Decimal(lo, hi)] if lo == hi => {
                Some(ExpressionKind::Constant(Constant::Decimal(*lo)))
            }
            [Atom::Literal(Constant::Text(value))] => {
                Some(ExpressionKind::Constant(Constant::Text(value.clone())))
            }
            _ => None,
        };
        bindings.push(Binding {
            slot,
            input: input_index,
            projection: if non_null {
                Projection::NonNull
            } else {
                Projection::Whole
            },
        });
        if let Pattern::Variant {
            name,
            arguments,
            bindings: names,
            span,
        } = input_pattern
        {
            let qualified = path_name(name);
            let (variant, fields) = if let Some((_, variant, fields)) =
                context.related_variant(&qualified, arguments, *span)?
            {
                (variant, fields)
            } else {
                let (id, variant) = context
                    .variant(&qualified, *span)?
                    .expect("checked choice pattern");
                let Shape::Choice(variants) = &context.types[id].shape else {
                    unreachable!()
                };
                (variant, variants[variant].fields.clone())
            };
            for (field, (name, payload)) in names.iter().zip(&fields).enumerate() {
                if name.text == "_" {
                    continue;
                }
                let slot = checker.declare(&name.text, payload.ty.clone(), true, name.span)?;
                bindings.push(Binding {
                    slot,
                    input: input_index,
                    projection: Projection::Payload { variant, field },
                });
            }
        }
    }
    debug_assert!(domains.next().is_none());
    let mut outputs = Vec::new();
    for (field, source) in function.outputs.iter().zip(&source.outputs) {
        outputs.push(checker.declare(&field.name, field.ty.clone(), false, source.span)?);
    }
    let mut body = Vec::new();
    let mut terminates = false;
    for (index, statement) in source.body.iter().enumerate() {
        match &statement.kind {
            StatementKind::Assign { target, value } => {
                let name = &target[0].text;
                if name == "setting"
                    && checker.context.symbols.packages[checker.context.owner]
                        .layout
                        .role()
                        == Some(crate::source::SourceRole::Adapter)
                {
                    return Err(Diagnostic::error(
                        "C006",
                        "Adapter setting is read-only and cannot be shadowed",
                        target[0].span,
                    ));
                }
                if target.len() == 1 {
                    let expected = checker.locals.get(name).map(|local| local.ty.clone());
                    let value = checker.expression(value, expected.as_ref())?;
                    if value.ty == Type::Unit {
                        return Err(Diagnostic::error(
                            "C005",
                            "a zero-output action cannot be assigned",
                            value.span,
                        ));
                    }
                    let slot = match checker.locals.get_mut(name) {
                        Some(local) => {
                            if is_affine(&local.ty) && !local.affine_consumed {
                                return Err(Diagnostic::error(
                                    "C005",
                                    format!(
                                        "{} must be consumed before it can be overwritten",
                                        affine_label(&local.ty)
                                    ),
                                    statement.span,
                                ));
                            }
                            warn_overwrite(local, checker.warnings);
                            local.initialized = true;
                            local.affine_consumed = false;
                            local.read = false;
                            local.last_write = Some(statement.span);
                            local.known = known_value(&value.kind);
                            local.slot
                        }
                        None => {
                            let slot =
                                checker.declare(name, value.ty.clone(), true, target[0].span)?;
                            let local = checker.locals.get_mut(name).expect("declared local");
                            local.last_write = Some(statement.span);
                            local.known = known_value(&value.kind);
                            slot
                        }
                    };
                    body.push(hir::Statement::Assign {
                        slot,
                        fields: Vec::new(),
                        value,
                    });
                } else {
                    let local = checker.read_local(name, target[0].span)?;
                    let slot = local.slot;
                    let mut ty = local.ty.clone();
                    let mut fields = Vec::new();
                    for name in &target[1..] {
                        if matches!(ty, Type::Outputs(_)) {
                            return Err(Diagnostic::error(
                                "C005",
                                "named call outputs are read-only projections",
                                name.span,
                            ));
                        }
                        let (index, field_type) = checker.field_type(&ty, name)?;
                        fields.push(index);
                        ty = field_type;
                    }
                    let value = checker.expression(value, Some(&ty))?;
                    let local = checker.locals.get_mut(name).expect("checked local");
                    local.last_write = Some(statement.span);
                    local.read = false;
                    local.known = None;
                    body.push(hir::Statement::Assign {
                        slot,
                        fields,
                        value,
                    });
                }
            }
            StatementKind::Call(expression) => {
                let value = checker.expression(expression, None)?;
                if value.ty != Type::Unit {
                    return Err(Diagnostic::error(
                        "C005",
                        "only zero-output calls may be action statements",
                        expression.span,
                    ));
                }
                if matches!(value.kind, ExpressionKind::Fail(_)) {
                    if index + 1 != source.body.len() {
                        return Err(Diagnostic::error(
                            "C005",
                            "fail must be the final statement in a function clause",
                            expression.span,
                        ));
                    }
                    terminates = true;
                }
                body.push(hir::Statement::Call(value));
            }
        }
    }
    if !terminates {
        for field in &function.outputs {
            checker.read_local(&field.name, source.span)?;
        }
    }
    if !terminates
        && let Some((_, local)) = checker
            .locals
            .iter()
            .find(|(_, local)| is_affine(&local.ty) && !local.affine_consumed)
    {
        return Err(Diagnostic::error(
            "C005",
            format!(
                "{} must be consumed before the function exits",
                affine_label(&local.ty)
            ),
            local.last_write.unwrap_or(source.span),
        ));
    }
    for local in checker.locals.values() {
        if !local.read
            && let Some(span) = local.last_write
        {
            checker.warnings.push(Diagnostic::error(
                "W001",
                "value is written but never read",
                span,
            ));
        }
    }
    Ok(hir::Clause {
        patterns,
        bindings,
        locals: checker.slots,
        outputs,
        body,
        terminates,
    })
}

fn warn_overwrite(local: &Local, warnings: &mut Vec<Diagnostic>) {
    if !local.read
        && let Some(span) = local.last_write
    {
        warnings.push(Diagnostic::error(
            "W001",
            "value is overwritten before it is read",
            span,
        ));
    }
}

impl Body<'_, '_> {
    fn declare(&mut self, name: &str, ty: Type, initialized: bool, span: Span) -> Checked<usize> {
        if self.locals.contains_key(name) || self.handlers.contains_key(name) {
            return Err(Diagnostic::error(
                "C002",
                format!("duplicate local binding '{name}'"),
                span,
            ));
        }
        let slot = self.slots.len();
        self.slots.push(ty.clone());
        self.locals.insert(
            name.into(),
            Local {
                slot,
                ty,
                initialized,
                last_write: None,
                read: false,
                known: None,
                affine_consumed: false,
            },
        );
        Ok(slot)
    }

    pub(super) fn read_local(&mut self, name: &str, span: Span) -> Checked<&Local> {
        let local = self
            .locals
            .get_mut(name)
            .filter(|local| local.initialized)
            .ok_or_else(|| {
                Diagnostic::error(
                    "C004",
                    format!("unknown or uninitialized local '{name}'"),
                    span,
                )
            })?;
        if is_affine(&local.ty) {
            return Err(Diagnostic::error(
                "C005",
                format!(
                    "{} is affine and may only be used by its task operation",
                    affine_label(&local.ty)
                ),
                span,
            ));
        }
        local.read = true;
        Ok(local)
    }

    pub(super) fn affine_local(
        &mut self,
        name: &str,
        span: Span,
        consume: bool,
    ) -> Checked<(usize, Type)> {
        let local = self
            .locals
            .get_mut(name)
            .ok_or_else(|| Diagnostic::error("C004", format!("unknown local '{name}'"), span))?;
        if !is_affine(&local.ty) {
            return Err(Diagnostic::error(
                "C005",
                "this operation requires a Task or Group local",
                span,
            ));
        }
        if local.affine_consumed {
            return Err(Diagnostic::error(
                "C005",
                format!("{} has already been consumed", affine_label(&local.ty)),
                span,
            ));
        }
        local.read = true;
        if consume {
            local.affine_consumed = true;
        }
        Ok((local.slot, local.ty.clone()))
    }
}

fn is_affine(ty: &Type) -> bool {
    matches!(ty, Type::Task(_) | Type::Group | Type::Upload)
}

fn affine_label(ty: &Type) -> &'static str {
    match ty {
        Type::Task(_) => "Task",
        Type::Group => "Group",
        Type::Upload => "Upload",
        _ => unreachable!("affine label requires Task or Group"),
    }
}

pub(crate) fn check_suspensions(program: &crate::hir::Program, errors: &mut Vec<Diagnostic>) {
    for function in &program.functions {
        for clause in &function.clauses {
            if let Err(error) = check_suspension_safety(clause, program) {
                errors.push(error);
            }
        }
    }
}

fn check_suspension_safety(clause: &hir::Clause, program: &crate::hir::Program) -> Checked<()> {
    let body = &clause.body;
    let locals = &clause.locals;
    let outputs = &clause.outputs;
    let mut live: BTreeSet<_> = outputs.iter().copied().collect();
    for statement in body.iter().rev() {
        match statement {
            hir::Statement::Assign {
                slot,
                fields,
                value,
            } => {
                if fields.is_empty() {
                    live.remove(slot);
                } else {
                    live.insert(*slot);
                }
                expression_liveness(value, locals, program, &mut live)?;
            }
            hir::Statement::Call(value) => {
                expression_liveness(value, locals, program, &mut live)?;
            }
        }
    }
    Ok(())
}

fn expression_liveness(
    expression: &hir::Expression,
    locals: &[Type],
    program: &crate::hir::Program,
    live: &mut BTreeSet<usize>,
) -> Checked<()> {
    if suspension_point(&expression.kind, program)
        && let Some(slot) = live.iter().copied().find(|slot| {
            let ty = &locals[*slot];
            !matches!(ty, Type::Task(_) | Type::Group | Type::Upload)
                && !ty.transferable(&program.types)
        })
    {
        return Err(Diagnostic::error(
            "C005",
            format!(
                "{} local cannot remain live across a suspension point",
                locals[slot].label(&program.types)
            ),
            expression.span,
        ));
    }

    match &expression.kind {
        ExpressionKind::Local(slot) => {
            live.insert(*slot);
        }
        ExpressionKind::Field { value, .. }
        | ExpressionKind::Fail(value)
        | ExpressionKind::Unary { value, .. }
        | ExpressionKind::Promote(value)
        | ExpressionKind::Some(value)
        | ExpressionKind::AwaitTask(value)
        | ExpressionKind::StopTask(value)
        | ExpressionKind::Group(value)
        | ExpressionKind::AwaitGroup(value)
        | ExpressionKind::StopGroup(value)
        | ExpressionKind::ChannelReceive(value)
        | ExpressionKind::ChannelClose(value) => {
            expression_liveness(value, locals, program, live)?;
        }
        ExpressionKind::Call { arguments, .. }
        | ExpressionKind::CaptureResult { arguments, .. }
        | ExpressionKind::ParallelCall { arguments, .. }
        | ExpressionKind::BlockingCall { arguments, .. } => {
            call_arguments_liveness(arguments, locals, program, live)?;
        }
        ExpressionKind::ModelOperation { operation, .. } => {
            for value in operation.expressions().into_iter().rev() {
                expression_liveness(value, locals, program, live)?;
            }
        }
        ExpressionKind::RunCall {
            group, arguments, ..
        } => {
            call_arguments_liveness(arguments, locals, program, live)?;
            if let Some(group) = group {
                expression_liveness(group, locals, program, live)?;
            }
        }
        ExpressionKind::Channel { capacity, .. } => {
            expression_liveness(capacity, locals, program, live)?;
        }
        ExpressionKind::ChannelSend { channel, value } => {
            expression_liveness(value, locals, program, live)?;
            expression_liveness(channel, locals, program, live)?;
        }
        ExpressionKind::Intrinsic { arguments, .. }
        | ExpressionKind::JobEnqueue { arguments, .. }
        | ExpressionKind::Collection { arguments, .. }
        | ExpressionKind::Variant { arguments, .. }
        | ExpressionKind::List(arguments) => {
            for argument in arguments.iter().rev() {
                expression_liveness(argument, locals, program, live)?;
            }
        }
        ExpressionKind::Record { fields } => {
            for (_, value) in fields.iter().rev() {
                expression_liveness(value, locals, program, live)?;
            }
        }
        ExpressionKind::Map(entries) => {
            for (key, value) in entries.iter().rev() {
                expression_liveness(value, locals, program, live)?;
                expression_liveness(key, locals, program, live)?;
            }
        }
        ExpressionKind::Binary { left, right, .. } => {
            expression_liveness(right, locals, program, live)?;
            expression_liveness(left, locals, program, live)?;
        }
        ExpressionKind::Constant(_) | ExpressionKind::ChoiceOptions { .. } => {}
    }
    Ok(())
}

fn call_arguments_liveness(
    arguments: &[hir::CallArgument],
    locals: &[Type],
    program: &crate::hir::Program,
    live: &mut BTreeSet<usize>,
) -> Checked<()> {
    for argument in arguments.iter().rev() {
        if let hir::CallArgument::Value(value) = argument {
            expression_liveness(value, locals, program, live)?;
        }
    }
    Ok(())
}

fn suspension_point(kind: &ExpressionKind, program: &crate::hir::Program) -> bool {
    if let ExpressionKind::Intrinsic { operation, .. } = kind {
        return operation.is_async();
    }
    if let ExpressionKind::Call { target, .. } | ExpressionKind::CaptureResult { target, .. } = kind
    {
        return call_target_may_suspend(*target, program);
    }
    if matches!(
        kind,
        ExpressionKind::ModelOperation { .. } | ExpressionKind::JobEnqueue { .. }
    ) {
        return true;
    }
    if let ExpressionKind::Collection {
        operation,
        handler,
        sequence,
        ..
    } = kind
    {
        return *operation == hir::CollectionOp::ParallelEach
            || (matches!(
                sequence,
                Some(hir::SequenceKind::AsyncStream | hir::SequenceKind::RowStream)
            ) && *operation != hir::CollectionOp::Close)
            || handler.is_some_and(|target| handler_may_suspend(target, program));
    }
    matches!(
        kind,
        ExpressionKind::RunCall { .. }
            | ExpressionKind::AwaitTask(_)
            | ExpressionKind::StopTask(_)
            | ExpressionKind::AwaitGroup(_)
            | ExpressionKind::StopGroup(_)
            | ExpressionKind::ParallelCall { .. }
            | ExpressionKind::BlockingCall { .. }
            | ExpressionKind::ChannelSend { .. }
            | ExpressionKind::ChannelReceive(_)
            | ExpressionKind::ChannelClose(_)
    )
}

fn call_target_may_suspend(target: hir::CallTarget, program: &crate::hir::Program) -> bool {
    match target {
        hir::CallTarget::Function(id) => function_may_suspend(id, program),
        hir::CallTarget::Handler(target) => handler_may_suspend(target, program),
    }
}

fn handler_may_suspend(target: hir::HandlerTarget, program: &crate::hir::Program) -> bool {
    match target {
        hir::HandlerTarget::Function(id) => function_may_suspend(id, program),
        hir::HandlerTarget::Parameter(_) => true,
    }
}

fn function_may_suspend(id: usize, program: &crate::hir::Program) -> bool {
    program.functions[id].suspends || !program.suspension_handlers[id].is_empty()
}

fn known_value(value: &ExpressionKind) -> Option<ExpressionKind> {
    match value {
        ExpressionKind::Constant(_) => Some(value.clone()),
        ExpressionKind::Variant { arguments, .. } if arguments.is_empty() => Some(value.clone()),
        ExpressionKind::Some(inner) if known_value(&inner.kind).is_some() => Some(value.clone()),
        _ => None,
    }
}
