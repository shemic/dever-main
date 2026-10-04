//! Non-application system capabilities keep their native runtime contracts.
use super::*;

pub(super) const DECLARATIONS: &str = "\
declare i32 @dever_rt_v1_process_arguments(ptr, ptr, ptr)
declare i32 @dever_rt_v1_time_unix_millis(ptr, ptr)
declare i32 @dever_rt_v1_time_now(ptr, ptr)
declare i32 @dever_rt_v1_time_monotonic_nanos(ptr, ptr)
declare i32 @dever_rt_v1_time_parse_datetime(ptr, ptr, ptr)
declare i32 @dever_rt_v1_time_parse_date(ptr, ptr, ptr)
declare i32 @dever_rt_v1_time_parse_time(ptr, ptr, ptr)
declare i32 @dever_rt_v1_time_format_datetime(i64, ptr, ptr)
declare i32 @dever_rt_v1_time_format_date(i64, ptr, ptr)
declare i32 @dever_rt_v1_time_format_time(i64, ptr, ptr)
declare i32 @dever_rt_v1_time_add(i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_time_subtract(i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_time_difference(i64, i64, ptr, ptr)
declare i32 @dever_rt_v1_stdout_write(ptr, ptr, ptr)
declare i32 @dever_rt_v1_log_write(i32, ptr, ptr, ptr, ptr)
declare void @dever_rt_v1_log_flush()
declare i32 @dever_rt_v1_uuid_parse(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_uuid_to_text(ptr, ptr, ptr)
declare i8 @dever_rt_v1_uuid_equal(ptr, ptr)
declare i32 @dever_rt_v1_secret_from_text(ptr, ptr, ptr)
declare i32 @dever_rt_v1_crypto_token(i64, ptr, ptr)
declare i32 @dever_rt_v1_crypto_password_hash(ptr, ptr, ptr)
declare i32 @dever_rt_v1_crypto_password_verify(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_crypto_sha256(ptr, ptr, ptr)
declare i32 @dever_rt_v1_crypto_hmac_sha256(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_crypto_constant_time_eq(ptr, ptr, ptr, ptr)
";

pub(super) fn supported(operation: Intrinsic) -> bool {
    use Intrinsic::*;
    matches!(
        operation,
        ProcessArguments
            | TimeUnixMillis
            | TimeNow
            | TimeMonotonicNanos
            | TimeParseDateTime
            | TimeParseDate
            | TimeParseTime
            | TimeFormatDateTime
            | TimeFormatDate
            | TimeFormatTime
            | TimeDuration
            | TimeAdd
            | TimeSubtract
            | TimeDifference
            | StdoutWrite
            | LogDebug
            | LogInfo
            | LogWarn
            | LogError
            | UuidParse
            | UuidToText
            | TestSecret
            | CryptoToken
            | CryptoPasswordHash
            | CryptoPasswordVerify
            | CryptoSha256
            | CryptoHmacSha256
            | CryptoConstantTimeEq
    )
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn uuid_from_literal_text(&mut self, text: &str, span: Span) -> String {
        let parsed = self.runtime_call(
            "dever_rt_v1_uuid_parse",
            vec![format!("ptr {text}")],
            &Type::Nullable(Box::new(Type::Uuid)),
            true,
            None,
            span,
        );
        // Literal validation already proved this exact spelling parses.
        let uuid = self.temp();
        self.line(format!("{uuid} = extractvalue {{ i1, ptr }} {parsed}, 1"));
        uuid
    }
    pub(super) fn system_intrinsic(
        &mut self,
        operation: Intrinsic,
        mut args: Vec<String>,
        values: &[String],
        result: &Expression,
    ) -> String {
        use Intrinsic::*;
        let (symbol, success) = match operation {
            TimeDuration => return values[0].clone(),
            ProcessArguments => {
                args.push(format!(
                    "ptr @dever_type_{}",
                    self.module.type_index(&Type::Text)
                ));
                ("process_arguments", Some("Read"))
            }
            TimeUnixMillis => ("time_unix_millis", Some("Read")),
            TimeNow => ("time_now", Some("Read")),
            TimeMonotonicNanos => ("time_monotonic_nanos", Some("Read")),
            TimeParseDateTime => ("time_parse_datetime", Some("Read")),
            TimeParseDate => ("time_parse_date", Some("Read")),
            TimeParseTime => ("time_parse_time", Some("Read")),
            TimeFormatDateTime => ("time_format_datetime", Some("Read")),
            TimeFormatDate => ("time_format_date", Some("Read")),
            TimeFormatTime => ("time_format_time", Some("Read")),
            TimeAdd => ("time_add", Some("Read")),
            TimeSubtract => ("time_subtract", Some("Read")),
            TimeDifference => ("time_difference", Some("Read")),
            CryptoToken => ("crypto_token", Some("Ready")),
            CryptoPasswordHash => ("crypto_password_hash", Some("Ready")),
            CryptoPasswordVerify => ("crypto_password_verify", Some("Ready")),
            CryptoSha256 => ("crypto_sha256", None),
            CryptoHmacSha256 => ("crypto_hmac_sha256", None),
            CryptoConstantTimeEq => ("crypto_constant_time_eq", None),
            TestSecret => ("secret_from_text", None),
            UuidParse => ("uuid_parse", None),
            UuidToText => ("uuid_to_text", None),
            StdoutWrite => ("stdout_write", None),
            LogDebug | LogInfo | LogWarn | LogError => {
                let level = match operation {
                    LogDebug => 0,
                    LogInfo => 1,
                    LogWarn => 2,
                    LogError => 3,
                    _ => unreachable!(),
                };
                args.insert(0, format!("i32 {level}"));
                ("log_write", None)
            }
            _ => unreachable!("checked system intrinsic"),
        };
        let symbol = format!("dever_rt_v1_{symbol}");
        if let Some(success) = success {
            return self.recoverable_call(&symbol, args, result, success, None);
        }
        let physical = matches!(result.ty, Type::Bool | Type::Unit).then_some("i8");
        let value = self.runtime_call(
            &symbol,
            args,
            &result.ty,
            matches!(result.ty, Type::Nullable(_)),
            physical,
            result.span,
        );
        if result.ty == Type::Unit {
            "zeroinitializer".into()
        } else {
            value
        }
    }
}
