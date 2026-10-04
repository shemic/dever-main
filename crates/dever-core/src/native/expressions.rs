use crate::hir::{CallArgument, CallTarget, Expression, ExpressionKind as Expr};
use crate::specialize::{self, Specialization};
use crate::syntax::{BinaryOperator as Op, UnaryOperator};
use crate::types::Type;

use super::{DatabaseContext, Emitter, field_suffix, optional_result, rust_type, typed_constant};

impl Emitter<'_> {
    pub(super) fn expression(&self, value: &Expression) -> String {
        match &value.kind {
            Expr::Constant(constant) => typed_constant(constant, &value.ty),
            Expr::Local(slot) if self.can_move(value) => format!("_v{slot}"),
            Expr::Local(slot) => format!("_v{slot}.clone()"),
            Expr::Field { .. } => self.field_expression(value),
            Expr::ChoiceOptions { choice } => {
                let crate::types::Shape::Choice(variants) = &self.program.types[*choice].shape
                else {
                    unreachable!("checked Model choice options")
                };
                let entries = variants
                    .iter()
                    .enumerate()
                    .map(|(variant, definition)| {
                        format!(
                            "(T{choice}::V{variant}, String::from({}))",
                            super::rust_string(
                                definition.label.as_deref().unwrap_or(&definition.name)
                            )
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Map::<T{choice}, String>::from_unique(vec![{entries}])")
            }
            Expr::ModelOperation { model, operation } => {
                self.model_operation(*model, operation, value)
            }
            Expr::JobEnqueue {
                target,
                arguments,
                scheduled,
            } => self.enqueue_job(*target, arguments, *scheduled, value.span),
            Expr::Call { target, arguments } => {
                let instance = self.call_instance(*target, arguments);
                let call = self.call(&instance, arguments, DatabaseContext::Inherit);
                let call = if specialize::suspends(self.program, &instance) {
                    format!("{call}.await")
                } else {
                    call
                };
                self.call_failure(&call, value.span)
            }
            Expr::Fail(error) => self.fail_expression(error, value),
            Expr::CaptureResult {
                target,
                arguments,
                choice,
                success,
            } => self.capture_result(*target, arguments, *choice, *success, value),
            Expr::RunCall {
                group,
                target,
                arguments,
            } => {
                let instance = self.call_instance(*target, arguments);
                let call = self.call(&instance, arguments, DatabaseContext::Detached);
                let call = if specialize::suspends(self.program, &instance) {
                    call
                } else {
                    format!("task::blocking_typed(move || {call})")
                };
                let operation = match group {
                    Some(group) => format!("{}.run({call}).await", self.group_local(group)),
                    None => format!("task::run_typed({call}).await"),
                };
                if group.is_some() {
                    self.call_failure(&operation, value.span)
                } else {
                    self.failure(&operation, value.span)
                }
            }
            Expr::AwaitTask(task) => self.call_failure(
                &format!("task::wait({}).await", self.expression(task)),
                value.span,
            ),
            Expr::StopTask(task) => self.call_failure(
                &format!("task::stop({}).await", self.expression(task)),
                value.span,
            ),
            Expr::Group(limit) => self.failure(
                &format!(
                    "task::Group::<AppError>::new_typed({})",
                    self.expression(limit)
                ),
                value.span,
            ),
            Expr::AwaitGroup(group) => self.call_failure(
                &format!("task::Group::wait({}).await", self.expression(group)),
                value.span,
            ),
            Expr::StopGroup(group) => self.call_failure(
                &format!("task::Group::stop({}).await", self.expression(group)),
                value.span,
            ),
            Expr::ParallelCall { target, arguments } => {
                self.thread_call("parallel", *target, arguments, value)
            }
            Expr::BlockingCall { target, arguments } => {
                self.thread_call("blocking", *target, arguments, value)
            }
            Expr::Channel { element, capacity } => self.failure(
                &format!(
                    "channel::Channel::<{}>::new({})",
                    rust_type(element),
                    self.expression(capacity)
                ),
                value.span,
            ),
            Expr::ChannelSend {
                channel,
                value: item,
            } => self.failure(
                &format!(
                    "({}).send({}).await",
                    self.expression(channel),
                    self.expression(item)
                ),
                value.span,
            ),
            Expr::ChannelReceive(channel) => {
                let Type::Channel(element) = &channel.ty else {
                    unreachable!("checked channel receive")
                };
                let received = self.failure(
                    &format!("({}).receive().await", self.expression(channel)),
                    value.span,
                );
                optional_result(&received, element)
            }
            Expr::ChannelClose(channel) => self.failure(
                &format!("({}).close().await", self.expression(channel)),
                value.span,
            ),
            Expr::Intrinsic {
                operation,
                handler,
                arguments,
            } => self.intrinsic(*operation, *handler, arguments, value),
            Expr::Collection {
                operation,
                handler,
                sequence,
                arguments,
            } => self.collection(*operation, *handler, *sequence, arguments, value),
            Expr::Record { fields } => {
                let Type::Named(id) = value.ty else {
                    unreachable!()
                };
                // Field expressions execute in source order, independent of declaration order.
                let bindings = fields
                    .iter()
                    .map(|(index, value)| format!("let field{index} = {};", self.expression(value)))
                    .collect::<Vec<_>>()
                    .join("\n");
                let fields = fields
                    .iter()
                    .map(|(index, _)| format!("f{index}: field{index}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ {bindings} T{id} {{ {fields} }} }}")
            }
            Expr::Variant { variant, arguments } => {
                let Type::Named(id) = value.ty else {
                    unreachable!()
                };
                if arguments.is_empty() {
                    format!("T{id}::V{variant}")
                } else {
                    format!("T{id}::V{variant}({})", self.arguments(arguments))
                }
            }
            Expr::List(elements) => format!(
                "{}::new(vec![{}])",
                rust_type(&value.ty).replacen("List<", "List::<", 1),
                self.arguments(elements)
            ),
            Expr::Map(entries) => {
                let entries = entries
                    .iter()
                    .map(|(key, value)| {
                        format!("({}, {})", self.expression(key), self.expression(value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.failure(
                    &format!(
                        "{}::new(vec![{entries}])",
                        rust_type(&value.ty).replacen("Map<", "Map::<", 1)
                    ),
                    value.span,
                )
            }
            Expr::Promote(value) => format!("DecimalValue::from_int({})", self.expression(value)),
            Expr::Some(value) => format!("Some({})", self.expression(value)),
            Expr::Unary {
                operator,
                value: operand,
            } => {
                let operand_text = self.expression(operand);
                match (operator, &operand.ty) {
                    (UnaryOperator::Not, _) => format!("!({operand_text})"),
                    (UnaryOperator::Negate, Type::Float) => format!("-({operand_text})"),
                    (UnaryOperator::Negate, Type::Int) => {
                        self.failure(&format!("number::int_neg({operand_text})"), value.span)
                    }
                    (UnaryOperator::Negate, Type::Decimal) => {
                        self.failure(&format!("({operand_text}).checked_neg()"), value.span)
                    }
                    _ => unreachable!(),
                }
            }
            Expr::Binary {
                left,
                operator,
                right,
            } => self.binary(left, *operator, right, value),
        }
    }

    fn can_move(&self, value: &Expression) -> bool {
        self.moves.contains(&(value as *const Expression)) && value.ty.movable(&self.program.types)
    }

    fn fail_expression(&self, error: &Expression, result: &Expression) -> String {
        let Type::Named(ty) = error.ty else {
            unreachable!("checked failure choice")
        };
        let Expr::Variant { variant, arguments } = &error.kind else {
            unreachable!("checked direct failure variant")
        };
        let payload = if arguments.is_empty() {
            String::new()
        } else {
            format!("({})", self.arguments(arguments))
        };
        format!(
            "return Err(AppError::business(AppErrorKind::E{ty}V{variant}{payload}, {}))",
            self.location(result.span)
        )
    }

    fn capture_result(
        &self,
        target: CallTarget,
        arguments: &[CallArgument],
        choice: usize,
        success: usize,
        result: &Expression,
    ) -> String {
        let instance = self.call_instance(target, arguments);
        let mut call = self.call(&instance, arguments, DatabaseContext::Inherit);
        if specialize::suspends(self.program, &instance) {
            call.push_str(".await");
        }
        let success_fields = match &self.program.types[choice].shape {
            crate::types::Shape::Choice(variants) => variants[success].fields.len(),
            _ => unreachable!("checked result capture choice"),
        };
        let success_arm = match success_fields {
            0 => format!("Ok(()) => T{choice}::V{success}"),
            1 => format!("Ok(value) => T{choice}::V{success}(value)"),
            count => {
                let values = (0..count)
                    .map(|index| format!("value{index}"))
                    .collect::<Vec<_>>();
                format!(
                    "Ok(({})) => T{choice}::V{success}({})",
                    values.join(", "),
                    values.join(", ")
                )
            }
        };
        let failures = specialize::failures(self.program, &instance);
        let error_arms = crate::capture::targets(&self.program.types, choice, &failures)
            .expect("validated result capture errors")
            .into_iter()
            .map(|(failure, target)| {
                let crate::types::Shape::Choice(variants) = &self.program.types[failure.ty].shape
                else {
                    unreachable!("checked error choice")
                };
                let definition = &variants[failure.variant];
                let fields = (0..definition.fields.len())
                    .map(|index| format!("field{index}"))
                    .collect::<Vec<_>>();
                let payload = if fields.is_empty() {
                    String::new()
                } else {
                    format!("({})", fields.join(", "))
                };
                let error = format!("T{}::V{}{payload}", failure.ty, failure.variant);
                let value = if target.wrapped {
                    format!("T{choice}::V{}({error})", target.variant)
                } else {
                    error
                };
                format!(
                    "Err(AppError {{ kind: AppErrorKind::E{}V{}{payload}, .. }}) => {value}",
                    failure.ty, failure.variant
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "match {call} {{ {success_arm}, {error_arms}, Err(error) => return Err(error.at({})) }}",
            self.location(result.span)
        )
    }

    fn call_instance(&self, target: CallTarget, arguments: &[CallArgument]) -> Specialization {
        match target {
            CallTarget::Function(function) => {
                specialize::call_specialization(function, arguments, &self.bindings)
            }
            CallTarget::Handler(target) => Specialization {
                function: specialize::resolve_handler(target, &self.bindings),
                handlers: Vec::new(),
            },
        }
    }

    fn call(
        &self,
        function: &Specialization,
        arguments: &[CallArgument],
        database: DatabaseContext,
    ) -> String {
        self.invoke(function, self.call_arguments(arguments), database)
    }

    fn group_local(&self, group: &Expression) -> String {
        let Expr::Local(slot) = group.kind else {
            unreachable!("checked Group operand")
        };
        format!("_v{slot}")
    }

    fn thread_call(
        &self,
        operation: &str,
        target: CallTarget,
        arguments: &[CallArgument],
        result: &Expression,
    ) -> String {
        let mut value_index = 0;
        let mut bindings = Vec::new();
        let mut call_arguments = Vec::new();
        for argument in arguments {
            match argument {
                CallArgument::Value(value) => {
                    bindings.push(format!(
                        "let a{value_index}: {} = {};",
                        rust_type(&value.ty),
                        self.expression(value)
                    ));
                    call_arguments.push(format!("a{value_index}"));
                    value_index += 1;
                }
                CallArgument::Handler(_) => {}
            }
        }
        let function = self.call_instance(target, arguments);
        let call = self.invoke(
            &function,
            call_arguments.join(", "),
            DatabaseContext::Detached,
        );
        let operation = self.call_failure(
            &format!("task::{operation}_typed(move || {call}).await"),
            result.span,
        );
        format!("{{ {} {operation} }}", bindings.join(" "))
    }

    fn field_expression(&self, expression: &Expression) -> String {
        let mut base = expression;
        let mut fields = Vec::new();
        while let Expr::Field { value, index } = &base.kind {
            fields.push(field_suffix(&value.ty, *index));
            base = value;
        }
        let suffix = fields.into_iter().rev().collect::<String>();
        if let Expr::Local(slot) = base.kind {
            let projected = format!("_v{slot}{suffix}");
            if self.can_move(expression) || self.can_move(base) {
                projected
            } else {
                format!("{projected}.clone()")
            }
        } else {
            format!("({}){suffix}", self.expression(base))
        }
    }

    pub(super) fn arguments(&self, arguments: &[Expression]) -> String {
        arguments
            .iter()
            .map(|value| self.expression(value))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn call_arguments(&self, arguments: &[CallArgument]) -> String {
        arguments
            .iter()
            .filter_map(|argument| match argument {
                CallArgument::Value(value) => Some(self.expression(value)),
                CallArgument::Handler(_) => None,
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn bind_arguments(&self, arguments: &[Expression]) -> String {
        arguments
            .iter()
            .enumerate()
            .map(|(index, value)| {
                format!(
                    "let a{index}: {} = {};",
                    rust_type(&value.ty),
                    self.expression(value)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn binary(
        &self,
        left: &Expression,
        operator: Op,
        right: &Expression,
        result: &Expression,
    ) -> String {
        let left_text = self.expression(left);
        let right_text = self.expression(right);
        let symbol = match operator {
            Op::Add => "+",
            Op::Subtract => "-",
            Op::Multiply => "*",
            Op::Divide => "/",
            Op::IntegerDivide => "/",
            Op::Remainder => "%",
            Op::Equal => "==",
            Op::NotEqual => "!=",
            Op::Less => "<",
            Op::LessEqual => "<=",
            Op::Greater => ">",
            Op::GreaterEqual => ">=",
            Op::And => "&&",
            Op::Or => "||",
        };
        if matches!(operator, Op::And | Op::Or) {
            return format!("(({left_text}) {symbol} ({right_text}))");
        }
        let operation = if matches!(
            operator,
            Op::Equal | Op::NotEqual | Op::Less | Op::LessEqual | Op::Greater | Op::GreaterEqual
        ) {
            format!("a0 {symbol} a1")
        } else if left.ty == Type::Text {
            "a0 + &a1".into()
        } else if left.ty == Type::Float {
            format!("a0 {symbol} a1")
        } else {
            let method = match operator {
                Op::Add => "add",
                Op::Subtract => "sub",
                Op::Multiply => "mul",
                Op::Divide | Op::IntegerDivide => "div",
                Op::Remainder => "rem",
                _ => unreachable!(),
            };
            let operation = if left.ty == Type::Int {
                format!("number::int_{method}(a0, a1)")
            } else {
                format!("a0.checked_{method}(a1)")
            };
            self.failure(&operation, result.span)
        };
        format!("{{ let a0 = {left_text}; let a1 = {right_text}; {operation} }}")
    }
}
