use crate::diagnostic::Diagnostic;
use crate::hir::{CallArgument, CollectionOp, Expression, ExpressionKind, SequenceKind};
use crate::source::Span;
use crate::syntax::{ContextualExpression, Expression as SourceExpression, ExpressionKind as Ast};
use crate::types::{Field, Type, output_type};

use super::expressions::ungroup;
use super::{Checked, body::Body};

impl Body<'_, '_> {
    pub(super) fn contextual(
        &mut self,
        operation: &ContextualExpression,
        span: Span,
    ) -> Checked<Expression> {
        match operation {
            ContextualExpression::Channel { element, capacity } => {
                let element = self.context.resolve_type(element)?;
                super::symbols::validate_type_constraints(&element, self.context.types, span)?;
                if !element.transferable(self.context.types) {
                    return Err(Diagnostic::error(
                        "C005",
                        "a Channel item type must be transferable between threads",
                        span,
                    ));
                }
                let capacity = self.capacity(
                    capacity,
                    dever_runtime::channel::MAX_CHANNEL_CAPACITY,
                    "channel capacity",
                )?;
                Ok(Expression {
                    kind: ExpressionKind::Channel {
                        element: element.clone(),
                        capacity: Box::new(capacity),
                    },
                    ty: Type::Channel(Box::new(element)),
                    span,
                })
            }
        }
    }

    pub(super) fn named_concurrency(
        &mut self,
        name: &str,
        arguments: &[SourceExpression],
        span: Span,
    ) -> Option<Checked<Expression>> {
        let result = match name {
            "run" => self.run(arguments, span),
            "wait" => self.wait(arguments, span),
            "stop" => self.stop(arguments, span),
            "group" => self.group(arguments, span),
            "parallel" => self.parallel(arguments, span),
            "blocking" => self.blocking(arguments, span),
            "send" => self.send(arguments, span),
            "receive" => self.receive(arguments, span),
            "close" => self.close(arguments, span),
            "timeout" => self.timeout(arguments, span),
            "race" => self.race(arguments, span),
            "stream" => self.stream(arguments, span),
            _ => return None,
        };
        Some(result)
    }

    fn timeout(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("timeout", arguments, 3, span)?;
        let (task, Type::Task(outputs)) = self.affine_operand(&arguments[0], true)? else {
            return Err(Diagnostic::error("C005", "timeout requires a Task", span));
        };
        let duration = self.expression(&arguments[1], Some(&Type::Int))?;
        let handler = self.handler_reference(
            &arguments[2],
            &crate::types::HandlerSignature {
                parameters: vec![],
                bounds: vec![],
                outputs: outputs.clone(),
            },
        )?;
        Ok(Expression {
            kind: ExpressionKind::Intrinsic {
                operation: crate::intrinsic::Intrinsic::TaskTimeout,
                handler: Some(handler),
                arguments: vec![task, duration],
            },
            ty: output_type(&outputs),
            span,
        })
    }

    fn race(&mut self, sources: &[SourceExpression], span: Span) -> Checked<Expression> {
        if sources.len() < 2 {
            return Err(Diagnostic::error(
                "C005",
                "race requires at least two Tasks",
                span,
            ));
        }
        let mut arguments = Vec::new();
        let mut signature = None;
        for source in sources {
            let (task, Type::Task(outputs)) = self.affine_operand(source, true)? else {
                return Err(Diagnostic::error(
                    "C005",
                    "race requires Tasks",
                    source.span,
                ));
            };
            if signature
                .as_ref()
                .is_some_and(|expected| expected != &outputs)
            {
                return Err(Diagnostic::error(
                    "C005",
                    "race Tasks must have identical output signatures",
                    source.span,
                ));
            }
            signature = Some(outputs);
            arguments.push(task);
        }
        Ok(Expression {
            kind: ExpressionKind::Intrinsic {
                operation: crate::intrinsic::Intrinsic::TaskRace,
                handler: None,
                arguments,
            },
            ty: output_type(&signature.expect("at least two tasks")),
            span,
        })
    }

    fn stream(&mut self, sources: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("stream", sources, 1, span)?;
        let value = self.expression(&sources[0], None)?;
        let element = match &value.ty {
            Type::Channel(element) | Type::List(element) => element.as_ref().clone(),
            Type::Bytes => Type::Int,
            _ => {
                return Err(Diagnostic::error(
                    "C005",
                    "stream requires Channel, List or Bytes",
                    span,
                ));
            }
        };
        if !element.transferable(self.context.types) {
            return Err(Diagnostic::error(
                "C005",
                "async stream elements must be transferable",
                span,
            ));
        }
        Ok(Expression {
            kind: ExpressionKind::Intrinsic {
                operation: crate::intrinsic::Intrinsic::StreamOf,
                handler: None,
                arguments: vec![value],
            },
            ty: Type::AsyncStream(Box::new(element)),
            span,
        })
    }

    fn wait(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("wait", arguments, 1, span)?;
        let (task, ty) = self.affine_operand(&arguments[0], true)?;
        match ty {
            Type::Task(outputs) => Ok(Expression {
                kind: ExpressionKind::AwaitTask(Box::new(task)),
                ty: output_type(&outputs),
                span,
            }),
            Type::Group => Ok(Expression {
                kind: ExpressionKind::AwaitGroup(Box::new(task)),
                ty: Type::Unit,
                span,
            }),
            _ => unreachable!("affine operands are Task or Group"),
        }
    }

    fn run(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        match arguments {
            [call] => {
                let call = self.checked_static_call(call)?;
                self.require_transferable_call(&call.arguments, &call.outputs, span)?;
                let outputs = call.outputs.clone();
                Ok(Expression {
                    kind: ExpressionKind::RunCall {
                        group: None,
                        target: call.target,
                        arguments: call.arguments,
                    },
                    ty: Type::Task(outputs),
                    span,
                })
            }
            [group, call] => {
                let (group, ty) = self.affine_operand(group, false)?;
                if ty != Type::Group {
                    return Err(Diagnostic::error(
                        "C005",
                        "the first argument to run(group, call) must be a Group local",
                        arguments[0].span,
                    ));
                }
                let call = self.checked_static_call(call)?;
                if !call.outputs.is_empty() {
                    return Err(Diagnostic::error(
                        "C005",
                        "a Group accepts only zero-output calls",
                        arguments[1].span,
                    ));
                }
                self.require_transferable_call(&call.arguments, &call.outputs, span)?;
                Ok(Expression {
                    kind: ExpressionKind::RunCall {
                        group: Some(Box::new(group)),
                        target: call.target,
                        arguments: call.arguments,
                    },
                    ty: Type::Unit,
                    span,
                })
            }
            _ => Err(Diagnostic::error(
                "C005",
                "'run' expects a call, optionally preceded by a Group",
                span,
            )),
        }
    }

    fn stop(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("stop", arguments, 1, span)?;
        let (value, ty) = self.affine_operand(&arguments[0], true)?;
        let kind = match ty {
            Type::Task(outputs) if outputs.is_empty() => ExpressionKind::StopTask(Box::new(value)),
            Type::Task(_) => {
                return Err(Diagnostic::error(
                    "C005",
                    "stop accepts only a zero-output Task; wait for result-bearing tasks",
                    arguments[0].span,
                ));
            }
            Type::Group => ExpressionKind::StopGroup(Box::new(value)),
            _ => unreachable!("affine operands are Task or Group"),
        };
        Ok(Expression {
            kind,
            ty: Type::Unit,
            span,
        })
    }

    fn group(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("group", arguments, 1, span)?;
        let limit = self.capacity(
            &arguments[0],
            dever_runtime::task::MAX_GROUP_TASKS,
            "group limit",
        )?;
        Ok(Expression {
            kind: ExpressionKind::Group(Box::new(limit)),
            ty: Type::Group,
            span,
        })
    }

    fn parallel(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("parallel", arguments, 1, span)?;
        let call = self.checked_static_call(&arguments[0])?;
        if !call.pure {
            return Err(Diagnostic::error(
                "C005",
                "parallel requires a pure function call",
                arguments[0].span,
            ));
        }
        self.require_transferable_call(&call.arguments, &call.outputs, span)?;
        Ok(Expression {
            kind: ExpressionKind::ParallelCall {
                target: call.target,
                arguments: call.arguments,
            },
            ty: output_type(&call.outputs),
            span,
        })
    }

    fn blocking(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("blocking", arguments, 1, span)?;
        let call = self.checked_static_call(&arguments[0])?;
        self.require_transferable_call(&call.arguments, &call.outputs, span)?;
        Ok(Expression {
            kind: ExpressionKind::BlockingCall {
                target: call.target,
                arguments: call.arguments,
            },
            ty: output_type(&call.outputs),
            span,
        })
    }

    fn send(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("send", arguments, 2, span)?;
        let channel = self.expression(&arguments[0], None)?;
        let Type::Channel(element) = &channel.ty else {
            return Err(Diagnostic::error(
                "C005",
                "send requires a Channel as its first argument",
                arguments[0].span,
            ));
        };
        let value = self.expression(&arguments[1], Some(element))?;
        Ok(Expression {
            kind: ExpressionKind::ChannelSend {
                channel: Box::new(channel),
                value: Box::new(value),
            },
            ty: Type::Unit,
            span,
        })
    }

    fn receive(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("receive", arguments, 1, span)?;
        let channel = self.expression(&arguments[0], None)?;
        let Type::Channel(element) = &channel.ty else {
            return Err(Diagnostic::error(
                "C005",
                "receive requires a Channel value",
                arguments[0].span,
            ));
        };
        let ty = element.as_ref().clone().nullable();
        Ok(Expression {
            kind: ExpressionKind::ChannelReceive(Box::new(channel)),
            ty,
            span,
        })
    }

    fn close(&mut self, arguments: &[SourceExpression], span: Span) -> Checked<Expression> {
        exact_arity("close", arguments, 1, span)?;
        let value = self.expression(&arguments[0], None)?;
        let kind = match &value.ty {
            Type::Channel(_) => ExpressionKind::ChannelClose(Box::new(value)),
            Type::Stream(_) | Type::AsyncStream(_) | Type::RowStream(_) => {
                ExpressionKind::Collection {
                    operation: CollectionOp::Close,
                    handler: None,
                    sequence: Some(match value.ty {
                        Type::AsyncStream(_) => SequenceKind::AsyncStream,
                        Type::RowStream(_) => SequenceKind::RowStream,
                        _ => SequenceKind::Stream,
                    }),
                    arguments: vec![value],
                }
            }
            _ => {
                return Err(Diagnostic::error(
                    "C005",
                    "close requires a Stream, AsyncStream or Channel value",
                    arguments[0].span,
                ));
            }
        };
        Ok(Expression {
            kind,
            ty: Type::Unit,
            span,
        })
    }

    fn affine_operand(
        &mut self,
        source: &SourceExpression,
        consume: bool,
    ) -> Checked<(Expression, Type)> {
        let Ast::Name(path) = &ungroup(source).kind else {
            return Err(Diagnostic::error(
                "C005",
                "Task and Group operations require a direct local name",
                source.span,
            ));
        };
        if path.len() != 1 {
            return Err(Diagnostic::error(
                "C005",
                "Task and Group cannot be stored or projected",
                source.span,
            ));
        }
        let (slot, ty) = self.affine_local(&path[0].text, source.span, consume)?;
        Ok((
            Expression {
                kind: ExpressionKind::Local(slot),
                ty: ty.clone(),
                span: source.span,
            },
            ty,
        ))
    }

    fn capacity(
        &mut self,
        source: &SourceExpression,
        maximum: usize,
        label: &str,
    ) -> Checked<Expression> {
        let value = self.expression(source, Some(&Type::Int))?;
        if let ExpressionKind::Constant(crate::hir::Constant::Int(constant)) = &value.kind
            && (*constant <= 0
                || usize::try_from(*constant).map_or(true, |constant| constant > maximum))
        {
            return Err(Diagnostic::error(
                "C005",
                format!("{label} must be between 1 and {maximum}"),
                source.span,
            ));
        }
        Ok(value)
    }

    fn require_transferable_call(
        &self,
        arguments: &[CallArgument],
        outputs: &[Field],
        span: Span,
    ) -> Checked<()> {
        let arguments_transfer = arguments.iter().all(|argument| match argument {
            CallArgument::Value(value) => value.ty.transferable(self.context.types),
            CallArgument::Handler(_) => true,
        });
        let outputs_transfer = outputs
            .iter()
            .all(|field| field.ty.transferable(self.context.types));
        if arguments_transfer && outputs_transfer {
            Ok(())
        } else {
            Err(Diagnostic::error(
                "C005",
                "task inputs and outputs must be transferable between threads",
                span,
            ))
        }
    }
}

fn exact_arity(
    name: &str,
    arguments: &[SourceExpression],
    expected: usize,
    span: Span,
) -> Checked<()> {
    if arguments.len() == expected {
        Ok(())
    } else {
        Err(Diagnostic::error(
            "C005",
            format!("'{name}' expects {expected} argument(s)"),
            span,
        ))
    }
}
