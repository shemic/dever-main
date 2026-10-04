use super::{Evaluator, ResultValue, RuntimeFault, Value};
use crate::{
    hir::Expression,
    intrinsic::Intrinsic,
    types::{Shape, Type, Variant},
};
use dever_runtime::{
    Id,
    bytes::Bytes,
    collections::List,
    number::{self, DecimalValue},
    process, resource, text, time,
};

impl Evaluator<'_> {
    pub(super) fn intrinsic(
        &mut self,
        operation: Intrinsic,
        arguments: Vec<Value>,
        result: &Expression,
    ) -> ResultValue {
        use Intrinsic::*;
        let args = &arguments;
        let recover = |outcome: Result<Value, String>, success| {
            self.resource_result(outcome, &result.ty, success)
        };
        Ok(match operation {
            TestSecret => Value::Secret(dever_runtime::secret::Secret::from_input(
                args[0].text().as_bytes().to_vec(),
            )),
            CryptoToken => recover(
                dever_runtime::crypto::token(args[0].int()).map(Value::Secret),
                "Ready",
            ),
            CryptoPasswordHash => recover(
                dever_runtime::crypto::password_hash(args[0].secret()).map(Value::Text),
                "Ready",
            ),
            CryptoPasswordVerify => recover(
                dever_runtime::crypto::password_verify(args[0].secret(), args[1].text())
                    .map(Value::Bool),
                "Ready",
            ),
            CryptoSha256 => Value::Bytes(dever_runtime::crypto::sha256(args[0].bytes())),
            CryptoHmacSha256 => Value::Bytes(dever_runtime::crypto::hmac_sha256(
                args[0].secret(),
                args[1].bytes(),
            )),
            CryptoConstantTimeEq => Value::Bool(dever_runtime::crypto::constant_time_eq(
                args[0].bytes(),
                args[1].bytes(),
            )),
            TestAssert => {
                if !args[0].boolean() {
                    return Err(RuntimeFault::new("assertion failed", result.span));
                }
                Value::Unit
            }
            TestAssertEq => {
                if args[0] != args[1] {
                    return Err(RuntimeFault::new(
                        format!(
                            "assert_eq failed: actual = {}, expected = {}",
                            args[0].render(self.program),
                            args[1].render(self.program)
                        ),
                        result.span,
                    ));
                }
                Value::Unit
            }
            TextTrim => Value::Text(args[0].text().trim().to_owned()),
            TextLower => Value::Text(args[0].text().to_lowercase()),
            TextUpper => Value::Text(args[0].text().to_uppercase()),
            TextCodepoint => optional(text::codepoint(args[0].text()), Value::Int),
            TextFromCodepoint => optional(text::from_codepoint(args[0].int()), Value::Text),
            TextAt => optional(text::at(args[0].text(), args[1].int()), Value::Text),
            TextSlice => optional(
                text::slice(args[0].text(), args[1].int(), args[2].int()),
                Value::Text,
            ),
            TextIndexOf => optional(text::index_of(args[0].text(), args[1].text()), Value::Int),
            TextContains => Value::Bool(args[0].text().contains(args[1].text())),
            TextStartsWith => Value::Bool(args[0].text().starts_with(args[1].text())),
            TextEndsWith => Value::Bool(args[0].text().ends_with(args[1].text())),
            TextSplit => text_list(text::split(args[0].text(), args[1].text())),
            TextReplace => Value::Text(args[0].text().replace(args[1].text(), args[2].text())),
            IntParse => optional(number::parse_int(args[0].text()), Value::Int),
            DecimalParse => optional(number::parse_decimal(args[0].text()), Value::Decimal),
            FloatParse => optional(number::parse_float(args[0].text()), Value::Float),
            IntToText | DecimalToText | FloatToText => Value::Text(args[0].render(self.program)),
            DecimalFromInt => Value::Decimal(DecimalValue::from_int(args[0].int())),
            ProcessArguments => recover(process::arguments().map(text_list), "Read"),
            TimeUnixMillis => recover(time::unix_millis().map(Value::Int), "Read"),
            TimeNow => recover(time::now().map(Value::Int), "Read"),
            TimeParseDateTime => {
                recover(time::parse_datetime(args[0].text()).map(Value::Int), "Read")
            }
            TimeFormatDateTime => recover(
                time::format_datetime(args[0].int()).map(Value::Text),
                "Read",
            ),
            TimeParseDate => recover(time::parse_date(args[0].text()).map(Value::Int), "Read"),
            TimeFormatDate => recover(time::format_date(args[0].int()).map(Value::Text), "Read"),
            TimeParseTime => recover(time::parse_time(args[0].text()).map(Value::Int), "Read"),
            TimeFormatTime => recover(time::format_time(args[0].int()).map(Value::Text), "Read"),
            TimeDuration => Value::Int(time::duration(args[0].int())),
            TimeAdd => recover(
                time::add(args[0].int(), args[1].int()).map(Value::Int),
                "Read",
            ),
            TimeSubtract => recover(
                time::subtract(args[0].int(), args[1].int()).map(Value::Int),
                "Read",
            ),
            TimeDifference => recover(
                time::difference(args[0].int(), args[1].int()).map(Value::Int),
                "Read",
            ),
            TimeMonotonicNanos => recover(time::monotonic_nanos().map(Value::Int), "Read"),
            TimeSleep => recover(time::sleep(args[0].int()).map(|()| Value::Unit), "Done"),
            TaskTicks
            | TaskTimeout
            | TaskRace
            | StreamOf
            | TlsSystem
            | TlsClient
            | TlsServer
            | HttpClientNew
            | HttpRequest
            | HttpOpen
            | HttpOpenStream
            | HttpCloseClient
            | HttpServeTls
            | HttpServeLiveTls
            | WsOpen
            | AuthIssue
            | AuthIssueCookie
            | AuthClearCookie
            | AuthOwnsUser
            | AuthId
            | AuthSession
            | AuthUserId
            | AuthTenantId
            | SiteKey
            | AuthPermissions
            | AuthSaveRole
            | AuthGrantRole
            | AuthRevokeRole
            | AuthDisableRole
            | ApiRequestId
            | ApiMethod
            | ApiPath
            | ApiClientAddress
            | ApiHeader
            | ApiCookie
            | ApiSecretCookie
            | ApiResponseHeader
            | ApiResponseCookie
            | ApiResponseSecretCookie
            | UploadFilename
            | UploadContentType
            | UploadSize
            | UploadClose
            | UploadStore
            | HttpServe
            | ApiServe
            | JobServe
            | TestJobDrain
            | TestClockAdvance
            | HttpSend
            | HttpServeLive
            | HttpRespond
            | HttpStart
            | HttpWrite
            | HttpFinish
            | SseStart
            | SseSend
            | WsAccept
            | WsConnect
            | WsSend
            | WsReceive
            | WsMessages
            | WsClose
            | TaskSleep
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
            | TcpConnections => {
                return Err(RuntimeFault::new(
                    "the reference evaluator does not execute async operations",
                    result.span,
                ));
            }
            DecimalRound => Value::Decimal(
                args[0]
                    .decimal()
                    .round(args[1].int())
                    .map_err(|message| RuntimeFault::new(message, result.span))?,
            ),
            FloatFromInt => Value::Float(args[0].int() as f64),
            FloatSqrt => Value::Float(args[0].float().sqrt()),
            FloatSin => Value::Float(args[0].float().sin()),
            FloatCos => Value::Float(args[0].float().cos()),
            FloatLog => Value::Float(args[0].float().ln()),
            FloatPow => Value::Float(args[0].float().powf(args[1].float())),
            FloatIsNan => Value::Bool(args[0].float().is_nan()),
            FloatIsFinite => Value::Bool(args[0].float().is_finite()),
            FloatIsInfinite => Value::Bool(args[0].float().is_infinite()),
            StdoutWrite => {
                self.stdout.push_str(args[0].text());
                Value::Unit
            }
            LogDebug | LogInfo | LogWarn | LogError => Value::Unit,
            IdFromText => Value::Id(Id(args[0].text().to_owned())),
            IdToText => {
                let Value::Id(id) = &args[0] else {
                    unreachable!()
                };
                Value::Text(id.0.clone())
            }
            UuidParse => optional(
                dever_runtime::orm::Uuid::parse(args[0].text()).ok(),
                Value::Uuid,
            ),
            UuidToText => Value::Text(args[0].uuid().to_string()),
            BytesFromText => Value::Bytes(Bytes::from_text(args[0].text())),
            BytesLength => Value::Int(args[0].bytes().length()),
            BytesAt => optional(args[0].bytes().at(args[1].int()), Value::Int),
            BytesConcat => Value::Bytes(args[0].bytes().clone().concat(args[1].bytes())),
            BytesToText => recover(args[0].bytes().to_text().map(Value::Text), "Decoded"),
            BytesFromInts => {
                let Value::List(values) = &args[0] else {
                    unreachable!()
                };
                recover(
                    Bytes::from_ints(&values.values().iter().map(Value::int).collect::<Vec<_>>())
                        .map(Value::Bytes)
                        .map_err(str::to_owned),
                    "Ready",
                )
            }
            BytesSlice => recover(
                args[0]
                    .bytes()
                    .slice(args[1].int(), args[2].int())
                    .map(Value::Bytes)
                    .map_err(str::to_owned),
                "Ready",
            ),
            FileOpen => recover(
                resource::file_open(args[0].text()).map(Value::File),
                "Opened",
            ),
            FileCreate => recover(
                resource::file_create(args[0].text()).map(Value::File),
                "Opened",
            ),
            FileRead => match resource::read(args[0].file(), args[1].int()) {
                Ok(Some(bytes)) => self.choice(&result.ty, "Read", vec![Value::Bytes(bytes)]),
                Ok(None) => self.choice(&result.ty, "End", vec![]),
                Err(message) => recover(Err(message), "Read"),
            },
            FileWrite => recover(
                resource::write(args[0].file(), args[1].bytes()).map(|()| Value::Unit),
                "Done",
            ),
            FileClose => recover(args[0].file().close().map(|()| Value::Unit), "Closed"),
            FileChunks => self.read_stream(args, &result.ty),
        })
    }

    fn read_stream(&self, arguments: &[Value], output: &Type) -> Value {
        let outcome = resource::read_chunks(arguments[0].file().clone(), arguments[1].int());
        let (_, variants) = self.variants(output);
        let Type::Stream(element) = &variants[variant(variants, "Streaming")].fields[0].ty else {
            unreachable!()
        };
        let (id, events) = self.variants(element);
        let chunk = variant(events, "Chunk");
        let failed = variant(events, "Failed");
        self.resource_result(
            outcome.map(|stream| {
                Value::Stream(stream.map(move |event| match event {
                    Ok(bytes) => Value::Variant(id, chunk, vec![Value::Bytes(bytes)]),
                    Err(message) => Value::Variant(id, failed, vec![Value::Text(message)]),
                }))
            }),
            output,
            "Streaming",
        )
    }

    fn resource_result(
        &self,
        outcome: Result<Value, String>,
        output: &Type,
        success: &str,
    ) -> Value {
        match outcome {
            Ok(Value::Unit) => self.choice(output, success, vec![]),
            Ok(value) => self.choice(output, success, vec![value]),
            Err(message) => self.choice(output, "Failed", vec![Value::Text(message)]),
        }
    }

    fn choice(&self, output: &Type, name: &str, payload: Vec<Value>) -> Value {
        let (id, variants) = self.variants(output);
        Value::Variant(id, variant(variants, name), payload)
    }

    fn variants(&self, output: &Type) -> (usize, &[Variant]) {
        let Type::Named(id) = output else {
            unreachable!("checked result type")
        };
        let Shape::Choice(variants) = &self.program.types[*id].shape else {
            unreachable!()
        };
        (*id, variants)
    }
}

fn variant(variants: &[Variant], name: &str) -> usize {
    variants
        .iter()
        .position(|variant| variant.name == name)
        .expect("checked result variant")
}

fn optional<T>(value: Option<T>, wrap: impl FnOnce(T) -> Value) -> Value {
    value.map(wrap).unwrap_or(Value::Null)
}
fn text_list(values: List<String>) -> Value {
    Value::List(List::new(
        values.values().iter().cloned().map(Value::Text).collect(),
    ))
}
