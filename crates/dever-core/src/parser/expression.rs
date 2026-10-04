use super::{ParseResult, Parser};
use crate::diagnostic::Diagnostic;
use crate::source::Span;
use crate::syntax::*;
use crate::token::Kind;

impl Parser {
    pub(super) fn expression(&mut self, minimum: u8, multiline: bool) -> ParseResult<Expression> {
        self.nested(|parser| parser.binary_expression(minimum, multiline))
    }

    fn binary_expression(&mut self, minimum: u8, multiline: bool) -> ParseResult<Expression> {
        if multiline {
            self.skip_lines();
        }
        let mut left = self.prefix(multiline)?;
        loop {
            if multiline {
                self.skip_lines();
            }
            if self.at(&Kind::LeftParen) {
                self.advance();
                let mut arguments =
                    self.separated(Kind::RightParen, |parser| parser.expression(0, true))?;
                let span = left.span.through(self.previous_span());
                let core_name = match &left.kind {
                    ExpressionKind::Name(path)
                        if path.len() == 1
                            && matches!(path[0].text.as_str(), "fail" | "result") =>
                    {
                        Some(path[0].text.as_str())
                    }
                    _ => None,
                };
                let kind = if let Some(core_name) = core_name {
                    if arguments.len() != 1 {
                        return Err(Diagnostic::error(
                            "P001",
                            format!("'{core_name}' expects exactly one argument"),
                            span,
                        ));
                    }
                    let argument = Box::new(arguments.remove(0));
                    if core_name == "fail" {
                        ExpressionKind::Fail(argument)
                    } else {
                        ExpressionKind::CaptureResult(argument)
                    }
                } else {
                    ExpressionKind::Call {
                        function: Box::new(left),
                        arguments,
                    }
                };
                left = self.expression_node(kind, span)?;
                continue;
            }
            if self.take(&Kind::Dot).is_some() {
                let name = self.name()?;
                let span = left.span.through(name.span);
                left = self.expression_node(
                    ExpressionKind::Field {
                        value: Box::new(left),
                        name,
                    },
                    span,
                )?;
                continue;
            }
            let Some(operator) = self.binary_operator() else {
                break;
            };
            let precedence = operator.precedence();
            if precedence < minimum {
                break;
            }
            self.advance();
            let right = self.expression(precedence + 1, multiline)?;
            let span = left.span.through(right.span);
            left = self.expression_node(
                ExpressionKind::Binary {
                    left: Box::new(left),
                    operator,
                    right: Box::new(right),
                },
                span,
            )?;
        }
        Ok(left)
    }

    fn binary_operator(&self) -> Option<BinaryOperator> {
        let operator = match self.current().kind {
            Kind::Plus => BinaryOperator::Add,
            Kind::Minus => BinaryOperator::Subtract,
            Kind::Star => BinaryOperator::Multiply,
            Kind::Slash => BinaryOperator::Divide,
            Kind::IntegerDivide => BinaryOperator::IntegerDivide,
            Kind::Percent => BinaryOperator::Remainder,
            Kind::Equal => BinaryOperator::Equal,
            Kind::NotEqual => BinaryOperator::NotEqual,
            Kind::Less => BinaryOperator::Less,
            Kind::LessEqual => BinaryOperator::LessEqual,
            Kind::Greater => BinaryOperator::Greater,
            Kind::GreaterEqual => BinaryOperator::GreaterEqual,
            Kind::And => BinaryOperator::And,
            Kind::Or => BinaryOperator::Or,
            _ => return None,
        };
        Some(operator)
    }

    fn prefix(&mut self, multiline: bool) -> ParseResult<Expression> {
        let start = self.current().span;
        if let Some(value) = self.literal() {
            return self.expression_node(ExpressionKind::Literal(value), start);
        }
        let kind = match &self.current().kind {
            Kind::Name(_) => {
                let name = self.path()?;
                if name.len() == 1
                    && name[0].text == "channel"
                    && self.take(&Kind::LeftParen).is_some()
                {
                    let element = self.type_ref()?;
                    self.expect(&Kind::Comma, "',' after the channel item type")?;
                    let capacity = self.expression(0, true)?;
                    self.expect(&Kind::RightParen, "')' after the channel capacity")?;
                    return self.expression_node(
                        ExpressionKind::Contextual(ContextualExpression::Channel {
                            element,
                            capacity: Box::new(capacity),
                        }),
                        start.through(self.previous_span()),
                    );
                }
                if self.take(&Kind::LeftBrace).is_some() {
                    self.record(name)?
                } else {
                    ExpressionKind::Name(name)
                }
            }
            Kind::Minus | Kind::Not => {
                let operator = if self.at(&Kind::Minus) {
                    UnaryOperator::Negate
                } else {
                    UnaryOperator::Not
                };
                self.advance();
                let value = self.expression(6, multiline)?;
                ExpressionKind::Unary {
                    operator,
                    value: Box::new(value),
                }
            }
            Kind::LeftParen => {
                self.advance();
                let value = self.expression(0, true)?;
                self.expect(&Kind::RightParen, "')'")?;
                ExpressionKind::Group(Box::new(value))
            }
            Kind::LeftBracket => {
                self.advance();
                ExpressionKind::List(
                    self.separated(Kind::RightBracket, |parser| parser.expression(0, true))?,
                )
            }
            Kind::LeftBrace => {
                self.advance();
                self.map()?
            }
            _ => return Err(self.error("expected a value, name, construction or function call")),
        };
        self.expression_node(kind, start.through(self.previous_span()))
    }

    fn expression_node(&self, kind: ExpressionKind, span: Span) -> ParseResult<Expression> {
        // Iterative operator/postfix parsing also creates recursive trees consumed by Drop and later phases.
        let child_depth = match &kind {
            ExpressionKind::Literal(_) | ExpressionKind::Name(_) => 0,
            ExpressionKind::Group(value)
            | ExpressionKind::Field { value, .. }
            | ExpressionKind::Fail(value)
            | ExpressionKind::CaptureResult(value)
            | ExpressionKind::Unary { value, .. } => value.depth,
            ExpressionKind::Binary { left, right, .. } => left.depth.max(right.depth),
            ExpressionKind::Call {
                function,
                arguments,
            } => arguments
                .iter()
                .map(|argument| argument.depth)
                .max()
                .unwrap_or(0)
                .max(function.depth),
            ExpressionKind::Contextual(ContextualExpression::Channel { capacity, .. }) => {
                capacity.depth
            }
            ExpressionKind::Record { fields, .. } => fields
                .iter()
                .map(|field| field.value.depth)
                .max()
                .unwrap_or(0),
            ExpressionKind::List(values) => {
                values.iter().map(|value| value.depth).max().unwrap_or(0)
            }
            ExpressionKind::Map(entries) => entries
                .iter()
                .map(|entry| entry.key.depth.max(entry.value.depth))
                .max()
                .unwrap_or(0),
        };
        let depth = child_depth + 1;
        if depth > MAX_SYNTAX_DEPTH {
            return Err(Diagnostic::error(
                "P009",
                "syntax nesting exceeds the compiler limit",
                span,
            ));
        }
        Ok(Expression { kind, span, depth })
    }

    fn record(&mut self, name: Path) -> ParseResult<ExpressionKind> {
        let mut fields = Vec::new();
        self.skip_lines();
        while !self.at(&Kind::RightBrace) {
            let name = self.name()?;
            self.expect(&Kind::Assign, "'=' after a record field name")?;
            let value = self.expression(0, false)?;
            let span = name.span.through(value.span);
            fields.push(RecordField { name, value, span });
            self.line_end(&Kind::RightBrace)?;
        }
        self.advance();
        Ok(ExpressionKind::Record { name, fields })
    }

    fn map(&mut self) -> ParseResult<ExpressionKind> {
        let mut entries = Vec::new();
        self.skip_lines();
        while !self.at(&Kind::RightBrace) {
            let key = self.expression(0, false)?;
            self.expect(&Kind::Assign, "'=' after a Map key")?;
            let value = self.expression(0, false)?;
            let span = key.span.through(value.span);
            entries.push(MapEntry { key, value, span });
            self.line_end(&Kind::RightBrace)?;
        }
        self.advance();
        Ok(ExpressionKind::Map(entries))
    }
}
