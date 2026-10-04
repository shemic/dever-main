use crate::diagnostic::Diagnostic;
use crate::intrinsic::Intrinsic;
use crate::source::Span;
use crate::types::{Field, HandlerSignature, Shape, Type};

use super::{Checked, Context};

pub(super) struct Signature {
    pub handler: Option<HandlerSignature>,
    pub parameters: Vec<Type>,
    pub output: Type,
}

pub(super) fn signature(
    operation: Intrinsic,
    context: &Context<'_>,
    span: Span,
) -> Checked<Signature> {
    use Intrinsic::*;
    use Type::*;
    let mut handler = None;
    let result = |name: &str, success: &str, payload: &[Type], eof: bool| -> Checked<Type> {
        let id = context.named_type(name, span)?;
        let Shape::Choice(variants) = &context.types[id].shape else {
            return Err(Diagnostic::error(
                "C005",
                "system result must be a choice type",
                span,
            ));
        };
        let mut expected = vec![(success, payload.to_vec()), ("Failed", vec![Text])];
        if eof {
            expected.push(("End", vec![]));
        }
        if variants.len() != expected.len()
            || !expected.iter().all(|(name, payload)| {
                variants.iter().any(|variant| {
                    variant.name == *name
                        && variant
                            .fields
                            .iter()
                            .map(|field| &field.ty)
                            .eq(payload.iter())
                })
            })
        {
            return Err(Diagnostic::error(
                "C005",
                "official result type does not match its system primitive contract",
                span,
            ));
        }
        Ok(Named(id))
    };
    let (parameters, output) = match operation {
        TestAssert | TestAssertEq | TestSecret | AuthOwnsUser => {
            unreachable!("polymorphic primitives are checked contextually")
        }
        CryptoToken => (
            vec![Int],
            result("dever.crypto.TokenResult", "Ready", &[Secret], false)?,
        ),
        CryptoPasswordHash => (
            vec![Secret],
            result("dever.crypto.HashResult", "Ready", &[Text], false)?,
        ),
        CryptoPasswordVerify => (
            vec![Secret, Text],
            result("dever.crypto.VerifyResult", "Ready", &[Bool], false)?,
        ),
        CryptoSha256 => (vec![Bytes], Bytes),
        CryptoHmacSha256 => (vec![Secret, Bytes], Bytes),
        CryptoConstantTimeEq => (vec![Bytes, Bytes], Bool),
        ApiServe => (vec![], Unit),
        UploadFilename | UploadContentType => (vec![Upload], Text),
        UploadSize => (vec![Upload], Int),
        UploadClose => (vec![Upload], Unit),
        UploadStore => (
            vec![Upload],
            result("dever.storage.PutResult", "Stored", &[Uuid], false)?,
        ),
        ApiRequestId | ApiMethod | ApiPath => (vec![], Text),
        ApiClientAddress => (vec![], Text.nullable()),
        ApiHeader | ApiCookie => (vec![Text], Text.nullable()),
        ApiSecretCookie => (vec![Text], Secret.nullable()),
        ApiResponseHeader => (vec![Text, Text], Unit),
        ApiResponseCookie | ApiResponseSecretCookie => {
            let options = context.named_type("dever.api.CookieOptions", span)?;
            let value = if matches!(operation, ApiResponseCookie) {
                Text
            } else {
                Secret
            };
            (vec![Text, value, Named(options)], Unit)
        }
        AuthIssue => (vec![Text, Text, Text.nullable()], Secret),
        AuthIssueCookie => (vec![Text, Text, Text.nullable()], Unit),
        AuthClearCookie => (vec![], Unit),
        AuthId | AuthSession | SiteKey => (vec![], Text),
        AuthUserId | AuthTenantId => (vec![], Int.nullable()),
        AuthPermissions => {
            let permission = context.named_type("dever.auth.Permission", span)?;
            (vec![], List(Box::new(Named(permission))))
        }
        AuthSaveRole => (vec![Text, Text, Bool, List(Box::new(Text))], Unit),
        AuthGrantRole | AuthRevokeRole => (vec![Int, Text], Unit),
        AuthDisableRole => (vec![Text], Unit),
        JobServe => {
            if !context.symbols.packages[context.owner].bundled
                && !matches!(
                    context.symbols.packages[context.owner].layout,
                    crate::source::SourceLayout::Main
                )
            {
                return Err(Diagnostic::error(
                    "C006",
                    "only main may start the Job worker",
                    span,
                ));
            }
            (vec![], Unit)
        }
        TestJobDrain | TestClockAdvance => {
            if !context.is_test() {
                return Err(Diagnostic::error(
                    "C006",
                    "Job test controls are available only in test sources",
                    span,
                ));
            }
            (
                vec![Int],
                if matches!(operation, TestJobDrain) {
                    Int
                } else {
                    Unit
                },
            )
        }
        TaskTimeout | TaskRace | StreamOf => unreachable!("polymorphic core operation"),
        TaskTicks => (vec![Int], AsyncStream(Box::new(Int))),
        TextTrim | TextLower | TextUpper => (vec![Text], Text),
        TextCodepoint => (vec![Text], Int.nullable()),
        TextFromCodepoint => (vec![Int], Text.nullable()),
        TextAt => (vec![Text, Int], Text.nullable()),
        TextSlice => (vec![Text, Int, Int], Text.nullable()),
        TextIndexOf => (vec![Text, Text], Int.nullable()),
        TextContains | TextStartsWith | TextEndsWith => (vec![Text, Text], Bool),
        TextSplit => (vec![Text, Text], List(Box::new(Text))),
        TextReplace => (vec![Text, Text, Text], Text),
        IntParse => (vec![Text], Int.nullable()),
        IntToText => (vec![Int], Text),
        DecimalParse => (vec![Text], Decimal.nullable()),
        DecimalToText => (vec![Decimal], Text),
        DecimalFromInt => (vec![Int], Decimal),
        FloatParse => (vec![Text], Float.nullable()),
        FloatToText => (vec![Float], Text),
        ProcessArguments => (
            vec![],
            result(
                "dever.process.ArgumentsResult",
                "Read",
                &[List(Box::new(Text))],
                false,
            )?,
        ),
        TimeUnixMillis | TimeMonotonicNanos => (
            vec![],
            result("dever.time.ClockResult", "Read", &[Int], false)?,
        ),
        TimeNow => (
            vec![],
            result("dever.time.DateTimeResult", "Read", &[DateTime], false)?,
        ),
        TimeParseDateTime => (
            vec![Text],
            result("dever.time.DateTimeResult", "Read", &[DateTime], false)?,
        ),
        TimeParseDate => (
            vec![Text],
            result("dever.time.DateResult", "Read", &[Date], false)?,
        ),
        TimeParseTime => (
            vec![Text],
            result("dever.time.TimeResult", "Read", &[Time], false)?,
        ),
        TimeFormatDateTime => (
            vec![DateTime],
            result("dever.time.TextResult", "Read", &[Text], false)?,
        ),
        TimeFormatDate => (
            vec![Date],
            result("dever.time.TextResult", "Read", &[Text], false)?,
        ),
        TimeFormatTime => (
            vec![Time],
            result("dever.time.TextResult", "Read", &[Text], false)?,
        ),
        TimeDuration => (vec![Int], Duration),
        TimeAdd | TimeSubtract => (
            vec![DateTime, Duration],
            result("dever.time.DateTimeResult", "Read", &[DateTime], false)?,
        ),
        TimeDifference => (
            vec![DateTime, DateTime],
            result("dever.time.DurationResult", "Read", &[Duration], false)?,
        ),
        TimeSleep | TaskSleep => (
            vec![Int],
            result("dever.time.SleepResult", "Done", &[], false)?,
        ),
        DecimalRound => (vec![Decimal, Int], Decimal),
        FloatFromInt => (vec![Int], Float),
        FloatSqrt | FloatSin | FloatCos | FloatLog => (vec![Float], Float),
        FloatPow => (vec![Float, Float], Float),
        FloatIsNan | FloatIsFinite | FloatIsInfinite => (vec![Float], Bool),
        StdoutWrite => (vec![Text], Unit),
        LogDebug | LogInfo | LogWarn | LogError => {
            (vec![Text, Map(Box::new(Text), Box::new(Text))], Unit)
        }
        IdFromText => (vec![Text], Id),
        IdToText => (vec![Id], Text),
        UuidParse => (vec![Text], Uuid.nullable()),
        UuidToText => (vec![Uuid], Text),
        BytesFromText => (vec![Text], Bytes),
        BytesToText => (
            vec![Bytes],
            result("dever.bytes.DecodeResult", "Decoded", &[Text], false)?,
        ),
        BytesFromInts => (
            vec![List(Box::new(Int))],
            result("dever.bytes.BufferResult", "Ready", &[Bytes], false)?,
        ),
        BytesLength => (vec![Bytes], Int),
        BytesAt => (vec![Bytes, Int], Int.nullable()),
        BytesSlice => (
            vec![Bytes, Int, Int],
            result("dever.bytes.BufferResult", "Ready", &[Bytes], false)?,
        ),
        BytesConcat => (vec![Bytes, Bytes], Bytes),
        FileOpen | FileCreate => (
            vec![Text],
            result("dever.io.OpenResult", "Opened", &[File], false)?,
        ),
        FileRead => (
            vec![File, Int],
            result("dever.io.SystemReadResult", "Read", &[Bytes], true)?,
        ),
        FileWrite => (
            vec![File, Bytes],
            result("dever.io.WriteResult", "Done", &[], false)?,
        ),
        FileClose => (
            vec![File],
            result("dever.io.CloseResult", "Closed", &[], false)?,
        ),
        FileChunks | TcpChunks => {
            let event = result("dever.io.ReadEvent", "Chunk", &[Bytes], false)?;
            let (source, stream, result_name) = if matches!(operation, FileChunks) {
                (File, Stream(Box::new(event)), "dever.io.ReadStreamResult")
            } else {
                (
                    Socket,
                    AsyncStream(Box::new(event)),
                    "dever.net.ReadStreamResult",
                )
            };
            (
                vec![source, Int],
                result(result_name, "Streaming", &[stream], false)?,
            )
        }
        TcpConnect => (
            vec![Text, Int],
            result("dever.net.ConnectResult", "Connected", &[Socket], false)?,
        ),
        TcpConnectTimeout => (
            vec![Text, Int, Int],
            result("dever.net.ConnectResult", "Connected", &[Socket], false)?,
        ),
        TcpListen => (
            vec![Text, Int],
            result("dever.net.ListenResult", "Listening", &[Listener], false)?,
        ),
        TcpAccept => (
            vec![Listener],
            result("dever.net.ConnectResult", "Connected", &[Socket], false)?,
        ),
        TcpPort => (
            vec![Listener],
            result("dever.net.PortResult", "Bound", &[Int], false)?,
        ),
        TcpRead => (
            vec![Socket, Int],
            result("dever.net.SystemReadResult", "Read", &[Bytes], true)?,
        ),
        TcpWrite => (
            vec![Socket, Bytes],
            result("dever.io.WriteResult", "Done", &[], false)?,
        ),
        TcpTimeout => (
            vec![Socket, Int],
            result("dever.io.WriteResult", "Done", &[], false)?,
        ),
        TcpClose => (
            vec![Socket],
            result("dever.io.CloseResult", "Closed", &[], false)?,
        ),
        TcpCloseListener => (
            vec![Listener],
            result("dever.io.CloseResult", "Closed", &[], false)?,
        ),
        TcpConnections => {
            let connection = result("dever.net.ConnectResult", "Connected", &[Socket], false)?;
            (vec![Listener], AsyncStream(Box::new(connection)))
        }
        TlsSystem => (vec![], ClientTls),
        TlsClient => (
            vec![Bytes],
            result("dever.tls.ClientResult", "Ready", &[ClientTls], false)?,
        ),
        TlsServer => (
            vec![Bytes, Bytes],
            result("dever.tls.ServerResult", "Ready", &[ServerTls], false)?,
        ),
        HttpCloseClient => (
            vec![HttpClient],
            result("dever.io.CloseResult", "Closed", &[], false)?,
        ),
        HttpServe | HttpServeTls | HttpSend | HttpServeLive | HttpServeLiveTls | HttpRespond
        | HttpStart | SseStart | HttpClientNew | HttpRequest | HttpOpen | HttpOpenStream => {
            let header = record(
                context,
                "dever.http.Header",
                &[("name", Text), ("value", Bytes)],
                span,
            )?;
            let headers = List(Box::new(header));
            let request = record(
                context,
                "dever.http.Request",
                &[
                    ("method", Text),
                    ("target", Text),
                    ("headers", headers.clone()),
                    ("body", Bytes),
                ],
                span,
            )?;
            let response = record(
                context,
                "dever.http.Response",
                &[
                    ("status", Int),
                    ("headers", headers.clone()),
                    ("body", Bytes),
                ],
                span,
            )?;
            let http2 = record(
                context,
                "dever.http.Http2Limits",
                &[
                    ("streams", Int),
                    ("stream_window_bytes", Int),
                    ("connection_window_bytes", Int),
                ],
                span,
            )?;
            let limits = record(
                context,
                "dever.http.Limits",
                &[
                    ("header_bytes", Int),
                    ("body_bytes", Int),
                    ("timeout_ms", Int),
                    ("connections", Int),
                    ("http2", http2.nullable()),
                ],
                span,
            )?;
            match operation {
                HttpServeLive | HttpServeLiveTls => {
                    let live = record(
                        context,
                        "dever.http.LiveLimits",
                        &[
                            ("chunk_bytes", Int),
                            ("idle_ms", Int),
                            ("heartbeat_ms", Int),
                        ],
                        span,
                    )?;
                    handler = Some(HandlerSignature {
                        parameters: vec![request, HttpReply],
                        bounds: vec![Default::default(); 2],
                        outputs: vec![],
                    });
                    let mut parameters = vec![Listener, limits, live];
                    if matches!(operation, HttpServeLiveTls) {
                        parameters.push(ServerTls);
                    }
                    (parameters, Unit)
                }
                HttpServe | HttpServeTls => {
                    handler = Some(HandlerSignature {
                        parameters: vec![request],
                        bounds: vec![Default::default()],
                        outputs: vec![Field {
                            name: "response".into(),
                            ty: response,
                            private: false,
                            bounds: Default::default(),
                        }],
                    });
                    let mut parameters = vec![Listener, limits];
                    if matches!(operation, HttpServeTls) {
                        parameters.push(ServerTls);
                    }
                    (parameters, Unit)
                }
                HttpRespond | HttpStart | SseStart => {
                    let parameters = match operation {
                        HttpRespond => vec![HttpReply, response],
                        HttpStart => vec![HttpReply, Int, headers],
                        SseStart => vec![HttpReply, headers],
                        _ => unreachable!(),
                    };
                    (
                        parameters,
                        result("dever.io.WriteResult", "Done", &[], false)?,
                    )
                }
                HttpClientNew => {
                    let pool = record(
                        context,
                        "dever.http.PoolLimits",
                        &[("idle_ms", Int), ("chunk_bytes", Int), ("read_ms", Int)],
                        span,
                    )?;
                    (
                        vec![Text, ClientTls, limits, pool],
                        result("dever.http.ClientResult", "Ready", &[HttpClient], false)?,
                    )
                }
                HttpSend => (
                    vec![Text, Int, request, limits],
                    result("dever.http.ResponseResult", "Read", &[response], false)?,
                ),
                HttpRequest => (
                    vec![HttpClient, request],
                    result("dever.http.ResponseResult", "Read", &[response], false)?,
                ),
                HttpOpen | HttpOpenStream => {
                    let event = result("dever.io.ReadEvent", "Chunk", &[Bytes], false)?;
                    let body = AsyncStream(Box::new(event));
                    let response = record(
                        context,
                        "dever.http.StreamResponse",
                        &[
                            ("status", Int),
                            ("headers", headers.clone()),
                            ("body", body.clone()),
                        ],
                        span,
                    )?;
                    let request = if matches!(operation, HttpOpenStream) {
                        record(
                            context,
                            "dever.http.StreamRequest",
                            &[
                                ("method", Text),
                                ("target", Text),
                                ("headers", headers),
                                ("body", body),
                            ],
                            span,
                        )?
                    } else {
                        request
                    };
                    (
                        vec![HttpClient, request],
                        result(
                            "dever.http.StreamResponseResult",
                            "Read",
                            &[response],
                            false,
                        )?,
                    )
                }
                _ => unreachable!("HTTP record operation"),
            }
        }
        HttpWrite => (
            vec![HttpReply, Bytes],
            result("dever.io.WriteResult", "Done", &[], false)?,
        ),
        HttpFinish => (
            vec![HttpReply],
            result("dever.io.CloseResult", "Closed", &[], false)?,
        ),
        SseSend => {
            let event = record(
                context,
                "dever.sse.Event",
                &[
                    ("event", Text),
                    ("data", Text),
                    ("id", Text.nullable()),
                    ("retry_ms", Int.nullable()),
                ],
                span,
            )?;
            (
                vec![HttpReply, event],
                result("dever.io.WriteResult", "Done", &[], false)?,
            )
        }
        WsAccept | WsConnect | WsOpen => {
            let limits = record(
                context,
                "dever.websocket.Limits",
                &[("message_bytes", Int), ("idle_ms", Int), ("write_ms", Int)],
                span,
            )?;
            let parameters = match operation {
                WsAccept => vec![HttpReply, limits],
                WsOpen => vec![Text, ClientTls, limits],
                _ => vec![Text, Int, Text, limits],
            };
            (
                parameters,
                result(
                    "dever.websocket.ConnectResult",
                    "Connected",
                    &[WebSocket],
                    false,
                )?,
            )
        }
        WsSend | WsReceive | WsMessages => {
            let message = websocket_message(context, span)?;
            match operation {
                WsSend => (
                    vec![WebSocket, message],
                    result("dever.io.WriteResult", "Done", &[], false)?,
                ),
                WsReceive => (
                    vec![WebSocket],
                    result(
                        "dever.websocket.SystemReceiveResult",
                        "Read",
                        &[message],
                        true,
                    )?,
                ),
                WsMessages => {
                    let event = result("dever.websocket.MessageEvent", "Read", &[message], false)?;
                    (vec![WebSocket], AsyncStream(Box::new(event)))
                }
                _ => unreachable!(),
            }
        }
        WsClose => (
            vec![WebSocket, Int, Text],
            result("dever.io.CloseResult", "Closed", &[], false)?,
        ),
    };
    Ok(Signature {
        handler,
        parameters,
        output,
    })
}

fn record(
    context: &Context<'_>,
    name: &str,
    expected: &[(&str, Type)],
    span: Span,
) -> Checked<Type> {
    let id = context.named_type(name, span)?;
    let valid = match &context.types[id].shape {
        Shape::Record(fields) => {
            fields.len() == expected.len()
                && expected.iter().all(|(name, ty)| {
                    fields.iter().any(|field| {
                        field.name == *name
                            && field.ty == *ty
                            && !field.private
                            && field.bounds.is_empty()
                    })
                })
        }
        Shape::Choice(_) => false,
    };
    if !valid {
        return Err(Diagnostic::error(
            "C005",
            format!("'{name}' does not match its system record contract"),
            span,
        ));
    }
    Ok(Type::Named(id))
}

fn websocket_message(context: &Context<'_>, span: Span) -> Checked<Type> {
    let id = context.named_type("dever.websocket.Message", span)?;
    let expected = [
        ("Text", Type::Text),
        ("Binary", Type::Bytes),
        ("Ping", Type::Bytes),
        ("Pong", Type::Bytes),
    ];
    let valid = match &context.types[id].shape {
        Shape::Choice(variants) => {
            variants.len() == expected.len()
                && expected.iter().all(|(name, ty)| {
                    variants.iter().any(|variant| {
                        variant.name == *name
                            && !variant.error
                            && variant.fields.len() == 1
                            && variant.fields[0].ty == *ty
                            && variant.fields[0].bounds.is_empty()
                            && !variant.fields[0].private
                    })
                })
        }
        _ => false,
    };
    if !valid {
        return Err(Diagnostic::error(
            "C005",
            "WebSocket Message does not match its system contract",
            span,
        ));
    }
    Ok(Type::Named(id))
}
