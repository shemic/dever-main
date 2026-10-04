use super::{Evaluator, ResultValue, RuntimeFault, Value, expressions::binary, value::Key};
use crate::{
    hir::{CollectionOp as Op, Expression},
    specialize::Specialization,
    syntax::BinaryOperator,
    types::Type,
};
use dever_runtime::{collections::List, number::DecimalValue};

impl Evaluator<'_> {
    pub(super) fn collection(
        &mut self,
        operation: Op,
        handler: Option<usize>,
        arguments: Vec<Value>,
        result: &Expression,
    ) -> ResultValue {
        if operation == Op::ParallelEach {
            return self.parallel_each(handler, arguments, result);
        }
        let mut arguments = arguments.into_iter();
        let sequence = arguments.next().expect("collection source");
        match operation {
            Op::Append => {
                let Value::List(list) = sequence else {
                    unreachable!()
                };
                return Ok(Value::List(list.append(arguments.next().unwrap())));
            }
            Op::First => {
                let Value::List(list) = sequence else {
                    unreachable!()
                };
                return Ok(list.values().first().cloned().unwrap_or(Value::Null));
            }
            Op::Get | Op::Put | Op::Remove => {
                let Value::Map(map) = sequence else {
                    unreachable!()
                };
                let key = Key::from_value(&arguments.next().unwrap());
                return Ok(match operation {
                    Op::Get => map.get(&key).unwrap_or(Value::Null),
                    Op::Put => Value::Map(map.put(key, arguments.next().unwrap())),
                    Op::Remove => Value::Map(map.remove(&key)),
                    _ => unreachable!(),
                });
            }
            Op::Entries => {
                let Value::Map(map) = sequence else {
                    unreachable!()
                };
                return Ok(Value::List(List::new(
                    map.pairs()
                        .map(|(key, value)| Value::Fields(vec![key.value(), value.clone()]))
                        .collect(),
                )));
            }
            Op::Length => {
                return Ok(Value::Int(match sequence {
                    Value::List(list) => list.values().len() as i64,
                    Value::Map(map) => map.pairs().count() as i64,
                    Value::Text(text) => text.chars().count() as i64,
                    Value::Bytes(bytes) => bytes.length(),
                    _ => unreachable!("checked length input"),
                }));
            }
            Op::Close => {
                let Value::Stream(stream) = sequence else {
                    unreachable!()
                };
                stream.close();
                return Ok(Value::Unit);
            }
            Op::Each | Op::Reduce | Op::ReduceUntil | Op::Filter | Op::Find | Op::Sum => {}
            Op::ParallelEach => unreachable!("parallel schedule handled before traversal"),
        }
        let context = arguments.next();
        let reducing = matches!(operation, Op::Reduce | Op::ReduceUntil);
        let mut state = if reducing {
            context.clone().unwrap()
        } else {
            match result.ty {
                Type::Int => Value::Int(0),
                Type::Decimal => Value::Decimal(DecimalValue::ZERO),
                Type::Float => Value::Float(0.0),
                _ => Value::Unit,
            }
        };
        let mut mapped = Vec::new();
        let mut cursor = 0;
        loop {
            let element = match &sequence {
                Value::List(list) => list.values().get(cursor).cloned(),
                Value::Bytes(bytes) => bytes
                    .values()
                    .get(cursor)
                    .map(|byte| Value::Int(i64::from(*byte))),
                Value::Stream(stream) => stream.pull(),
                _ => unreachable!("checked sequence"),
            };
            let Some(element) = element else {
                break;
            };
            cursor += 1;
            let projected = if let Some(handler) = handler {
                let mut inputs = vec![element.clone()];
                if reducing {
                    inputs.push(state.clone());
                } else if let Some(context) = &context {
                    inputs.push(context.clone());
                }
                self.call(
                    Specialization {
                        function: handler,
                        handlers: vec![],
                    },
                    inputs,
                )
                .map_err(|fault| fault.at(result.span))?
            } else {
                element.clone()
            };
            match operation {
                Op::Each => {
                    if result.ty != Type::Unit {
                        mapped.push(projected);
                    }
                }
                Op::Filter => {
                    if projected.boolean() {
                        mapped.push(element);
                    }
                }
                Op::Find => {
                    if projected.boolean() {
                        return Ok(element);
                    }
                }
                Op::Sum => {
                    state = binary(state, BinaryOperator::Add, projected)
                        .map_err(|message| RuntimeFault::new(message, result.span))?;
                }
                Op::Reduce => state = projected,
                Op::ReduceUntil => {
                    let stop = projected.fields()[1].boolean();
                    state = projected.fields()[0].clone();
                    if stop {
                        break;
                    }
                }
                _ => unreachable!(),
            }
        }
        Ok(match operation {
            Op::Each if result.ty == Type::Unit => Value::Unit,
            Op::Each | Op::Filter => Value::List(List::new(mapped)),
            Op::Find => Value::Null,
            Op::Sum | Op::Reduce | Op::ReduceUntil => state,
            _ => unreachable!(),
        })
    }

    /// The oracle chooses a serial schedule. Concurrent side-effect ordering is deliberately
    /// unspecified, so native concurrency tests must assert invariants rather than stdout order.
    fn parallel_each(
        &mut self,
        handler: Option<usize>,
        mut arguments: Vec<Value>,
        result: &Expression,
    ) -> ResultValue {
        let workers = arguments.remove(1).int();
        dever_runtime::concurrent::worker_count(workers)
            .map_err(|message| RuntimeFault::new(message, result.span))?;
        self.collection(Op::Each, handler, arguments, result)
    }
}
