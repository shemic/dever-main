//! Opt-in executable HIR specification for conformance tests; never a production backend.
use std::collections::BTreeMap;

use crate::hir::{Atom, Program, Projection, Statement};
use crate::source::{SourceMap, Span};
use crate::specialize::{self, Specialization};

mod collections;
mod expressions;
mod intrinsics;
mod value;
use value::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeFault {
    pub message: String,
    /// Innermost operation first, followed by its Dever call sites.
    pub spans: Vec<Span>,
    business: Option<Value>,
}

impl RuntimeFault {
    fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            spans: vec![span],
            business: None,
        }
    }

    fn business(value: Value, message: String, span: Span) -> Self {
        Self {
            message,
            spans: vec![span],
            business: Some(value),
        }
    }

    fn at(mut self, span: Span) -> Self {
        self.spans.push(span);
        self
    }

    fn into_business(
        mut self,
        targets: &BTreeMap<crate::hir::Failure, crate::capture::Target>,
        choice: usize,
    ) -> Result<Value, Self> {
        let Some(Value::Variant(ty, variant, _)) = &self.business else {
            return Err(self);
        };
        let Some(target) = targets.get(&crate::hir::Failure {
            ty: *ty,
            variant: *variant,
        }) else {
            return Err(self);
        };
        let error = self.business.take().expect("matched business error");
        Ok(if target.wrapped {
            Value::Variant(choice, target.variant, vec![error])
        } else {
            error
        })
    }

    pub fn render(&self, sources: &SourceMap) -> String {
        let mut locations = self
            .spans
            .iter()
            .rev()
            .map(|span| {
                let source = sources.get(span.source);
                let (line, column) = source.position(span.start);
                format!("{}:{line}:{column}", source.path().display())
            })
            .collect::<Vec<_>>();
        locations.push(self.message.clone());
        locations.join(": ")
    }
}

#[derive(Debug)]
pub struct Evaluation {
    /// Includes observable writes and successful entry named outputs, like a native process.
    pub stdout: String,
    pub fault: Option<RuntimeFault>,
}

pub fn evaluate(program: &Program, entry: &str) -> Result<Evaluation, String> {
    let id = program.entry(entry)?;
    let mut evaluator = Evaluator {
        program,
        stdout: String::new(),
    };
    let result = evaluator.call(
        Specialization {
            function: id,
            handlers: vec![],
        },
        vec![],
    );
    let fault = match result {
        Ok(result) => {
            let outputs = &program.functions[id].outputs;
            let values = if outputs.len() > 1 {
                result.fields().to_vec()
            } else {
                vec![result]
            };
            for (field, value) in outputs.iter().zip(values) {
                evaluator
                    .stdout
                    .push_str(&format!("{} = {}\n", field.name, value.render(program)));
            }
            None
        }
        Err(fault) => Some(fault),
    };
    Ok(Evaluation {
        stdout: evaluator.stdout,
        fault,
    })
}

type ResultValue = Result<Value, RuntimeFault>;

struct Evaluator<'a> {
    program: &'a Program,
    stdout: String,
}

struct Frame {
    locals: Vec<Option<Value>>,
    handlers: BTreeMap<usize, usize>,
}

impl Evaluator<'_> {
    fn call(&mut self, instance: Specialization, arguments: Vec<Value>) -> ResultValue {
        let function = &self.program.functions[instance.function];
        if let Some(port) = &function.port {
            if let [implementation] = port.implementations.as_slice() {
                return self.call(
                    Specialization {
                        function: *implementation,
                        handlers: Vec::new(),
                    },
                    arguments,
                );
            }
            return Err(RuntimeFault::new(
                "configured Port dispatch requires native execution",
                function.span,
            ));
        }
        if function.setting.is_some() {
            return Err(RuntimeFault::new(
                "Adapter settings require native execution",
                function.span,
            ));
        }
        if function.external.is_some() {
            return Err(RuntimeFault::new(
                "external Adapter requires native execution",
                function.span,
            ));
        }
        // HIR domains are proven exhaustive and disjoint; source order cannot select a winner.
        let mut matches = function.clauses.iter().filter(|clause| {
            clause
                .patterns
                .iter()
                .zip(&arguments)
                .all(|(domain, value)| domain.iter().any(|atom| matches_atom(atom, value)))
        });
        let clause = matches.next().expect("checked exhaustive clause domains");
        assert!(matches.next().is_none(), "checked disjoint clause domains");
        let mut frame = Frame {
            locals: vec![None; clause.locals.len()],
            handlers: specialize::handler_bindings(function, &instance),
        };
        let inputs: BTreeMap<_, _> = function
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| parameter.value_type().map(|_| index))
            .zip(arguments)
            .collect();
        for binding in &clause.bindings {
            let input = &inputs[&binding.input];
            let value = match binding.projection {
                Projection::Whole | Projection::NonNull => input.clone(),
                Projection::Payload { variant: _, field } => input.fields()[field].clone(),
            };
            frame.locals[binding.slot] = Some(value);
        }
        for statement in &clause.body {
            match statement {
                Statement::Assign {
                    slot,
                    fields,
                    value,
                } => {
                    let value = self.expression(value, &frame)?;
                    if fields.is_empty() {
                        frame.locals[*slot] = Some(value);
                    } else {
                        let mut host = frame.locals[*slot]
                            .as_mut()
                            .expect("initialized assignment host");
                        for index in fields {
                            host = &mut host.fields_mut()[*index];
                        }
                        *host = value;
                    }
                }
                Statement::Call(value) => {
                    self.expression(value, &frame)?;
                }
            }
        }
        let mut outputs = clause
            .outputs
            .iter()
            .map(|slot| frame.locals[*slot].take().expect("initialized output"))
            .collect::<Vec<_>>();
        Ok(match outputs.len() {
            0 => Value::Unit,
            1 => outputs.pop().unwrap(),
            _ => Value::Fields(outputs),
        })
    }
}

fn matches_atom(atom: &Atom, value: &Value) -> bool {
    if matches!(value, Value::Null) {
        return matches!(atom, Atom::Null);
    }
    match atom {
        Atom::Null => false,
        Atom::Any => true,
        Atom::Bool(expected) => value == &Value::Bool(*expected),
        Atom::Variant(expected) => {
            matches!(value, Value::Variant(_, actual, _) if expected == actual)
        }
        Atom::Int(lo, hi) => matches!(value, Value::Int(actual) if lo <= actual && actual <= hi),
        Atom::Decimal(lo, hi) => {
            matches!(value, Value::Decimal(actual) if lo <= actual && actual <= hi)
        }
        Atom::Literal(literal) => value == &Value::constant(literal),
        Atom::Remainder(excluded) => excluded
            .iter()
            .all(|literal| value != &Value::constant(literal)),
    }
}
