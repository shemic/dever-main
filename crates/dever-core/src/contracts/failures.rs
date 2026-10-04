use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::diagnostic::{Diagnostic, Label};
use crate::hir::{
    Atom, CallArgument, CallTarget, Clause, CollectionOp, Domain, Expression,
    ExpressionKind as Expr, Function, Program, Projection, Statement,
};
use crate::source::Span;
use crate::syntax::BinaryOperator;
use crate::types::{Parameter, Shape, Type, output_type};

use super::MAX_PROOF_WORK;

/// Origins stay attached to values; reading or renaming a result is not handling it.
#[derive(Clone, Default)]
struct Value {
    own: BTreeSet<usize>,
    fields: Vec<Rc<Value>>,
    definite_error: bool,
    fields_guaranteed: bool,
    list_length: Option<usize>,
    origins_cache: OnceCell<BTreeSet<usize>>,
    error_cache: OnceCell<bool>,
}

impl Value {
    fn origins(&self) -> &BTreeSet<usize> {
        self.origins_cache.get_or_init(|| {
            let mut origins = self.own.clone();
            for field in &self.fields {
                origins.extend(field.origins());
            }
            origins
        })
    }

    fn carries_error(&self) -> bool {
        *self.error_cache.get_or_init(|| {
            self.definite_error
                || (self.fields_guaranteed && self.fields.iter().any(|field| field.carries_error()))
        })
    }

    fn invalidate(&mut self) {
        self.origins_cache.take();
        self.error_cache.take();
    }
}

struct Flow<'a> {
    program: &'a Program,
    summaries: &'a [Vec<bool>],
    error_types: &'a [bool],
    expanded: usize,
    exhausted: Option<Span>,
    pending: BTreeMap<usize, Span>,
    next_origin: usize,
    controls: BTreeSet<usize>,
    locals: Vec<Value>,
}

pub(super) fn check(
    program: &Program,
    dependencies: &super::dependencies::Dependencies,
    errors: &mut Vec<Diagnostic>,
) {
    let error_types = error_types(program);
    // Monotone finite summaries describe guaranteed failure outputs, not guessed success.
    let mut summaries: Vec<Vec<bool>> = program
        .functions
        .iter()
        .map(|f| vec![false; f.outputs.len()])
        .collect();
    let mut pending = vec![Vec::new(); program.functions.len()];
    let mut work = dependencies.work();
    while let Some(id) = work.next() {
        let function = &program.functions[id];
        let mut next = vec![true; function.outputs.len()];
        pending[id].clear();
        for clause in &function.clauses {
            let analysis = analyze(program, &summaries, &error_types, function, clause);
            if let Some(span) = analysis.exhausted {
                errors.push(Diagnostic::error("C015", "failure analysis exceeds its structural limit; simplify the nested failure-bearing value", span));
                return;
            }
            pending[id].push(analysis.pending.values().next().copied());
            for (guarantee, value) in next.iter_mut().zip(&analysis.outputs) {
                *guarantee &= value.carries_error();
            }
        }
        if next != summaries[id] {
            summaries[id] = next;
            work.changed(id);
        }
    }
    for (function, clauses) in program.functions.iter().zip(pending) {
        if function.recovery.is_some() {
            continue;
        }
        for span in clauses.into_iter().flatten() {
            let mut diagnostic = Diagnostic::error(
                "C012",
                "failure is not handled: propagate it, return an explicit error, or declare recover(\"reason\")",
                span,
            );
            diagnostic.related.push(Label { span: function.span, message: format!("function '{}' leaves a failure obligation unresolved; logging or reading it is not recovery", function.name) });
            errors.push(diagnostic);
        }
    }
}

fn error_types(program: &Program) -> Vec<bool> {
    let mut known = vec![false; program.types.len()];
    loop {
        let mut changed = false;
        for (id, definition) in program.types.iter().enumerate() {
            if known[id] {
                continue;
            }
            let contains = match &definition.shape {
                Shape::Record(fields) => fields.iter().any(|field| has_error(&field.ty, &known)),
                Shape::Choice(variants) => variants.iter().any(|variant| {
                    variant.error
                        || variant
                            .fields
                            .iter()
                            .any(|field| has_error(&field.ty, &known))
                }),
            };
            if contains {
                known[id] = true;
                changed = true;
            }
        }
        if !changed {
            return known;
        }
    }
}

fn has_error(ty: &Type, known: &[bool]) -> bool {
    match ty {
        Type::Named(id) => known[*id],
        Type::Nullable(inner) | Type::List(inner) => has_error(inner, known),
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            has_error(key, known) || has_error(value, known)
        }
        Type::Outputs(fields) | Type::Task(fields) => {
            fields.iter().any(|field| has_error(&field.ty, known))
        }
        _ => false,
    }
}

struct Analysis {
    outputs: Vec<Value>,
    pending: BTreeMap<usize, Span>,
    exhausted: Option<Span>,
}

fn analyze(
    program: &Program,
    summaries: &[Vec<bool>],
    error_types: &[bool],
    function: &Function,
    clause: &Clause,
) -> Analysis {
    let mut flow = Flow {
        program,
        summaries,
        error_types,
        expanded: 0,
        exhausted: None,
        pending: BTreeMap::new(),
        next_origin: 0,
        controls: BTreeSet::new(),
        locals: vec![Value::default(); clause.locals.len()],
    };
    let mut inputs = BTreeMap::new();
    let mut domains = clause.patterns.iter();
    for (index, parameter) in function.parameters.iter().enumerate() {
        if let Parameter::Value(ty) = parameter {
            let domain = domains.next().expect("value input domain");
            let value = flow.unknown(ty, Some(domain), function.span);
            flow.controls.extend(value.origins());
            inputs.insert(index, value);
        }
    }
    for binding in &clause.bindings {
        let value = &inputs[&binding.input];
        flow.locals[binding.slot] = match binding.projection {
            Projection::Whole | Projection::NonNull => value.clone(),
            Projection::Payload { field, .. } => value
                .fields
                .get(field)
                .map(|value| value.as_ref().clone())
                .unwrap_or_default(),
        };
    }
    for statement in &clause.body {
        match statement {
            Statement::Assign {
                slot,
                fields,
                value,
            } => {
                let assigned = flow.expression(value);
                let mut target = &mut flow.locals[*slot];
                for index in fields {
                    target.invalidate();
                    // Type checking guarantees the path; unknown containers get their declared shape.
                    if target.fields.len() <= *index {
                        target.fields.resize_with(index + 1, Default::default);
                    }
                    target = Rc::make_mut(&mut target.fields[*index]);
                }
                *target = assigned;
            }
            Statement::Call(value) => {
                flow.expression(value);
            }
        }
    }
    let outputs: Vec<_> = clause
        .outputs
        .iter()
        .map(|slot| flow.locals[*slot].clone())
        .collect();
    for output in &outputs {
        flow.consume(output);
    }
    if outputs.iter().any(Value::carries_error) {
        for control in &flow.controls {
            flow.pending.remove(control);
        }
    }
    Analysis {
        outputs,
        pending: flow.pending,
        exhausted: flow.exhausted,
    }
}

impl Flow<'_> {
    fn origin(&mut self, span: Span) -> usize {
        let origin = self.next_origin;
        self.next_origin += 1;
        self.pending.insert(origin, span);
        origin
    }

    fn consume(&mut self, value: &Value) {
        for origin in value.origins() {
            self.pending.remove(origin);
        }
    }

    fn unknown(&mut self, ty: &Type, domain: Option<&Domain>, span: Span) -> Value {
        if !has_error(ty, self.error_types) {
            return Value::default();
        }
        if self.expanded >= MAX_PROOF_WORK {
            self.exhausted = Some(span);
            return Value::default();
        }
        self.expanded += 1;
        match ty {
            Type::Named(id) => match &self.program.types[*id].shape {
                Shape::Record(fields) => Value {
                    fields: fields
                        .iter()
                        .map(|field| Rc::new(self.unknown(&field.ty, None, span)))
                        .collect(),
                    fields_guaranteed: true,
                    ..Value::default()
                },
                Shape::Choice(variants) => {
                    let selected: Vec<_> = variants
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| {
                            domain.is_none_or(|atoms| {
                                atoms.iter().any(|atom| {
                                    matches!(atom, Atom::Any)
                                        || matches!(atom, Atom::Variant(id) if id == index)
                                })
                            })
                        })
                        .collect();
                    let possible_error = selected.iter().any(|(_, variant)| variant.error);
                    let definite_error =
                        !selected.is_empty() && selected.iter().all(|(_, variant)| variant.error);
                    let fields = if selected.len() == 1 {
                        selected[0]
                            .1
                            .fields
                            .iter()
                            .map(|field| Rc::new(self.unknown(&field.ty, None, span)))
                            .collect()
                    } else {
                        // Distinct alternatives have no common payload projection. Keep their obligations.
                        selected
                            .iter()
                            .flat_map(|(_, variant)| &variant.fields)
                            .map(|field| Rc::new(self.unknown(&field.ty, None, span)))
                            .collect()
                    };
                    Value {
                        own: if possible_error {
                            BTreeSet::from([self.origin(span)])
                        } else {
                            BTreeSet::new()
                        },
                        fields,
                        definite_error,
                        fields_guaranteed: selected.len() == 1,
                        list_length: None,
                        ..Value::default()
                    }
                }
            },
            Type::Nullable(inner) => {
                if domain.is_some_and(|atoms| atoms.iter().all(|atom| matches!(atom, Atom::Null))) {
                    Value::default()
                } else {
                    let mut value = self.unknown(inner, domain, span);
                    if domain
                        .is_none_or(|atoms| atoms.iter().any(|atom| matches!(atom, Atom::Null)))
                    {
                        value.definite_error = false;
                        value.fields_guaranteed = false;
                    }
                    value
                }
            }
            Type::List(inner) => Value {
                fields: vec![Rc::new(self.unknown(inner, None, span))],
                ..Value::default()
            },
            Type::Map(key, value) | Type::MapEntry(key, value) => Value {
                fields: vec![
                    Rc::new(self.unknown(key, None, span)),
                    Rc::new(self.unknown(value, None, span)),
                ],
                fields_guaranteed: matches!(ty, Type::MapEntry(..)),
                ..Value::default()
            },
            Type::Outputs(fields) | Type::Task(fields) => Value {
                fields: fields
                    .iter()
                    .map(|field| Rc::new(self.unknown(&field.ty, None, span)))
                    .collect(),
                fields_guaranteed: true,
                ..Value::default()
            },
            // An unread Stream has no yielded failure. Its handler checks each yielded value.
            _ => Value::default(),
        }
    }

    fn expression(&mut self, expression: &Expression) -> Value {
        match &expression.kind {
            Expr::Local(slot) => self.locals[*slot].clone(),
            Expr::Field { value, index } => self
                .expression(value)
                .fields
                .get(*index)
                .map(|value| value.as_ref().clone())
                .unwrap_or_default(),
            Expr::Record { fields } => {
                let empty = Rc::new(Value::default());
                let mut values = vec![empty; fields.len()];
                for (index, value) in fields {
                    values[*index] = Rc::new(self.expression(value));
                }
                Value {
                    fields: values,
                    fields_guaranteed: true,
                    ..Value::default()
                }
            }
            Expr::Variant { variant, arguments } => {
                let fields = arguments
                    .iter()
                    .map(|value| Rc::new(self.expression(value)))
                    .collect();
                let error = matches!(&expression.ty, Type::Named(id) if matches!(&self.program.types[*id].shape, Shape::Choice(variants) if variants[*variant].error));
                Value {
                    fields,
                    definite_error: error,
                    fields_guaranteed: true,
                    own: if error {
                        BTreeSet::from([self.origin(expression.span)])
                    } else {
                        BTreeSet::new()
                    },
                    list_length: None,
                    ..Value::default()
                }
            }
            Expr::Fail(error) => {
                self.expression(error);
                // fail terminates the clause with an explicit propagated or converted
                // failure, so no previously observed obligation can be silently lost.
                self.pending.clear();
                Value::default()
            }
            Expr::CaptureResult { arguments, .. } => {
                for argument in arguments {
                    if let CallArgument::Value(value) = argument {
                        let value = self.expression(value);
                        self.consume(&value);
                    }
                }
                self.unknown(&expression.ty, None, expression.span)
            }
            Expr::ModelOperation { operation, .. } => {
                for value in operation.expressions() {
                    let value = self.expression(value);
                    self.consume(&value);
                }
                self.unknown(&expression.ty, None, expression.span)
            }
            Expr::JobEnqueue { arguments, .. } => {
                for value in arguments {
                    let value = self.expression(value);
                    self.consume(&value);
                }
                self.unknown(&expression.ty, None, expression.span)
            }
            Expr::List(elements) => Value {
                fields: elements
                    .iter()
                    .map(|value| Rc::new(self.expression(value)))
                    .collect(),
                fields_guaranteed: true,
                list_length: Some(elements.len()),
                ..Value::default()
            },
            Expr::Map(entries) => {
                let mut fields = Vec::new();
                for (index, (key, value)) in entries.iter().enumerate() {
                    let key_value = self.expression(key);
                    let value = self.expression(value);
                    // Later equal keys replace earlier entries. Unknown keys cannot
                    // prove that a failure survives construction of the Map.
                    if entries[index + 1..].iter().all(|(later, _)| matches!((&key.kind, &later.kind), (Expr::Constant(left), Expr::Constant(right)) if left != right)) {
                        fields.extend([Rc::new(key_value), Rc::new(value)]);
                    }
                }
                Value {
                    fields,
                    fields_guaranteed: true,
                    ..Value::default()
                }
            }
            Expr::Some(value) | Expr::Promote(value) => self.expression(value),
            Expr::Unary { value, .. } => {
                self.expression(value);
                Value::default()
            }
            Expr::Binary {
                left,
                operator,
                right,
            } => {
                self.expression(left);
                let short_circuit = match operator {
                    BinaryOperator::And => Some(false),
                    BinaryOperator::Or => Some(true),
                    _ => None,
                };
                if let Some(skip_when) = short_circuit {
                    if let Expr::Constant(crate::hir::Constant::Bool(value)) = left.kind {
                        if value == skip_when {
                            return Value::default();
                        }
                    } else {
                        let pending = self.pending.clone();
                        self.expression(right);
                        // The right operand may never execute, so it cannot discharge
                        // obligations that already existed before the conditional call.
                        self.pending.extend(pending);
                        return Value::default();
                    }
                }
                self.expression(right);
                Value::default()
            }
            Expr::Call { target, arguments }
            | Expr::ParallelCall { target, arguments }
            | Expr::BlockingCall { target, arguments } => {
                self.call_value(*target, arguments, &expression.ty, expression.span)
            }
            Expr::RunCall {
                group,
                target,
                arguments,
            } => {
                if let Some(group) = group {
                    self.expression(group);
                }
                let Type::Task(outputs) = &expression.ty else {
                    self.call_value(*target, arguments, &Type::Unit, expression.span);
                    return Value::default();
                };
                let result =
                    self.call_value(*target, arguments, &output_type(outputs), expression.span);
                match outputs.as_slice() {
                    [] => Value::default(),
                    [_] => Value {
                        fields: vec![Rc::new(result)],
                        fields_guaranteed: true,
                        ..Value::default()
                    },
                    _ => result,
                }
            }
            Expr::AwaitTask(task) => {
                let result = self.expression(task);
                match &task.ty {
                    Type::Task(outputs) if outputs.len() == 1 => result
                        .fields
                        .first()
                        .map(|value| value.as_ref().clone())
                        .unwrap_or_default(),
                    Type::Task(_) => result,
                    _ => unreachable!("await task HIR carries a Task"),
                }
            }
            Expr::StopTask(task)
            | Expr::Group(task)
            | Expr::AwaitGroup(task)
            | Expr::StopGroup(task)
            | Expr::ChannelReceive(task)
            | Expr::ChannelClose(task) => {
                self.expression(task);
                self.unknown(&expression.ty, None, expression.span)
            }
            Expr::Channel { capacity, .. } => {
                self.expression(capacity);
                Value::default()
            }
            Expr::ChannelSend { channel, value } => {
                self.expression(channel);
                let value = self.expression(value);
                self.consume(&value);
                Value::default()
            }
            Expr::Intrinsic {
                operation: crate::intrinsic::Intrinsic::StreamOf,
                arguments,
                ..
            } => self.expression(&arguments[0]),
            Expr::Intrinsic { arguments, .. } => {
                // Typed primitive boundaries consume task/context/upload inputs;
                // recoverable failures remain explicit in their checked outputs.
                for value in arguments {
                    let value = self.expression(value);
                    self.consume(&value);
                }
                self.unknown(&expression.ty, None, expression.span)
            }
            Expr::Collection {
                operation,
                handler,
                arguments,
                ..
            } => self.collection(expression, operation, handler.is_some(), arguments),
            Expr::Constant(_) | Expr::ChoiceOptions { .. } => Value::default(),
        }
    }

    fn call_value(
        &mut self,
        target: CallTarget,
        arguments: &[CallArgument],
        output: &Type,
        span: Span,
    ) -> Value {
        for argument in arguments {
            if let CallArgument::Value(value) = argument {
                let value = self.expression(value);
                self.consume(&value);
            }
        }
        let mut result = self.unknown(output, None, span);
        let callee = match target {
            CallTarget::Function(id)
            | CallTarget::Handler(crate::hir::HandlerTarget::Function(id)) => Some(id),
            CallTarget::Handler(crate::hir::HandlerTarget::Parameter(_)) => None,
        };
        if let Some(id) = callee {
            match self.summaries[id].as_slice() {
                [guarantee] => result.definite_error = *guarantee,
                guarantees => {
                    for (field, guarantee) in result.fields.iter_mut().zip(guarantees) {
                        let field = Rc::make_mut(field);
                        field.invalidate();
                        field.definite_error = *guarantee;
                    }
                }
            }
        }
        result
    }

    fn collection(
        &mut self,
        expression: &Expression,
        operation: &CollectionOp,
        has_handler: bool,
        arguments: &[Expression],
    ) -> Value {
        let values: Vec<_> = arguments
            .iter()
            .map(|value| self.expression(value))
            .collect();
        match operation {
            CollectionOp::Append if values[0].list_length.is_some() => {
                let mut result = values[0].clone();
                result.invalidate();
                result.fields.push(Rc::new(values[1].clone()));
                result.list_length = result.list_length.map(|length| length + 1);
                result
            }
            CollectionOp::Append | CollectionOp::Entries => {
                // Copying/repacking errors keeps their identities and obligations.
                Value {
                    fields: values.into_iter().map(Rc::new).collect(),
                    fields_guaranteed: true,
                    ..Value::default()
                }
            }
            CollectionOp::Each
            | CollectionOp::ParallelEach
            | CollectionOp::Reduce
            | CollectionOp::Filter
            | CollectionOp::Sum
                if has_handler =>
            {
                if values[0].list_length == Some(0) {
                    return if *operation == CollectionOp::Reduce {
                        values[1].clone()
                    } else {
                        Value {
                            list_length: Some(0),
                            ..Value::default()
                        }
                    };
                }
                self.consume(&values[0]);
                // Context is not passed anywhere when a traversal is empty.
                if *operation == CollectionOp::Reduce
                    || values[0].list_length.is_some_and(|length| length > 0)
                {
                    for value in &values[1..] {
                        self.consume(value);
                    }
                }
                self.unknown(&expression.ty, None, expression.span)
            }
            CollectionOp::ReduceUntil => {
                // Only the state is wholly transferred; a finite source may stop early.
                if let Some(state) = values.get(1) {
                    self.consume(state);
                }
                self.unknown(&expression.ty, None, expression.span)
            }
            CollectionOp::First if values[0].list_length == Some(1) => {
                values[0].fields[0].as_ref().clone()
            }
            CollectionOp::First if values[0].list_length == Some(0) => Value::default(),
            CollectionOp::Put => {
                // The replaced entry may contain an unhandled failure. Only the
                // inserted key/value are guaranteed to survive this operation.
                Value {
                    fields: values[1..].iter().cloned().map(Rc::new).collect(),
                    fields_guaranteed: true,
                    ..Value::default()
                }
            }
            CollectionOp::First | CollectionOp::Get | CollectionOp::Find | CollectionOp::Remove => {
                // A projection cannot silently dispose of errors in the unselected remainder.
                self.unknown(&expression.ty, None, expression.span)
            }
            _ => Value::default(),
        }
    }
}
