//! Typed stream and TCP operations reuse coroutine polling and resource choices.
use super::resources::ResourceRow;
use super::*;

pub(super) const DECLARATIONS: &str = "\
declare i32 @dever_rt_v1_async_stream_from_list(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_async_stream_from_bytes(ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_async_channel_stream(ptr, ptr, ptr)
declare i32 @dever_rt_v1_async_ticks(i64, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_stream_retain(ptr)
declare void @dever_rt_v1_async_stream_release(ptr)
declare ptr @dever_rt_v1_async_stream_pull(ptr)
declare i32 @dever_rt_v1_async_stream_close(ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_net_connect(ptr, i64)
declare ptr @dever_rt_v1_async_net_connect_timeout(ptr, i64, i64)
declare ptr @dever_rt_v1_async_net_listen(ptr, i64)
declare ptr @dever_rt_v1_async_net_accept(ptr)
declare ptr @dever_rt_v1_async_net_read(ptr, i64)
declare ptr @dever_rt_v1_async_net_write(ptr, ptr)
declare ptr @dever_rt_v1_socket_retain(ptr)
declare void @dever_rt_v1_socket_release(ptr)
declare ptr @dever_rt_v1_listener_retain(ptr)
declare void @dever_rt_v1_listener_release(ptr)
declare i32 @dever_rt_v1_net_port(ptr, ptr, ptr)
declare i32 @dever_rt_v1_net_timeout(ptr, i64, ptr, ptr)
declare i32 @dever_rt_v1_socket_close(ptr, ptr, ptr)
declare i32 @dever_rt_v1_listener_close(ptr, ptr, ptr)
declare i32 @dever_rt_v1_net_chunks(ptr, i64, ptr, ptr, ptr, ptr)
declare i32 @dever_rt_v1_net_connections(ptr, ptr, ptr, ptr, ptr)
declare ptr @dever_rt_v1_async_task_wait_timeout(ptr, i64)
declare ptr @dever_rt_v1_async_task_race(ptr, i64, ptr)
";

impl FunctionEmitter<'_, '_> {
    pub(super) fn network_intrinsic(
        &mut self,
        operation: Intrinsic,
        arguments: &[Expression],
        mut values: Vec<String>,
        result: &Expression,
    ) -> String {
        use Intrinsic::*;
        if matches!(operation, StreamOf | TaskTicks) {
            let Type::AsyncStream(element) = &result.ty else {
                unreachable!("checked async stream")
            };
            let symbol = match operation {
                TaskTicks => "async_ticks",
                StreamOf => match arguments[0].ty {
                    Type::List(_) => "async_stream_from_list",
                    Type::Bytes => "async_stream_from_bytes",
                    Type::Channel(_) => "async_channel_stream",
                    _ => unreachable!("checked stream source"),
                },
                _ => unreachable!(),
            };
            if !matches!(arguments[0].ty, Type::Channel(_)) {
                values.push(format!(
                    "ptr @dever_owned_{}",
                    self.module.type_index(element)
                ));
            }
            return self.runtime_call(
                &format!("dever_rt_v1_{symbol}"),
                values,
                &result.ty,
                false,
                None,
                result.span,
            );
        }
        if matches!(operation, TcpChunks | TcpConnections) {
            let stream_type = if operation == TcpChunks {
                let Type::Named(id) = result.ty else {
                    unreachable!()
                };
                let Shape::Choice(variants) = &self.module.program.types[id].shape else {
                    unreachable!()
                };
                variants
                    .iter()
                    .find(|variant| variant.name == "Streaming")
                    .expect("checked Streaming")
                    .fields[0]
                    .ty
                    .clone()
            } else {
                result.ty.clone()
            };
            let Type::AsyncStream(element) = stream_type else {
                unreachable!()
            };
            let Type::Named(event) = element.as_ref() else {
                unreachable!()
            };
            let descriptor = self.module.read_event(*event);
            values.push(format!("ptr {descriptor}"));
            values.push(format!(
                "ptr @dever_owned_{}",
                self.module.type_index(&element)
            ));
            return if operation == TcpChunks {
                self.recoverable_call("dever_rt_v1_net_chunks", values, result, "Streaming", None)
            } else {
                self.runtime_call(
                    "dever_rt_v1_net_connections",
                    values,
                    &result.ty,
                    false,
                    None,
                    result.span,
                )
            };
        }
        let (symbol, success, end, asynchronous) = match operation {
            TcpConnect => ("async_net_connect", "Connected", None, true),
            TcpConnectTimeout => ("async_net_connect_timeout", "Connected", None, true),
            TcpListen => ("async_net_listen", "Listening", None, true),
            TcpAccept => ("async_net_accept", "Connected", None, true),
            TcpRead => ("async_net_read", "Read", Some("End"), true),
            TcpWrite => ("async_net_write", "Done", None, true),
            TcpPort => ("net_port", "Bound", None, false),
            TcpTimeout => ("net_timeout", "Done", None, false),
            TcpClose => ("socket_close", "Closed", None, false),
            TcpCloseListener => ("listener_close", "Closed", None, false),
            _ => unreachable!("checked network operation"),
        };
        if !asynchronous {
            return self.recoverable_call(
                &format!("dever_rt_v1_{symbol}"),
                values,
                result,
                success,
                end,
            );
        }
        let operation = self.temp();
        self.line(format!(
            "{operation} = call ptr @dever_rt_v1_{symbol}({})",
            values.join(", ")
        ));
        self.async_resource_result(&operation, result, success, end)
    }

    pub(super) fn async_resource_result(
        &mut self,
        operation: &str,
        result: &Expression,
        success: &str,
        end: Option<&str>,
    ) -> String {
        let Type::Named(id) = result.ty else {
            unreachable!("checked resource result")
        };
        let Shape::Choice(variants) = &self.module.program.types[id].shape else {
            unreachable!()
        };
        let mut row = variants
            .iter()
            .find(|variant| variant.name == success)
            .expect("checked success")
            .fields
            .first()
            .map_or(Type::Bool, |field| field.ty.clone());
        let protocol_output = matches!(&row, Type::Named(id) if matches!(self.module.program.types[*id].name.as_str(), "dever.http.Response" | "dever.http.StreamResponse" | "dever.websocket.Message"));
        if protocol_output {
            row = Type::Text;
        }
        let (status, output, present, error) = self.poll_raw(operation, &row);
        let typed_fault = self.temp();
        self.line(format!("{typed_fault} = icmp eq i32 {status}, 2"));
        let fault = self.label("resource_typed_fault");
        let ready = self.label("resource_polled");
        self.line(format!(
            "br i1 {typed_fault}, label %{fault}, label %{ready}"
        ));
        self.start(&fault);
        self.append_frame(result.span);
        self.exit("1");
        self.start(&ready);
        // Poll and synchronous ABI statuses have different numeric contracts.
        let okay = self.temp();
        self.line(format!("{okay} = icmp eq i32 {status}, 1"));
        let failed = self.temp();
        self.line(format!("{failed} = icmp eq i32 {status}, 3"));
        let failure_status = self.temp();
        self.line(format!(
            "{failure_status} = select i1 {failed}, i32 1, i32 2"
        ));
        let status = self.temp();
        self.line(format!(
            "{status} = select i1 {okay}, i32 0, i32 {failure_status}"
        ));
        self.resource_row(
            result,
            success,
            end,
            ResourceRow {
                status,
                output,
                present: end.map(|_| present),
                error,
                protocol_output,
            },
        )
    }
}
