use crate::hir::{Expression, HandlerTarget};
use crate::intrinsic::Intrinsic;
use crate::types::{Shape, Type};

use super::{DatabaseContext, Emitter};

impl Emitter<'_> {
    pub(super) fn intrinsic(
        &self,
        intrinsic: Intrinsic,
        handler: Option<HandlerTarget>,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        use Intrinsic::*;
        if matches!(
            intrinsic,
            UploadFilename | UploadContentType | UploadSize | UploadClose | UploadStore
        ) {
            let crate::hir::ExpressionKind::Local(slot) = arguments[0].kind else {
                unreachable!("checked upload local")
            };
            if matches!(intrinsic, UploadStore) {
                return self.resource_result(
                    &format!("dever_runtime::api::upload::store(_v{slot}).await"),
                    &result.ty,
                    "Stored",
                    false,
                );
            }
            let method = match intrinsic {
                UploadFilename => "filename",
                UploadContentType => "content_type",
                UploadSize => "size",
                _ => "close",
            };
            return format!("_v{slot}.{method}()");
        }
        let operation = match intrinsic {
            UploadFilename | UploadContentType | UploadSize | UploadClose | UploadStore => unreachable!("upload handled above"),
            TestSecret => "dever_runtime::secret::Secret::from_input(a0.into_bytes())".into(),
            CryptoToken => self.resource_result("dever_runtime::crypto::token(a0)", &result.ty, "Ready", false),
            CryptoPasswordHash => self.resource_result("dever_runtime::crypto::password_hash(&a0)", &result.ty, "Ready", false),
            CryptoPasswordVerify => self.resource_result("dever_runtime::crypto::password_verify(&a0, &a1)", &result.ty, "Ready", false),
            CryptoSha256 => "dever_runtime::crypto::sha256(&a0)".into(),
            CryptoHmacSha256 => "dever_runtime::crypto::hmac_sha256(&a0, &a1)".into(),
            CryptoConstantTimeEq => "dever_runtime::crypto::constant_time_eq(&a0, &a1)".into(),
            TestAssert => format!(
                "if !a0 {{ return Err(AppError::fault({}, \"assertion failed\")); }}",
                self.location(result.span)
            ),
            TestAssertEq => format!(
                "if a0 != a1 {{ let mut actual = String::new(); a0.render_to(&mut actual); let mut expected = String::new(); a1.render_to(&mut expected); return Err(AppError::fault({}, format!(\"assert_eq failed: actual = {{actual}}, expected = {{expected}}\"))); }}",
                self.location(result.span)
            ),
            TlsSystem => "dever_runtime::tls::system()".into(),
            TaskTicks => self.failure("task::ticks(a0)", result.span),
            TaskTimeout => {
                let target = crate::specialize::Specialization {
                    function: crate::specialize::resolve_handler(handler.expect("timeout handler"), &self.bindings),
                    handlers: vec![],
                };
                let wait = if crate::specialize::suspends(self.program, &target) { ".await" } else { "" };
                let invoke = self.invoke(&target, String::new(), DatabaseContext::Inherit);
                format!("match {} {{ Some(value) => value, None => {} }}",
                    self.call_failure("task::wait_timeout(a0, a1).await", result.span),
                    self.call_failure(&format!("{invoke}{wait}"), result.span))
            }
            TaskRace => {
                let tasks = (0..arguments.len()).map(|index| format!("a{index}")).collect::<Vec<_>>().join(", ");
                self.call_failure(&format!("task::race(vec![{tasks}]).await"), result.span)
            }
            StreamOf => match &arguments[0].ty {
                Type::Channel(_) => "a0.stream()".into(),
                Type::List(_) => "dever_runtime::async_stream::AsyncStream::from_values(a0.into_values())".into(),
                Type::Bytes => "dever_runtime::async_stream::AsyncStream::from_values(a0.into_values().map(i64::from))".into(),
                _ => unreachable!(),
            },
            TextTrim => "a0.trim().to_owned()".into(),
            TextLower => "a0.to_lowercase()".into(),
            TextUpper => "a0.to_uppercase()".into(),
            TextCodepoint => "dever_runtime::text::codepoint(&a0)".into(),
            TextFromCodepoint => "dever_runtime::text::from_codepoint(a0)".into(),
            TextAt => "dever_runtime::text::at(&a0, a1)".into(),
            TextSlice => "dever_runtime::text::slice(&a0, a1, a2)".into(),
            TextIndexOf => "dever_runtime::text::index_of(&a0, &a1)".into(),
            TextContains => "a0.contains(&a1)".into(),
            TextStartsWith => "a0.starts_with(&a1)".into(),
            TextEndsWith => "a0.ends_with(&a1)".into(),
            TextSplit => "dever_runtime::text::split(&a0, &a1)".into(),
            TextReplace => "a0.replace(&a1, &a2)".into(),
            IntParse => "number::parse_int(&a0)".into(),
            IntToText | DecimalToText | FloatToText => "a0.to_string()".into(),
            DecimalParse => "number::parse_decimal(&a0)".into(),
            DecimalFromInt => "DecimalValue::from_int(a0)".into(),
            FloatParse => "number::parse_float(&a0)".into(),
            ProcessArguments => self.resource_result(
                "dever_runtime::process::arguments()",
                &result.ty,
                "Read",
                false,
            ),
            TimeUnixMillis => self.resource_result(
                "dever_runtime::time::unix_millis()",
                &result.ty,
                "Read",
                false,
            ),
            TimeNow => self.resource_result("dever_runtime::time::now()", &result.ty, "Read", false),
            TimeParseDateTime => self.resource_result("dever_runtime::time::parse_datetime(&a0)", &result.ty, "Read", false),
            TimeFormatDateTime => self.resource_result("dever_runtime::time::format_datetime(a0)", &result.ty, "Read", false),
            TimeParseDate => self.resource_result("dever_runtime::time::parse_date(&a0)", &result.ty, "Read", false),
            TimeFormatDate => self.resource_result("dever_runtime::time::format_date(a0)", &result.ty, "Read", false),
            TimeParseTime => self.resource_result("dever_runtime::time::parse_time(&a0)", &result.ty, "Read", false),
            TimeFormatTime => self.resource_result("dever_runtime::time::format_time(a0)", &result.ty, "Read", false),
            TimeDuration => "dever_runtime::time::duration(a0)".into(),
            TimeAdd => self.resource_result("dever_runtime::time::add(a0, a1)", &result.ty, "Read", false),
            TimeSubtract => self.resource_result("dever_runtime::time::subtract(a0, a1)", &result.ty, "Read", false),
            TimeDifference => self.resource_result("dever_runtime::time::difference(a0, a1)", &result.ty, "Read", false),
            TimeMonotonicNanos => self.resource_result(
                "dever_runtime::time::monotonic_nanos()",
                &result.ty,
                "Read",
                false,
            ),
            TimeSleep => {
                self.resource_result("dever_runtime::time::sleep(a0)", &result.ty, "Done", false)
            }
            TaskSleep => self.resource_result(
                "task::sleep(a0).await",
                &result.ty,
                "Done",
                false,
            ),
            DecimalRound => self.failure("a0.round(a1)", result.span),
            FloatFromInt => "a0 as f64".into(),
            FloatSqrt => "a0.sqrt()".into(),
            FloatSin => "a0.sin()".into(),
            FloatCos => "a0.cos()".into(),
            FloatLog => "a0.ln()".into(),
            FloatPow => "a0.powf(a1)".into(),
            FloatIsNan => "a0.is_nan()".into(),
            FloatIsFinite => "a0.is_finite()".into(),
            FloatIsInfinite => "a0.is_infinite()".into(),
            StdoutWrite => self.failure("resource::stdout_write(&a0)", result.span),
            LogDebug | LogInfo | LogWarn | LogError => {
                let level = match intrinsic {
                    LogDebug => "debug",
                    LogInfo => "info",
                    LogWarn => "warn",
                    LogError => "error",
                    _ => unreachable!(),
                };
                format!(
                    "dever_runtime::log::{level}(a0, a1.pairs().map(|(name, value)| dever_runtime::log::Field {{ name: name.clone(), value: value.clone() }}).collect())"
                )
            }
            IdFromText => "dever_runtime::Id(a0)".into(),
            IdToText => "a0.0".into(),
            UuidParse => "dever_runtime::orm::Uuid::parse(&a0).ok()".into(),
            UuidToText => "a0.to_string()".into(),
            BytesFromText => "Bytes::from_string(a0)".into(),
            BytesLength => "a0.length()".into(),
            BytesAt => "a0.at(a1)".into(),
            BytesConcat => "a0.concat(&a1)".into(),
            BytesToText => self.resource_result("a0.into_text()", &result.ty, "Decoded", false),
            BytesFromInts => {
                self.resource_result("Bytes::from_ints(a0.values())", &result.ty, "Ready", false)
            }
            BytesSlice => self.resource_result("a0.slice(a1, a2)", &result.ty, "Ready", false),
            FileOpen => {
                self.resource_result("resource::file_open(&a0)", &result.ty, "Opened", false)
            }
            FileCreate => {
                self.resource_result("resource::file_create(&a0)", &result.ty, "Opened", false)
            }
            FileRead => {
                self.resource_result("resource::read(&a0, a1)", &result.ty, "Read", true)
            }
            FileWrite => {
                self.resource_result("resource::write(&a0, &a1)", &result.ty, "Done", false)
            }
            FileClose | TcpClose | TcpCloseListener => {
                self.resource_result("a0.close()", &result.ty, "Closed", false)
            }
            FileChunks => self.read_stream_result("resource::read_chunks(a0, a1)", &result.ty),
            TcpChunks => self.read_stream_result("dever_runtime::net::chunks(a0, a1)", &result.ty),
            TcpRead => self.resource_result(
                "dever_runtime::net::read(&a0, a1).await", &result.ty, "Read", true,
            ),
            TcpWrite => self.resource_result(
                "dever_runtime::net::write(&a0, &a1).await", &result.ty, "Done", false,
            ),
            TcpConnect => self.resource_result(
                "dever_runtime::net::connect(&a0, a1).await",
                &result.ty,
                "Connected",
                false,
            ),
            TcpConnectTimeout => self.resource_result(
                "dever_runtime::net::connect_timeout(&a0, a1, a2).await",
                &result.ty,
                "Connected",
                false,
            ),
            TcpListen => self.resource_result(
                "dever_runtime::net::listen(&a0, a1).await",
                &result.ty,
                "Listening",
                false,
            ),
            TcpAccept => {
                self.resource_result("dever_runtime::net::accept(&a0).await", &result.ty, "Connected", false)
            }
            TcpPort => self.resource_result("dever_runtime::net::port(&a0)", &result.ty, "Bound", false),
            TcpTimeout => {
                self.resource_result("dever_runtime::net::timeout(&a0, a1)", &result.ty, "Done", false)
            }
            TcpConnections => self.connection_stream(&result.ty),
            ApiServe => self.failure("dever_runtime::api::serve(api_route).await", result.span),
            ApiRequestId => self.failure("dever_runtime::api::request_id()", result.span),
            ApiMethod => self.failure("dever_runtime::api::method()", result.span),
            ApiPath => self.failure("dever_runtime::api::path()", result.span),
            ApiClientAddress => self.failure("dever_runtime::api::client_address()", result.span),
            ApiHeader => self.failure("dever_runtime::api::header(&a0)", result.span),
            ApiCookie => self.failure("dever_runtime::api::cookie(&a0)", result.span),
            ApiSecretCookie => self.failure("dever_runtime::api::secret_cookie(&a0)", result.span),
            ApiResponseHeader => self.failure("dever_runtime::api::response_header(&a0, &a1)", result.span),
            ApiResponseCookie => self.failure(&format!(
                "dever_runtime::api::response_cookie(&a0, &a1, {})",
                self.api_cookie_options(),
            ), result.span),
            ApiResponseSecretCookie => self.failure(&format!(
                "dever_runtime::api::response_secret_cookie(&a0, a1, {})",
                self.api_cookie_options(),
            ), result.span),
            AuthIssue => self.failure("dever_runtime::auth::issue(&a0, &a1, a2.as_deref())", result.span),
            AuthIssueCookie => self.failure("dever_runtime::auth::issue_cookie(&a0, &a1, a2.as_deref())", result.span),
            AuthClearCookie => self.failure("dever_runtime::auth::clear_cookie()", result.span),
            AuthOwnsUser => self.failure("dever_runtime::auth::owns_user(a0.0)", result.span),
            AuthId => self.failure("dever_runtime::auth::id()", result.span),
            AuthSession => self.failure("dever_runtime::auth::session()", result.span),
            AuthUserId | AuthTenantId => {
                let method = if intrinsic == AuthUserId { "user_id" } else { "tenant_id" };
                let call = format!("dever_runtime::auth::{method}()");
                let call = match &result.ty {
                    Type::Nullable(inner) => match inner.as_ref() {
                        Type::Named(id) => format!("{call}.map(|value| value.map(T{id}))"),
                        _ => call,
                    },
                    _ => unreachable!("authentication ID getter returns a nullable value"),
                };
                self.failure(&call, result.span)
            }
            AuthPermissions => {
                let Type::List(permission) = &result.ty else { unreachable!("checked permission list") };
                let Type::Named(permission) = permission.as_ref() else { unreachable!("checked Permission type") };
                self.failure(&format!(
                    "dever_runtime::auth::permissions().await.map(|values| List::new(values.into_iter().map(|value| T{permission} {{ f0: value.key, f1: value.component, f2: value.domain, f3: value.site, f4: value.action, f5: value.method }}).collect()))"
                ), result.span)
            }
            AuthSaveRole => self.failure(
                "dever_runtime::auth::save_role(&a0, &a1, a2, a3.into_values().collect()).await",
                result.span,
            ),
            AuthGrantRole => self.failure(
                "dever_runtime::auth::grant_role(a0, &a1).await",
                result.span,
            ),
            AuthRevokeRole => self.failure(
                "dever_runtime::auth::revoke_role(a0, &a1).await",
                result.span,
            ),
            AuthDisableRole => self.failure(
                "dever_runtime::auth::disable_role(&a0).await",
                result.span,
            ),
            SiteKey => self.failure("dever_runtime::auth::site_key()", result.span),
            JobServe => self.failure("job_serve().await", result.span),
            TestJobDrain => self.failure("job_drain(a0).await", result.span),
            TestClockAdvance => self.failure("JOB_CLOCK.get().expect(\"test clock initialized\").advance(a0)", result.span),
            HttpServe | HttpServeTls => self.http_serve(handler.expect("checked HTTP handler"), arguments, result, false, matches!(intrinsic, HttpServeTls)),
            HttpServeLive | HttpServeLiveTls => self.http_serve(handler.expect("checked HTTP handler"), arguments, result, true, matches!(intrinsic, HttpServeLiveTls)),
            HttpSend => self.http_send(arguments, result),
            operation @ (HttpRespond | HttpStart | HttpWrite | HttpFinish | SseStart | SseSend
            | WsAccept | WsConnect | WsSend | WsReceive | WsMessages | WsClose | WsOpen
            | TlsClient | TlsServer | HttpClientNew | HttpCloseClient | HttpRequest | HttpOpen | HttpOpenStream) => self.live_network(operation, arguments, result),
        };
        format!("{{ {}\n{operation} }}", self.bind_arguments(arguments))
    }

    fn api_cookie_options(&self) -> String {
        let (id, definition) = self
            .program
            .types
            .iter()
            .enumerate()
            .find(|(_, definition)| definition.name == "dever.api.SameSite")
            .expect("official SameSite type");
        let Shape::Choice(variants) = &definition.shape else {
            unreachable!("official SameSite choice")
        };
        let arms = variants
            .iter()
            .enumerate()
            .map(|(index, variant)| {
                let name = match variant.name.as_str() {
                    "Strict" => "Strict",
                    "Lax" => "Lax",
                    "None" => "None",
                    _ => unreachable!("official SameSite variant"),
                };
                format!("T{id}::V{index} => dever_runtime::api::SameSite::{name}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "dever_runtime::api::CookieOptions {{ path: a2.f0, domain: a2.f1, same_site: match a2.f2 {{ {arms} }}, secure: a2.f3, http_only: a2.f4, max_age: a2.f5 }}"
        )
    }

    fn read_stream_result(&self, call: &str, output: &Type) -> String {
        let Type::Named(result_id) = output else {
            unreachable!("checked read stream result")
        };
        let Shape::Choice(result_variants) = &self.program.types[*result_id].shape else {
            unreachable!()
        };
        let streaming = variant(result_variants, "Streaming");
        let failed = variant(result_variants, "Failed");
        let (Type::Stream(event) | Type::AsyncStream(event)) =
            &result_variants[streaming].fields[0].ty
        else {
            unreachable!()
        };
        let Type::Named(event_id) = event.as_ref() else {
            unreachable!()
        };
        let Shape::Choice(event_variants) = &self.program.types[*event_id].shape else {
            unreachable!()
        };
        let chunk = variant(event_variants, "Chunk");
        let event_failed = variant(event_variants, "Failed");
        format!(
            "match {call} {{ Ok(stream) => T{result_id}::V{streaming}(stream.map(|event| match event {{ Ok(bytes) => T{event_id}::V{chunk}(bytes), Err(message) => T{event_id}::V{event_failed}(message) }})), Err(message) => T{result_id}::V{failed}(message) }}"
        )
    }

    fn connection_stream(&self, output: &Type) -> String {
        let Type::AsyncStream(connection) = output else {
            unreachable!("checked connection Stream")
        };
        let Type::Named(id) = connection.as_ref() else {
            unreachable!()
        };
        let Shape::Choice(variants) = &self.program.types[*id].shape else {
            unreachable!()
        };
        let connected = variant(variants, "Connected");
        let failed = variant(variants, "Failed");
        format!(
            "dever_runtime::net::connections(a0).map(|event| match event {{ Ok(socket) => T{id}::V{connected}(socket), Err(message) => T{id}::V{failed}(message) }})"
        )
    }

    pub(super) fn resource_result(
        &self,
        call: &str,
        output: &Type,
        success_name: &str,
        eof: bool,
    ) -> String {
        let Type::Named(id) = output else {
            unreachable!("official result choice")
        };
        let Shape::Choice(variants) = &self.program.types[*id].shape else {
            unreachable!()
        };
        let variant = |name| {
            variants
                .iter()
                .position(|variant| variant.name == name)
                .expect("checked system result contract")
        };
        let success = variant(success_name);
        let failed = variant("Failed");
        if eof {
            let end = variant("End");
            format!(
                "match {call} {{ Ok(Some(bytes)) => T{id}::V{success}(bytes), Ok(None) => T{id}::V{end}, Err(message) => T{id}::V{failed}(message.into()) }}"
            )
        } else if variants[success].fields.is_empty() {
            format!(
                "match {call} {{ Ok(()) => T{id}::V{success}, Err(message) => T{id}::V{failed}(message.into()) }}"
            )
        } else {
            format!(
                "match {call} {{ Ok(value) => T{id}::V{success}(value), Err(message) => T{id}::V{failed}(message.into()) }}"
            )
        }
    }
}

fn variant(variants: &[crate::types::Variant], name: &str) -> usize {
    variants
        .iter()
        .position(|variant| variant.name == name)
        .expect("checked system result contract")
}
