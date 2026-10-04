use crate::hir::{CollectionOp as Op, Expression, HandlerTarget, SequenceKind};
use crate::specialize::{self, Specialization};
use crate::types::Type;

use super::{DatabaseContext, Emitter, optional_result, rust_type};

impl Emitter<'_> {
    pub(super) fn collection(
        &self,
        operation: Op,
        handler: Option<HandlerTarget>,
        sequence: Option<SequenceKind>,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        let traverses = matches!(
            operation,
            Op::Each | Op::Reduce | Op::ReduceUntil | Op::Filter | Op::Find | Op::Sum
        );
        let split = if traverses {
            self.split_arguments(&arguments[0])
        } else {
            None
        };
        let pipeline = if traverses {
            self.fused_iterator(&arguments[0])
        } else {
            None
        };
        let mut bindings = if let Some((bindings, _)) = &pipeline {
            bindings.clone()
        } else if let Some([text, separator]) = split {
            format!(
                "let a0_text = {}; let a0_separator = {};",
                self.expression(text),
                self.expression(separator)
            )
        } else {
            self.bind_arguments(arguments)
        };
        if pipeline.is_some() || split.is_some() {
            for (index, argument) in arguments.iter().enumerate().skip(1) {
                bindings.push_str(&format!(
                    "let a{index}: {} = {};",
                    rust_type(&argument.ty),
                    self.expression(argument)
                ));
            }
        }
        if operation == Op::ParallelEach {
            let target = Specialization {
                function: specialize::resolve_handler(
                    handler.expect("checked parallel handler"),
                    &self.bindings,
                ),
                handlers: Vec::new(),
            };
            let async_handler = specialize::suspends(self.program, &target);
            let sequence = sequence.expect("checked parallel sequence");
            let (dispatch, values, element) = match sequence {
                SequenceKind::AsyncStream => ("parallel_each_stream_typed", "a0", "element"),
                SequenceKind::List if async_handler => (
                    "parallel_each_stream_typed",
                    "dever_runtime::async_stream::AsyncStream::from_values(a0.into_values())",
                    "element",
                ),
                SequenceKind::Bytes if async_handler => (
                    "parallel_each_stream_typed",
                    "dever_runtime::async_stream::AsyncStream::from_values(a0.into_values().map(i64::from))",
                    "element",
                ),
                SequenceKind::List if self.async_context => {
                    ("parallel_each_typed", "a0.into_values()", "element")
                }
                SequenceKind::Bytes if self.async_context => (
                    "parallel_each_typed",
                    "a0.into_values()",
                    "i64::from(element)",
                ),
                SequenceKind::List => ("each_slice_typed", "a0.values()", "element.clone()"),
                SequenceKind::Bytes => ("each_slice_typed", "a0.values()", "i64::from(*element)"),
                SequenceKind::Stream => {
                    ("each_typed", "std::iter::from_fn(|| a0.pull())", "element")
                }
                SequenceKind::RowStream => unreachable!("checked sequential database stream"),
            };
            let context = if arguments.len() == 3 {
                ", a2.clone()"
            } else {
                ""
            };
            let call = if async_handler {
                let capture = if arguments.len() == 3 {
                    "let context = a2.clone();"
                } else {
                    ""
                };
                let argument = if arguments.len() == 3 {
                    ", context"
                } else {
                    ""
                };
                let invoke = self.invoke(
                    &target,
                    format!("{element}{argument}"),
                    DatabaseContext::Detached,
                );
                format!(
                    "task::{dispatch}({values}, a1, move |element| {{ {capture} async move {{ {invoke}.await }} }}).await"
                )
            } else if sequence == SequenceKind::AsyncStream {
                let capture = if arguments.len() == 3 {
                    "let context = a2.clone();"
                } else {
                    ""
                };
                let argument = if arguments.len() == 3 {
                    ", context"
                } else {
                    ""
                };
                let invoke = self.invoke(
                    &target,
                    format!("{element}{argument}"),
                    DatabaseContext::Detached,
                );
                format!(
                    "task::{dispatch}({values}, a1, move |element| {{ {capture} async move {{ {invoke} }} }}).await"
                )
            } else if self.async_context {
                let invoke = self.invoke(
                    &target,
                    format!("{element}{context}"),
                    DatabaseContext::Detached,
                );
                format!("task::{dispatch}({values}, a1, move |element| {invoke}).await")
            } else {
                let invoke = self.invoke(
                    &target,
                    format!("{element}{context}"),
                    DatabaseContext::Detached,
                );
                format!("dever_runtime::concurrent::{dispatch}({values}, a1, |element| {invoke})")
            };
            return format!(
                "{{ {bindings}\n{} }}",
                self.call_failure(&call, result.span)
            );
        }
        let body = match operation {
            Op::Append => "a0.append(a1)".into(),
            Op::First => optional_result("a0.into_first()", list_element(&arguments[0].ty)),
            Op::Get => {
                let Type::Map(_, value) = &arguments[0].ty else {
                    unreachable!()
                };
                optional_result("a0.into_get(&a1)", value)
            }
            Op::Put => "a0.put(a1, a2)".into(),
            Op::Remove => "a0.remove(&a1)".into(),
            Op::Entries => "a0.into_entries()".into(),
            Op::Length => match &arguments[0].ty {
                Type::List(_) => "a0.values().len() as i64".into(),
                Type::Map(_, _) => "a0.pairs().count() as i64".into(),
                Type::Text => "a0.chars().count() as i64".into(),
                Type::Bytes => "a0.length()".into(),
                _ => unreachable!(),
            },
            Op::Close => "a0.close(); ()".into(),
            _ => {
                let sequence = sequence.expect("checked sequence operation");
                let consume =
                    sequence == SequenceKind::List && arguments[0].ty.movable(&self.program.types);
                let list_values = if let Some((_, iterator)) = &pipeline {
                    iterator.as_str()
                } else if split.is_some() {
                    "dever_runtime::text::pieces(&a0_text, &a0_separator)"
                } else if consume {
                    "a0.into_values()"
                } else {
                    "a0.values()"
                };
                let element = match sequence {
                    SequenceKind::List
                        if consume && !matches!(operation, Op::Filter | Op::Find) =>
                    {
                        "element"
                    }
                    SequenceKind::List => "element.clone()",
                    SequenceKind::Bytes => "i64::from(*element)",
                    SequenceKind::Stream | SequenceKind::AsyncStream | SequenceKind::RowStream => {
                        "element"
                    }
                };
                let call = match handler {
                    Some(handler) => {
                        let target = Specialization {
                            function: specialize::resolve_handler(handler, &self.bindings),
                            handlers: Vec::new(),
                        };
                        let state = if matches!(operation, Op::Reduce | Op::ReduceUntil) {
                            ", state"
                        } else if arguments.len() == 2 {
                            ", a1.clone()"
                        } else {
                            ""
                        };
                        let wait = if specialize::suspends(self.program, &target) {
                            ".await"
                        } else {
                            ""
                        };
                        let invoke = self.invoke(
                            &target,
                            format!("{element}{state}"),
                            DatabaseContext::Inherit,
                        );
                        self.call_failure(&format!("{invoke}{wait}"), result.span)
                    }
                    None => element.into(),
                };
                let loop_over = |body: &str| match sequence {
                    SequenceKind::List if consume => {
                        format!("for element in {list_values} {{ {body} }}")
                    }
                    SequenceKind::List | SequenceKind::Bytes => {
                        format!("for element in a0.values() {{ {body} }}")
                    }
                    SequenceKind::Stream => {
                        format!("while let Some(element) = a0.pull() {{ {body} }}")
                    }
                    SequenceKind::AsyncStream => {
                        format!(
                            "let _stream_owner = a0.close_on_drop(); while let Some(element) = a0.pull().await {{ {body} }}"
                        )
                    }
                    SequenceKind::RowStream => {
                        let pull = self.database_failure("a0.pull().await", result.span);
                        format!(
                            "let _stream_owner = a0.close_on_drop(); while let Some(element) = {pull} {{ {body} }}"
                        )
                    }
                };
                match operation {
                    Op::Each if result.ty == Type::Unit => loop_over(&format!("{call};")),
                    Op::Each => {
                        let capacity = match sequence {
                            SequenceKind::List if split.is_some() || pipeline.is_some() => "0",
                            SequenceKind::List => "a0.values().len()",
                            SequenceKind::Bytes => "a0.values().len()",
                            SequenceKind::Stream
                            | SequenceKind::AsyncStream
                            | SequenceKind::RowStream => unreachable!("checked stream each output"),
                        };
                        format!(
                            "let mut values = Vec::with_capacity({capacity}); {} List::new(values)",
                            loop_over(&format!("values.push({call});"))
                        )
                    }
                    Op::Filter => {
                        let selected = if consume {
                            "element"
                        } else {
                            "element.clone()"
                        };
                        format!(
                            "let mut values = Vec::new(); for element in {list_values} {{ if {call} {{ values.push({selected}); }} }} List::new(values)"
                        )
                    }
                    Op::Find => {
                        let selected = if consume {
                            "element"
                        } else {
                            "element.clone()"
                        };
                        let found = if matches!(list_element(&arguments[0].ty), Type::Nullable(_)) {
                            selected.to_owned()
                        } else {
                            format!("Some({selected})")
                        };
                        format!(
                            "let mut found = None; for element in {list_values} {{ if {call} {{ found = {found}; break; }} }} found"
                        )
                    }
                    Op::Sum => {
                        let zero = match result.ty {
                            Type::Int => "0i64",
                            Type::Decimal => "DecimalValue::ZERO",
                            Type::Float => "0.0f64",
                            _ => unreachable!(),
                        };
                        let add = match result.ty {
                            Type::Int => {
                                self.failure("number::int_add(total, projected)", result.span)
                            }
                            Type::Decimal => {
                                self.failure("total.checked_add(projected)", result.span)
                            }
                            Type::Float => "total + projected".into(),
                            _ => unreachable!(),
                        };
                        format!(
                            "let mut total: {} = {zero}; for element in {list_values} {{ let projected = {call}; total = {add}; }} total",
                            rust_type(&result.ty)
                        )
                    }
                    Op::Reduce => format!(
                        "let mut state = a1; {} state",
                        loop_over(&format!("state = {call};"))
                    ),
                    Op::ReduceUntil => format!(
                        "let mut state = a1; {} state",
                        loop_over(&format!(
                            "let step = {call}; state = step.0; if step.1 {{ break; }}"
                        ))
                    ),
                    _ => unreachable!(),
                }
            }
        };
        format!("{{ {bindings}\n{body} }}")
    }
}

fn list_element(ty: &Type) -> &Type {
    let Type::List(element) = ty else {
        unreachable!()
    };
    element
}
