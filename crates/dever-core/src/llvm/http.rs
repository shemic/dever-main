//! Protocol ABI conversion is field-name based, independent of source layout.
use super::*;

pub(super) const DECLARATIONS: &str = "\
%dever.http_handler = type { ptr, ptr, ptr, ptr, ptr, ptr, ptr }
declare ptr @dever_rt_v1_http_headers_retain(ptr)
declare void @dever_rt_v1_http_headers_release(ptr)
declare void @dever_rt_v1_http_request_release(ptr)
declare ptr @dever_rt_v1_http_request_retain(ptr)
declare void @dever_rt_v1_http_response_release(ptr)
declare void @dever_rt_v1_http_stream_request_release(ptr)
declare void @dever_rt_v1_http_stream_response_release(ptr)
declare void @dever_rt_v1_ws_message_release(ptr)
declare i32 @dever_rt_v1_http_headers_new(ptr, ptr)
declare i32 @dever_rt_v1_http_headers_push(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_http_headers_length(ptr, ptr, ptr)
declare i32 @dever_rt_v1_http_headers_get(ptr, i64, ptr, ptr)
declare i32 @dever_rt_v1_http_request_new(ptr, ptr, ptr)
declare i32 @dever_rt_v1_http_request_fields(ptr, ptr, ptr)
declare i32 @dever_rt_v1_http_response_new(ptr, ptr, ptr)
declare i32 @dever_rt_v1_http_response_fields(ptr, ptr, ptr)
declare i32 @dever_rt_v1_http_stream_request_new(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_http_stream_response_fields(ptr, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_tls_system(ptr, ptr)
declare i32 @dever_rt_v1_tls_client(ptr, ptr, ptr)
declare i32 @dever_rt_v1_tls_server(ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_http_send(ptr, i64, ptr, ptr)
declare ptr @dever_rt_v1_async_http_client(ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_http_request(ptr, ptr)
declare ptr @dever_rt_v1_async_http_open(ptr, ptr)
declare ptr @dever_rt_v1_async_http_open_stream(ptr, ptr)
declare ptr @dever_rt_v1_async_http_close_client(ptr)
declare ptr @dever_rt_v1_async_http_serve(ptr, ptr, ptr, ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_http_respond(ptr, ptr)
declare ptr @dever_rt_v1_async_http_start(ptr, i64, ptr)
declare ptr @dever_rt_v1_async_http_write(ptr, ptr)
declare ptr @dever_rt_v1_async_http_finish(ptr)
declare ptr @dever_rt_v1_async_sse_start(ptr, ptr)
declare ptr @dever_rt_v1_async_sse_send(ptr, ptr)
declare ptr @dever_rt_v1_async_ws_accept(ptr, ptr)
declare ptr @dever_rt_v1_async_ws_connect(ptr, i64, ptr, ptr)
declare ptr @dever_rt_v1_async_ws_open(ptr, ptr, ptr)
declare i32 @dever_rt_v1_ws_message_new(i32, ptr, ptr, ptr)
declare i32 @dever_rt_v1_ws_message_fields(ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_ws_send(ptr, ptr)
declare ptr @dever_rt_v1_async_ws_receive(ptr)
declare i32 @dever_rt_v1_ws_messages(ptr, ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_ws_close(ptr, i64, ptr)
";

pub(super) fn supported(operation: Intrinsic) -> bool {
    use Intrinsic::*;
    matches!(
        operation,
        TlsSystem
            | TlsClient
            | TlsServer
            | HttpSend
            | HttpClientNew
            | HttpRequest
            | HttpOpen
            | HttpOpenStream
            | HttpCloseClient
            | HttpServe
            | HttpServeTls
            | HttpServeLive
            | HttpServeLiveTls
            | HttpRespond
            | HttpStart
            | HttpWrite
            | HttpFinish
            | SseStart
            | SseSend
            | WsAccept
            | WsConnect
            | WsOpen
            | WsSend
            | WsReceive
            | WsMessages
            | WsClose
    )
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn http_intrinsic(
        &mut self,
        operation: Intrinsic,
        handler: Option<crate::hir::HandlerTarget>,
        arguments: &[Expression],
        values: &[String],
        result: &Expression,
    ) -> String {
        use Intrinsic::*;
        let mut args = arguments
            .iter()
            .zip(values)
            .map(|(argument, value)| format!("{} {value}", self.module.ty(&argument.ty)))
            .collect::<Vec<_>>();
        match operation {
            TlsSystem => {
                return self.runtime_call(
                    "dever_rt_v1_tls_system",
                    args,
                    &result.ty,
                    false,
                    None,
                    result.span,
                );
            }
            TlsClient | TlsServer => {
                return self.recoverable_call(
                    if operation == TlsClient {
                        "dever_rt_v1_tls_client"
                    } else {
                        "dever_rt_v1_tls_server"
                    },
                    args,
                    result,
                    "Ready",
                    None,
                );
            }
            HttpServe | HttpServeTls | HttpServeLive | HttpServeLiveTls => {
                return self.http_serve(
                    operation,
                    handler.expect("checked HTTP handler"),
                    arguments,
                    values,
                    result,
                );
            }
            HttpSend => {
                args[2] = format!(
                    "ptr {}",
                    self.protocol_to_runtime(&arguments[2].ty, &values[2], result.span)
                );
                args[3] = format!("ptr {}", self.http_limits(&arguments[3].ty, &values[3]));
            }
            HttpClientNew => {
                args[2] = format!("ptr {}", self.http_limits(&arguments[2].ty, &values[2]));
                args[3] = format!(
                    "ptr {}",
                    self.protocol_limits(
                        &arguments[3].ty,
                        &values[3],
                        &["idle_ms", "chunk_bytes", "read_ms"]
                    )
                );
            }
            HttpRequest | HttpOpen | HttpOpenStream | HttpRespond | WsSend => {
                args[1] = format!(
                    "ptr {}",
                    self.protocol_to_runtime(&arguments[1].ty, &values[1], result.span)
                );
            }
            HttpStart | SseStart => {
                let index = if operation == HttpStart { 2 } else { 1 };
                args[index] = format!(
                    "ptr {}",
                    self.headers_to_runtime(&arguments[index].ty, &values[index], result.span)
                );
            }
            SseSend => args[1] = format!("ptr {}", self.sse_event(&arguments[1].ty, &values[1])),
            WsAccept | WsConnect | WsOpen => {
                let index = args.len() - 1;
                args[index] = format!(
                    "ptr {}",
                    self.protocol_limits(
                        &arguments[index].ty,
                        &values[index],
                        &["message_bytes", "idle_ms", "write_ms"]
                    )
                );
            }
            WsMessages => {
                let Type::AsyncStream(event) = &result.ty else {
                    unreachable!()
                };
                let Type::Named(id) = event.as_ref() else {
                    unreachable!()
                };
                let descriptor = self.module.websocket_event(*id);
                args.push(format!("ptr {descriptor}"));
                args.push(format!(
                    "ptr @dever_owned_{}",
                    self.module.type_index(event)
                ));
                return self.runtime_call(
                    "dever_rt_v1_ws_messages",
                    args,
                    &result.ty,
                    false,
                    None,
                    result.span,
                );
            }
            _ => {}
        }
        let (symbol, success, end) = match operation {
            HttpSend => ("http_send", "Read", None),
            HttpClientNew => ("http_client", "Ready", None),
            HttpRequest => ("http_request", "Read", None),
            HttpOpen => ("http_open", "Read", None),
            HttpOpenStream => ("http_open_stream", "Read", None),
            HttpCloseClient => ("http_close_client", "Closed", None),
            HttpRespond => ("http_respond", "Done", None),
            HttpStart => ("http_start", "Done", None),
            HttpWrite => ("http_write", "Done", None),
            HttpFinish => ("http_finish", "Closed", None),
            SseStart => ("sse_start", "Done", None),
            SseSend => ("sse_send", "Done", None),
            WsAccept => ("ws_accept", "Connected", None),
            WsConnect => ("ws_connect", "Connected", None),
            WsOpen => ("ws_open", "Connected", None),
            WsSend => ("ws_send", "Done", None),
            WsReceive => ("ws_receive", "Read", Some("End")),
            WsClose => ("ws_close", "Closed", None),
            _ => unreachable!("checked HTTP operation"),
        };
        let operation = self.temp();
        self.line(format!(
            "{operation} = call ptr @dever_rt_v1_async_{symbol}({})",
            args.join(", ")
        ));
        self.async_resource_result(&operation, result, success, end)
    }
}
