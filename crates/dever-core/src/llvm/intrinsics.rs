use super::*;

pub(super) fn supported(operation: Intrinsic) -> bool {
    use Intrinsic::*;
    super::system::supported(operation)
        || super::testing::supported(operation)
        || super::http::supported(operation)
        || super::api_context::supported(operation)
        || matches!(
            operation,
            TextTrim
                | TextLower
                | TextUpper
                | TextCodepoint
                | TextFromCodepoint
                | TextAt
                | TextSlice
                | TextIndexOf
                | TextContains
                | TextStartsWith
                | TextEndsWith
                | TextReplace
                | TextSplit
                | IntParse
                | IntToText
                | DecimalParse
                | DecimalToText
                | DecimalFromInt
                | DecimalRound
                | FloatParse
                | FloatToText
                | FloatFromInt
                | FloatSqrt
                | FloatSin
                | FloatCos
                | FloatLog
                | FloatPow
                | FloatIsNan
                | FloatIsFinite
                | FloatIsInfinite
                | IdFromText
                | IdToText
                | BytesFromText
                | BytesLength
                | BytesAt
                | BytesConcat
                | BytesToText
                | BytesFromInts
                | BytesSlice
                | FileOpen
                | FileCreate
                | FileRead
                | FileWrite
                | FileClose
                | FileChunks
                | TimeSleep
                | TaskSleep
                | TaskTicks
                | TaskTimeout
                | TaskRace
                | StreamOf
                | TcpConnect
                | TcpConnectTimeout
                | TcpListen
                | TcpAccept
                | TcpPort
                | TcpRead
                | TcpWrite
                | TcpTimeout
                | TcpClose
                | TcpCloseListener
                | TcpChunks
                | TcpConnections
        )
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn decimal_parts(&mut self, value: &str) -> Vec<String> {
        (0..2)
            .map(|index| {
                let part = self.temp();
                self.line(format!(
                    "{part} = extractvalue {{ i64, i64 }} {value}, {index}"
                ));
                format!("i64 {part}")
            })
            .collect()
    }

    pub(super) fn intrinsic(
        &mut self,
        operation: Intrinsic,
        handler: Option<crate::hir::HandlerTarget>,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        use Intrinsic::*;
        if matches!(operation, TaskTimeout | TaskRace) {
            return self.task_selection(operation, handler, arguments, result);
        }
        let values = arguments
            .iter()
            .map(|argument| {
                self.expression(argument)
                    .expect("validated intrinsic argument")
            })
            .collect::<Vec<_>>();
        let args = arguments
            .iter()
            .zip(&values)
            .map(|(argument, value)| format!("{} {value}", self.module.ty(&argument.ty)))
            .collect::<Vec<_>>();
        let nullable = matches!(result.ty, Type::Nullable(_));
        let span = result.span;
        if super::testing::supported(operation) {
            return self.test_intrinsic(operation, arguments, &values, span);
        }
        if super::api_context::supported(operation) {
            return self.api_intrinsic(operation, arguments, &values, result);
        }
        if super::http::supported(operation) {
            return self.http_intrinsic(operation, handler, arguments, &values, result);
        }
        if super::system::supported(operation) {
            return self.system_intrinsic(operation, args, &values, result);
        }
        if matches!(
            operation,
            StreamOf
                | TaskTicks
                | TcpConnect
                | TcpConnectTimeout
                | TcpListen
                | TcpAccept
                | TcpPort
                | TcpRead
                | TcpWrite
                | TcpTimeout
                | TcpClose
                | TcpCloseListener
                | TcpChunks
                | TcpConnections
        ) {
            return self.network_intrinsic(operation, arguments, args, result);
        }
        if matches!(operation, BytesToText | BytesFromInts | BytesSlice) {
            return self.bytes_result(operation, args, result);
        }
        if matches!(
            operation,
            FileOpen | FileCreate | FileRead | FileWrite | FileClose | FileChunks
        ) {
            return self.file_result(operation, args, result);
        }
        if operation == TaskSleep {
            return self.task_sleep(&values[0], result);
        }
        if operation == TimeSleep {
            return self.recoverable_call("dever_rt_v1_time_sleep", args, result, "Done", None);
        }
        let (symbol, args, physical) = match operation {
            TextTrim => ("text_trim", args, None),
            TextLower => ("text_lower", args, None),
            TextUpper => ("text_upper", args, None),
            TextCodepoint => ("text_codepoint", args, None),
            TextFromCodepoint => ("text_from_codepoint", args, None),
            TextAt => ("text_at", args, None),
            TextSlice => ("text_slice", args, None),
            TextIndexOf => ("text_index_of", args, None),
            TextContains => ("text_contains", args, Some("i8")),
            TextStartsWith => ("text_starts_with", args, Some("i8")),
            TextEndsWith => ("text_ends_with", args, Some("i8")),
            TextReplace => ("text_replace", args, None),
            TextSplit => {
                let mut args = args;
                args.push(format!(
                    "ptr @dever_type_{}",
                    self.module.type_index(&Type::Text)
                ));
                ("text_split", args, None)
            }
            IntParse => ("int_parse", args, None),
            DecimalParse => ("decimal_parse", args, None),
            FloatParse => ("float_parse", args, None),
            DecimalFromInt => ("decimal_from_int", args, None),
            DecimalRound => {
                let mut parts = self.decimal_parts(&values[0]);
                parts.push(format!("i64 {}", values[1]));
                ("decimal_round", parts, None)
            }
            BytesFromText => ("bytes_from_text", args, None),
            BytesLength => ("bytes_length", args, None),
            BytesAt => ("bytes_at", args, None),
            BytesConcat => ("bytes_concat", args, None),
            IntToText => return self.render_text("dever_rt_v1_int_to_text", args, span),
            FloatToText => return self.render_text("dever_rt_v1_float_to_text", args, span),
            DecimalToText => {
                let parts = self.decimal_parts(&values[0]);
                return self.render_text("dever_rt_v1_decimal_to_text", parts, span);
            }
            FloatFromInt => {
                let converted = self.temp();
                self.line(format!("{converted} = sitofp i64 {} to double", values[0]));
                return converted;
            }
            FloatSqrt | FloatSin | FloatCos | FloatLog | FloatPow => {
                let name = match operation {
                    FloatSqrt => "sqrt",
                    FloatSin => "sin",
                    FloatCos => "cos",
                    FloatLog => "log",
                    FloatPow => "pow",
                    _ => unreachable!(),
                };
                let converted = self.temp();
                self.line(format!(
                    "{converted} = call double @llvm.{name}.f64({})",
                    args.join(", ")
                ));
                return converted;
            }
            FloatIsNan => {
                let converted = self.temp();
                self.line(format!(
                    "{converted} = fcmp uno double {}, {}",
                    values[0], values[0]
                ));
                return converted;
            }
            FloatIsFinite | FloatIsInfinite => {
                let bits = self.temp();
                self.line(format!("{bits} = bitcast double {} to i64", values[0]));
                let exponent = self.temp();
                self.line(format!("{exponent} = and i64 {bits}, 9218868437227405312"));
                let all_ones = self.temp();
                self.line(format!(
                    "{all_ones} = icmp eq i64 {exponent}, 9218868437227405312"
                ));
                if operation == FloatIsFinite {
                    let finite = self.temp();
                    self.line(format!("{finite} = xor i1 {all_ones}, 1"));
                    return finite;
                }
                let fraction = self.temp();
                self.line(format!("{fraction} = and i64 {bits}, 4503599627370495"));
                let no_fraction = self.temp();
                self.line(format!("{no_fraction} = icmp eq i64 {fraction}, 0"));
                let infinite = self.temp();
                self.line(format!("{infinite} = and i1 {all_ones}, {no_fraction}"));
                return infinite;
            }
            IdFromText | IdToText => {
                let retained = self.temp();
                self.line(format!(
                    "{retained} = call ptr @dever_rt_v1_text_retain(ptr {})",
                    values[0]
                ));
                return retained;
            }
            _ => unreachable!("preflighted intrinsic"),
        };
        self.runtime_call(
            &format!("dever_rt_v1_{symbol}"),
            args,
            &result.ty,
            nullable,
            physical,
            span,
        )
    }

    fn bytes_result(
        &mut self,
        operation: Intrinsic,
        args: Vec<String>,
        result: &Expression,
    ) -> String {
        let success_name = if operation == Intrinsic::BytesToText {
            "Decoded"
        } else {
            "Ready"
        };
        let symbol = match operation {
            Intrinsic::BytesToText => "dever_rt_v1_bytes_to_text_handle",
            Intrinsic::BytesFromInts => "dever_rt_v1_bytes_from_ints",
            Intrinsic::BytesSlice => "dever_rt_v1_bytes_slice",
            _ => unreachable!(),
        };
        self.recoverable_call(symbol, args, result, success_name, None)
    }
}
