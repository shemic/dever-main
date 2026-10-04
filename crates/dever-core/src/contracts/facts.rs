//! Conservative value guarantees shared by construction, assignment and call checks.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use dever_runtime::number::DecimalValue;

use crate::diagnostic::{Diagnostic, Label};
use crate::hir::{
    Atom, CallArgument, CallTarget, Clause, CollectionOp, Domain, Expression, ExpressionKind,
    Function, HandlerTarget, Program, Projection, Statement,
};
use crate::source::Span;
use crate::syntax::BinaryOperator as Binary;
use crate::types::{Field, Parameter, Shape, Type};

mod domain;

use super::MAX_PROOF_WORK;
use super::dependencies::Dependencies;
pub(crate) use domain::bounds_subset;
use domain::{binary, constant_atom, disjoint, subset, unary, union};

type FactFields = BTreeMap<usize, Rc<Fact>>;

#[derive(Default)]
struct ProofWork {
    visits: Cell<usize>,
}

impl ProofWork {
    fn spend(&self) -> bool {
        let visits = self.visits.get();
        if visits > MAX_PROOF_WORK {
            return false;
        }
        self.visits.set(visits + 1);
        visits < MAX_PROOF_WORK
    }

    fn exhausted(&self) -> bool {
        self.visits.get() > MAX_PROOF_WORK
    }
}

#[derive(Clone, Debug, PartialEq)]
struct InputOrigin {
    parameter: usize,
    fields: Vec<usize>,
}

#[derive(Clone, Debug)]
struct Fact {
    domain: Domain,
    fields: Rc<FactFields>,
    origin: Option<InputOrigin>,
    evidence: Option<Span>,
}

impl Fact {
    fn domain(domain: Domain) -> Self {
        Self {
            domain,
            fields: Rc::new(BTreeMap::new()),
            origin: None,
            evidence: None,
        }
    }

    fn merge(&self, other: &Self, work: &ProofWork) -> Self {
        self.merge_cached(other, work, &mut BTreeMap::new())
    }

    fn merge_cached(
        &self,
        other: &Self,
        work: &ProofWork,
        cache: &mut BTreeMap<(usize, usize), Rc<FactFields>>,
    ) -> Self {
        if !work.spend() {
            return Self::domain(vec![Atom::Any]);
        }
        let key = (
            Rc::as_ptr(&self.fields) as usize,
            Rc::as_ptr(&other.fields) as usize,
        );
        let fields = if Rc::ptr_eq(&self.fields, &other.fields) {
            Rc::clone(&self.fields)
        } else if let Some(fields) = cache.get(&key) {
            Rc::clone(fields)
        } else {
            let fields = Rc::new(
                self.fields
                    .iter()
                    .filter_map(|(index, fact)| {
                        other
                            .fields
                            .get(index)
                            .map(|other| (*index, Rc::new(fact.merge_cached(other, work, cache))))
                    })
                    .collect(),
            );
            cache.insert(key, Rc::clone(&fields));
            fields
        };
        Self {
            domain: union(&self.domain, &other.domain),
            fields,
            origin: (self.origin == other.origin)
                .then(|| self.origin.clone())
                .flatten(),
            evidence: (self.evidence == other.evidence)
                .then_some(self.evidence)
                .flatten(),
        }
    }
}

pub(super) fn check(
    program: &mut Program,
    dependencies: &Dependencies,
    errors: &mut Vec<Diagnostic>,
) {
    let mut analyzer = Analyzer {
        program,
        summaries: vec![None; program.functions.len()],
        errors: Vec::new(),
        warnings: Vec::new(),
        work: ProofWork::default(),
    };
    for id in dependencies.order() {
        analyzer.analyze_function(*id);
    }
    errors.extend(analyzer.errors);
    program.warnings.extend(analyzer.warnings);
}

struct Analyzer<'a> {
    program: &'a Program,
    summaries: Vec<Option<Vec<Fact>>>,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
    work: ProofWork,
}

impl Analyzer<'_> {
    fn unknown(&self, ty: &Type) -> Fact {
        let domain = match ty {
            Type::Bool => vec![Atom::Bool(false), Atom::Bool(true)],
            Type::Int => vec![Atom::Int(i64::MIN, i64::MAX)],
            Type::Decimal => vec![Atom::Decimal(
                DecimalValue::minimum(),
                DecimalValue::maximum(),
            )],
            Type::Nullable(inner) => {
                let mut domain = self.unknown(inner).domain;
                domain.push(Atom::Null);
                domain
            }
            Type::Named(id) => match &self.program.types[*id].shape {
                Shape::Choice(variants) => (0..variants.len()).map(Atom::Variant).collect(),
                Shape::Record(_) => vec![Atom::Any],
            },
            _ => vec![Atom::Any],
        };
        Fact::domain(domain)
    }

    fn field_fact(&self, field: &Field) -> Fact {
        if field.bounds.is_empty() {
            self.unknown(&field.ty)
        } else {
            Fact::domain(field.bounds.clone())
        }
    }

    fn fields<'a>(&'a self, ty: &'a Type) -> Option<&'a [Field]> {
        match ty.base() {
            Type::Named(id) => match &self.program.types[*id].shape {
                Shape::Record(fields) => Some(fields),
                Shape::Choice(_) => None,
            },
            Type::Outputs(fields) => Some(fields),
            _ => None,
        }
    }

    fn project(&self, fact: &Fact, ty: &Type, index: usize) -> Fact {
        if let Some(field) = fact.fields.get(&index) {
            return field.as_ref().clone();
        }
        let mut field = self
            .fields(ty)
            .map(|fields| self.field_fact(&fields[index]))
            .unwrap_or_else(|| Fact::domain(vec![Atom::Any]));
        field.origin = fact.origin.clone().map(|mut origin| {
            origin.fields.push(index);
            origin
        });
        field.evidence = fact.evidence;
        field
    }

    fn function(&self, id: usize) -> Vec<Fact> {
        self.summaries[id].clone().unwrap_or_else(|| {
            self.program.functions[id]
                .outputs
                .iter()
                .map(|field| self.field_fact(field))
                .collect()
        })
    }

    fn analyze_function(&mut self, id: usize) {
        self.work.visits.set(0);
        let function = self.program.functions[id].clone();
        let mut summary: Option<Vec<Fact>> = None;
        for clause in &function.clauses {
            let facts = self.clause(&function, clause);
            summary = Some(match summary {
                None => facts,
                Some(previous) => previous
                    .iter()
                    .zip(&facts)
                    .map(|(left, right)| left.merge(right, &self.work))
                    .collect(),
            });
        }
        let summary = summary.unwrap_or_else(|| {
            function
                .outputs
                .iter()
                .map(|field| self.field_fact(field))
                .collect()
        });
        self.summaries[id] = Some(summary);
        if self.work.exhausted() {
            self.errors.push(Diagnostic::error("C015", "value contract analysis exceeds its proof-work limit; simplify the value or contract flow", function.span));
        }
    }

    fn clause(&mut self, function: &Function, clause: &Clause) -> Vec<Fact> {
        let mut locals: Vec<_> = clause.locals.iter().map(|ty| self.unknown(ty)).collect();
        let mut axis = 0;
        let inputs: Vec<_> = function
            .parameters
            .iter()
            .enumerate()
            .map(|(parameter, ty)| {
                if let Parameter::Value(_) = ty {
                    let mut fact = Fact::domain(clause.patterns[axis].clone());
                    axis += 1;
                    fact.origin = Some(InputOrigin {
                        parameter,
                        fields: Vec::new(),
                    });
                    fact.evidence = Some(function.span);
                    Some(fact)
                } else {
                    None
                }
            })
            .collect();
        for binding in &clause.bindings {
            let input = inputs[binding.input].as_ref().expect("value binding");
            locals[binding.slot] = match binding.projection {
                Projection::Whole | Projection::NonNull => input.clone(),
                Projection::Payload { variant, field } => {
                    let ty = function.parameters[binding.input]
                        .value_type()
                        .expect("value binding");
                    if let Type::Related(payload) = ty.base() {
                        debug_assert_eq!(variant, 1);
                        debug_assert_eq!(field, 0);
                        let mut fact = self.unknown(payload);
                        fact.evidence = Some(function.span);
                        locals[binding.slot] = fact;
                        continue;
                    }
                    let Type::Named(id) = ty.base() else {
                        unreachable!("choice binding")
                    };
                    let Shape::Choice(variants) = &self.program.types[*id].shape else {
                        unreachable!("choice binding")
                    };
                    let mut fact = self.field_fact(&variants[variant].fields[field]);
                    fact.evidence = Some(self.program.types[*id].span);
                    fact
                }
            };
        }
        for statement in &clause.body {
            match statement {
                Statement::Assign {
                    slot,
                    fields,
                    value,
                } => {
                    let fact = self.expression(value, &locals, function);
                    if fields.is_empty() {
                        if let Some(output) =
                            clause.outputs.iter().position(|output| output == slot)
                        {
                            self.require(
                                &fact,
                                &function.outputs[output].bounds,
                                value.span,
                                "named output",
                                function.span,
                            );
                        }
                        locals[*slot] = fact;
                    } else {
                        self.write(
                            &mut locals[*slot],
                            &clause.locals[*slot],
                            fields,
                            fact,
                            value.span,
                        );
                    }
                }
                Statement::Call(value) => {
                    self.expression(value, &locals, function);
                }
            }
        }
        clause
            .outputs
            .iter()
            .map(|slot| locals[*slot].clone())
            .collect()
    }

    fn write(&mut self, host: &mut Fact, ty: &Type, path: &[usize], value: Fact, span: Span) {
        if !self.work.spend() {
            return;
        }
        let field = self.fields(ty).expect("checked assignment path")[path[0]].clone();
        if path.len() == 1 {
            self.require(&value, &field.bounds, span, "field assignment", span);
            Rc::make_mut(&mut host.fields).insert(path[0], Rc::new(value));
        } else {
            let mut child = self.project(host, ty, path[0]);
            self.write(&mut child, &field.ty, &path[1..], value, span);
            Rc::make_mut(&mut host.fields).insert(path[0], Rc::new(child));
        }
        // Value aliases keep their own snapshot. This aggregate no longer equals
        // the original parameter after any of its fields change.
        host.origin = None;
        host.evidence = Some(span);
    }

    fn require(
        &mut self,
        fact: &Fact,
        bounds: &Domain,
        span: Span,
        context: &str,
        declaration: Span,
    ) {
        if bounds.is_empty() || subset(&fact.domain, bounds) {
            return;
        }
        let mut diagnostic = Diagnostic::error(
            "C011",
            format!("cannot prove {context} satisfies its numeric bounds"),
            span,
        );
        diagnostic.related.push(Label {
            span: declaration,
            message: format!("required bounds: {bounds:?}"),
        });
        if let Some(evidence) = fact.evidence.filter(|evidence| *evidence != span) {
            diagnostic.related.push(Label {
                span: evidence,
                message: format!("known value domain: {:?}", fact.domain),
            });
        }
        self.errors.push(diagnostic);
    }

    fn expression(&mut self, expression: &Expression, locals: &[Fact], owner: &Function) -> Fact {
        if !self.work.spend() {
            return self.unknown(&expression.ty);
        }
        let mut result = match &expression.kind {
            ExpressionKind::Constant(value) => Fact::domain(vec![constant_atom(value)]),
            ExpressionKind::ChoiceOptions { .. } => self.unknown(&expression.ty),
            ExpressionKind::ModelOperation { operation, .. } => {
                for value in operation.expressions() {
                    self.expression(value, locals, owner);
                }
                self.unknown(&expression.ty)
            }
            ExpressionKind::JobEnqueue { arguments, .. } => {
                for value in arguments {
                    self.expression(value, locals, owner);
                }
                self.unknown(&expression.ty)
            }
            ExpressionKind::Local(slot) => return locals[*slot].clone(),
            ExpressionKind::Field { value, index } => {
                let fact = self.expression(value, locals, owner);
                self.project(&fact, &value.ty, *index)
            }
            ExpressionKind::Some(value) => self.expression(value, locals, owner),
            ExpressionKind::Promote(value) => {
                let fact = self.expression(value, locals, owner);
                if expression.ty == Type::Decimal {
                    Fact::domain(
                        fact.domain
                            .iter()
                            .map(|atom| match atom {
                                Atom::Int(lo, hi) => Atom::Decimal(
                                    DecimalValue::from_int(*lo),
                                    DecimalValue::from_int(*hi),
                                ),
                                _ => Atom::Any,
                            })
                            .collect(),
                    )
                } else {
                    self.unknown(&expression.ty)
                }
            }
            ExpressionKind::Record { fields } => {
                let definitions = self
                    .fields(&expression.ty)
                    .expect("checked record")
                    .to_vec();
                let mut result = self.unknown(&expression.ty);
                for (index, value) in fields {
                    let fact = self.expression(value, locals, owner);
                    self.require(
                        &fact,
                        &definitions[*index].bounds,
                        value.span,
                        "record field",
                        expression.span,
                    );
                    Rc::make_mut(&mut result.fields).insert(*index, Rc::new(fact));
                }
                result
            }
            ExpressionKind::Variant { variant, arguments } => {
                let Type::Named(id) = expression.ty.base() else {
                    unreachable!("choice expression")
                };
                let Shape::Choice(variants) = &self.program.types[*id].shape else {
                    unreachable!("choice expression")
                };
                let fields = variants[*variant].fields.clone();
                let mut result = Fact::domain(vec![Atom::Variant(*variant)]);
                for (index, value) in arguments.iter().enumerate() {
                    let fact = self.expression(value, locals, owner);
                    self.require(
                        &fact,
                        &fields[index].bounds,
                        value.span,
                        "choice payload",
                        self.program.types[*id].span,
                    );
                    // Payload projections are resolved only by a selected clause;
                    // they must not be confused with record/output field indices.
                }
                Rc::make_mut(&mut result.fields).clear();
                result
            }
            ExpressionKind::Fail(error) => {
                self.expression(error, locals, owner);
                self.unknown(&expression.ty)
            }
            ExpressionKind::Call { target, arguments }
            | ExpressionKind::ParallelCall { target, arguments }
            | ExpressionKind::BlockingCall { target, arguments } => {
                let values: Vec<_> = arguments
                    .iter()
                    .map(|argument| match argument {
                        CallArgument::Value(value) => Some(self.expression(value, locals, owner)),
                        CallArgument::Handler(_) => None,
                    })
                    .collect();
                self.call(*target, arguments, &values, expression, owner)
            }
            ExpressionKind::CaptureResult {
                target, arguments, ..
            } => {
                let values: Vec<_> = arguments
                    .iter()
                    .map(|argument| match argument {
                        CallArgument::Value(value) => Some(self.expression(value, locals, owner)),
                        CallArgument::Handler(_) => None,
                    })
                    .collect();
                self.call(*target, arguments, &values, expression, owner);
                self.unknown(&expression.ty)
            }
            ExpressionKind::RunCall {
                group,
                target,
                arguments,
            } => {
                if let Some(group) = group {
                    self.expression(group, locals, owner);
                }
                let values: Vec<_> = arguments
                    .iter()
                    .map(|argument| match argument {
                        CallArgument::Value(value) => Some(self.expression(value, locals, owner)),
                        CallArgument::Handler(_) => None,
                    })
                    .collect();
                self.call(*target, arguments, &values, expression, owner)
            }
            ExpressionKind::AwaitTask(value) => self.expression(value, locals, owner),
            ExpressionKind::StopTask(value)
            | ExpressionKind::Group(value)
            | ExpressionKind::AwaitGroup(value)
            | ExpressionKind::StopGroup(value)
            | ExpressionKind::ChannelReceive(value)
            | ExpressionKind::ChannelClose(value) => {
                self.expression(value, locals, owner);
                self.unknown(&expression.ty)
            }
            ExpressionKind::Channel { capacity, .. } => {
                self.expression(capacity, locals, owner);
                self.unknown(&expression.ty)
            }
            ExpressionKind::ChannelSend { channel, value } => {
                self.expression(channel, locals, owner);
                self.expression(value, locals, owner);
                self.unknown(&expression.ty)
            }
            ExpressionKind::Unary { operator, value } => {
                let fact = self.expression(value, locals, owner);
                unary(*operator, &fact).unwrap_or_else(|| self.unknown(&expression.ty))
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let left_fact = self.expression(left, locals, owner);
                let right_fact = self.expression(right, locals, owner);
                let inferred = binary(*operator, &left_fact, &right_fact);
                if let Some(Fact { domain, .. }) = &inferred
                    && matches!(domain.as_slice(), [Atom::Bool(_)])
                    && matches!(
                        operator,
                        Binary::Equal
                            | Binary::NotEqual
                            | Binary::Less
                            | Binary::LessEqual
                            | Binary::Greater
                            | Binary::GreaterEqual
                    )
                    && (!matches!(left.kind, ExpressionKind::Constant(_))
                        || !matches!(right.kind, ExpressionKind::Constant(_)))
                {
                    self.warning(
                        expression.span,
                        "comparison is determined by known input/value bounds",
                        left_fact.evidence,
                    );
                }
                inferred.unwrap_or_else(|| self.unknown(&expression.ty))
            }
            ExpressionKind::Collection {
                operation,
                handler,
                arguments,
                ..
            } => {
                let facts: Vec<_> = arguments
                    .iter()
                    .map(|value| self.expression(value, locals, owner))
                    .collect();
                self.collection(*operation, *handler, arguments, &facts, expression, owner)
            }
            ExpressionKind::Intrinsic { arguments, .. } | ExpressionKind::List(arguments) => {
                for value in arguments {
                    self.expression(value, locals, owner);
                }
                self.unknown(&expression.ty)
            }
            ExpressionKind::Map(entries) => {
                for (key, value) in entries {
                    self.expression(key, locals, owner);
                    self.expression(value, locals, owner);
                }
                self.unknown(&expression.ty)
            }
        };
        if result.evidence.is_none() {
            result.evidence = Some(expression.span);
        }
        result
    }

    fn call(
        &mut self,
        target: CallTarget,
        arguments: &[CallArgument],
        values: &[Option<Fact>],
        expression: &Expression,
        owner: &Function,
    ) -> Fact {
        let outputs = match target {
            CallTarget::Function(id) | CallTarget::Handler(HandlerTarget::Function(id)) => {
                let callee = self.program.functions[id].clone();
                let summary = self.function(id);
                for (parameter, argument) in callee.parameters.iter().zip(arguments) {
                    if let (
                        Parameter::Handler(signature),
                        CallArgument::Handler(HandlerTarget::Function(handler)),
                    ) = (parameter, argument)
                    {
                        let output = self.function(*handler);
                        let handler_function = &self.program.functions[*handler];
                        let inputs: Vec<_> = signature
                            .parameters
                            .iter()
                            .zip(&signature.bounds)
                            .map(|(ty, bounds)| {
                                Some(if bounds.is_empty() {
                                    self.unknown(ty)
                                } else {
                                    Fact::domain(bounds.clone())
                                })
                            })
                            .collect();
                        for (fact, field) in output.iter().zip(&signature.outputs) {
                            let fact = self.instantiate(fact, handler_function, &inputs);
                            self.require(
                                &fact,
                                &field.bounds,
                                expression.span,
                                "handler output",
                                callee.span,
                            );
                        }
                    }
                }
                let domains: Vec<_> = values.iter().flatten().map(|fact| &fact.domain).collect();
                let excluded = callee
                    .clauses
                    .iter()
                    .filter(|clause| {
                        clause
                            .patterns
                            .iter()
                            .zip(&domains)
                            .any(|(pattern, domain)| disjoint(pattern, domain))
                    })
                    .count();
                if excluded > 0 && arguments.iter().any(|argument| matches!(argument, CallArgument::Value(value) if !matches!(value.kind, ExpressionKind::Constant(_)))) {
                    self.warning(expression.span, &format!("{excluded} clause(s) of '{}' cannot run for the known argument domains", callee.name), values.iter().flatten().find_map(|fact| fact.evidence));
                }
                summary
                    .iter()
                    .map(|fact| self.instantiate(fact, &callee, values))
                    .collect()
            }
            CallTarget::Handler(HandlerTarget::Parameter(parameter)) => {
                let Parameter::Handler(signature) = &owner.parameters[parameter] else {
                    unreachable!("handler parameter")
                };
                for (fact, bounds) in values.iter().flatten().zip(&signature.bounds) {
                    self.require(fact, bounds, expression.span, "handler input", owner.span);
                }
                signature
                    .outputs
                    .iter()
                    .map(|field| self.field_fact(field))
                    .collect()
            }
        };
        output_fact(outputs)
    }

    fn instantiate(&self, fact: &Fact, callee: &Function, arguments: &[Option<Fact>]) -> Fact {
        self.instantiate_cached(fact, callee, arguments, &mut BTreeMap::new())
    }

    fn instantiate_cached(
        &self,
        fact: &Fact,
        callee: &Function,
        arguments: &[Option<Fact>],
        cache: &mut BTreeMap<usize, Rc<FactFields>>,
    ) -> Fact {
        if !self.work.spend() {
            return Fact::domain(vec![Atom::Any]);
        }
        if let Some(origin) = &fact.origin {
            let mut transferred = arguments[origin.parameter]
                .as_ref()
                .expect("value origin")
                .clone();
            let mut ty = callee.parameters[origin.parameter]
                .value_type()
                .expect("value origin")
                .clone();
            for index in &origin.fields {
                transferred = self.project(&transferred, &ty, *index);
                let Some(fields) = self.fields(&ty) else {
                    return fact.clone();
                };
                ty = fields[*index].ty.clone();
            }
            return transferred;
        }
        let mut instantiated = fact.clone();
        let key = Rc::as_ptr(&fact.fields) as usize;
        instantiated.fields = if let Some(fields) = cache.get(&key) {
            Rc::clone(fields)
        } else {
            let fields = Rc::new(
                fact.fields
                    .iter()
                    .map(|(index, field)| {
                        (
                            *index,
                            Rc::new(self.instantiate_cached(field, callee, arguments, cache)),
                        )
                    })
                    .collect(),
            );
            cache.insert(key, Rc::clone(&fields));
            fields
        };
        instantiated
    }

    fn collection(
        &mut self,
        operation: CollectionOp,
        handler: Option<HandlerTarget>,
        arguments: &[Expression],
        facts: &[Fact],
        expression: &Expression,
        owner: &Function,
    ) -> Fact {
        if let Some(HandlerTarget::Parameter(parameter)) = handler {
            let Parameter::Handler(signature) = &owner.parameters[parameter] else {
                unreachable!("handler parameter")
            };
            let mut inputs: Vec<_> = signature
                .parameters
                .iter()
                .map(|ty| self.unknown(ty))
                .collect();
            // Traversals receive one unknown element plus context/state. Bounds
            // on an element are not implied by a List<Int> or Stream<Int> type.
            if inputs.len() > 1 {
                let context = if operation == CollectionOp::ParallelEach {
                    2
                } else {
                    1
                };
                if let Some(fact) = facts.get(context) {
                    inputs[1] = fact.clone();
                }
            }
            if matches!(operation, CollectionOp::Reduce | CollectionOp::ReduceUntil)
                && let Some(state) = inputs.get_mut(1)
            {
                *state = state.merge(&self.field_fact(&signature.outputs[0]), &self.work);
            }
            for (fact, bounds) in inputs.iter().zip(&signature.bounds) {
                self.require(
                    fact,
                    bounds,
                    expression.span,
                    "sequence handler input",
                    owner.span,
                );
            }
        } else if let Some(HandlerTarget::Function(id)) = handler {
            self.function(id);
        }
        if operation == CollectionOp::Length && !arguments.is_empty() {
            return Fact::domain(vec![Atom::Int(0, i64::MAX)]);
        }
        self.unknown(&expression.ty)
    }

    fn warning(&mut self, span: Span, message: &str, evidence: Option<Span>) {
        let mut diagnostic = Diagnostic::error("W002", message, span);
        if let Some(span) = evidence {
            diagnostic.related.push(Label {
                span,
                message: "value guarantee established here".into(),
            });
        }
        self.warnings.push(diagnostic);
    }
}

fn output_fact(mut outputs: Vec<Fact>) -> Fact {
    if outputs.len() == 1 {
        return outputs.remove(0);
    }
    let mut result = Fact::domain(vec![Atom::Any]);
    result.fields = Rc::new(
        outputs
            .into_iter()
            .enumerate()
            .map(|(index, fact)| (index, Rc::new(fact)))
            .collect(),
    );
    result
}
