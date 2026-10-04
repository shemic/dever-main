use crate::hir::{Expression, HandlerTarget};
use crate::specialize::{self, Specialization};
use crate::types::{Parameter, Shape, Type};

use super::{DatabaseContext, Emitter};

#[derive(Clone, Copy)]
enum Direction {
    ToRuntime,
    ToLanguage,
}

impl Emitter<'_> {
    pub(super) fn live_network(
        &self,
        operation: crate::intrinsic::Intrinsic,
        arguments: &[Expression],
        result: &Expression,
    ) -> String {
        use crate::intrinsic::Intrinsic::*;
        let input = |index: usize| {
            self.network_value(
                &format!("a{index}"),
                &arguments[index].ty,
                Direction::ToRuntime,
            )
        };
        let call = match operation {
            TlsClient => "dever_runtime::tls::client(&a0)".into(),
            TlsServer => "dever_runtime::tls::server(&a0, &a1)".into(),
            HttpClientNew => format!(
                "dever_runtime::http::client(&a0, a1, {}, {}).await",
                input(2),
                input(3)
            ),
            HttpCloseClient => "dever_runtime::http::close_client(&a0).await".into(),
            HttpRequest | HttpOpen | HttpOpenStream => {
                let method = match operation {
                    HttpRequest => "request",
                    HttpOpen => "open",
                    _ => "open_stream",
                };
                return self.http_result(
                    &format!("dever_runtime::http::{method}(&a0, {}).await", input(1)),
                    result,
                );
            }
            HttpRespond => format!("dever_runtime::http::respond(&a0, {}).await", input(1)),
            HttpStart => format!("dever_runtime::http::start(&a0, a1, {}).await", input(2)),
            HttpWrite => "dever_runtime::http::write(&a0, a1).await".into(),
            HttpFinish => "dever_runtime::http::finish(&a0).await".into(),
            SseStart => format!("dever_runtime::sse::start(&a0, {}).await", input(1)),
            SseSend => format!("dever_runtime::sse::send(&a0, {}).await", input(1)),
            WsAccept => format!("dever_runtime::websocket::accept(&a0, {}).await", input(1)),
            WsConnect => format!(
                "dever_runtime::websocket::connect(&a0, a1, &a2, {}).await",
                input(3)
            ),
            WsOpen => format!(
                "dever_runtime::websocket::open(&a0, a1, {}).await",
                input(2)
            ),
            WsSend => format!("dever_runtime::websocket::send(&a0, {}).await", input(1)),
            WsClose => "dever_runtime::websocket::close(&a0, a1, &a2).await".into(),
            WsReceive | WsMessages => {
                let event = match &result.ty {
                    Type::AsyncStream(event) => event.as_ref(),
                    event => event,
                };
                let Type::Named(id) = event else {
                    unreachable!()
                };
                let Shape::Choice(variants) = &self.program.types[*id].shape else {
                    unreachable!()
                };
                let read = variants
                    .iter()
                    .find(|variant| variant.name == "Read")
                    .unwrap();
                let message =
                    self.network_value("message", &read.fields[0].ty, Direction::ToLanguage);
                if matches!(operation, WsMessages) {
                    let event = self.resource_result(
                        &format!("event.map(|message| {message})"),
                        event,
                        "Read",
                        false,
                    );
                    return format!("dever_runtime::websocket::messages(a0).map(|event| {event})");
                }
                return self.resource_result(&format!(
                    "dever_runtime::websocket::receive(&a0).await.map(|message| message.map(|message| {message}))"
                ), event, "Read", true);
            }
            _ => unreachable!("live network intrinsic"),
        };
        let success = match operation {
            TlsClient | TlsServer | HttpClientNew => "Ready",
            WsAccept | WsConnect | WsOpen => "Connected",
            HttpFinish | WsClose | HttpCloseClient => "Closed",
            _ => "Done",
        };
        self.resource_result(&call, &result.ty, success, false)
    }

    pub(super) fn http_serve(
        &self,
        handler: HandlerTarget,
        arguments: &[Expression],
        result: &Expression,
        live: bool,
        tls: bool,
    ) -> String {
        let target = Specialization {
            function: specialize::resolve_handler(handler, &self.bindings),
            handlers: Vec::new(),
        };
        let wait = if specialize::suspends(self.program, &target) {
            ".await"
        } else {
            ""
        };
        let function = &self.program.functions[target.function];
        let Parameter::Value(request) = &function.parameters[0] else {
            unreachable!("checked HTTP request")
        };
        let request = self.network_value("request", request, Direction::ToLanguage);
        let tls_index = if live { 3 } else { 2 };
        let context_index = tls_index + usize::from(tls);
        let tls_argument = if tls {
            format!(", a{tls_index}")
        } else {
            String::new()
        };
        let suffix = if tls { "_tls" } else { "" };
        let (capture, context) = if arguments.len() > context_index {
            (
                format!("let context = a{context_index}.clone();"),
                ", context",
            )
        } else {
            (String::new(), "")
        };
        if live {
            let limits = self.network_value("a1", &arguments[1].ty, Direction::ToRuntime);
            let live_limits = self.network_value("a2", &arguments[2].ty, Direction::ToRuntime);
            let invoke = self.invoke(
                &target,
                format!("{request}, reply{context}"),
                DatabaseContext::Detached,
            );
            return self.failure(&format!(
                "dever_runtime::http::serve_live{suffix}(move |request, reply| {{ {capture} async move {{ {invoke}{wait}.map_err(|error| error.to_string()) }} }}, a0, {limits}, {live_limits}{tls_argument}).await",
            ), result.span);
        }
        let response =
            self.network_value("response", &function.outputs[0].ty, Direction::ToRuntime);
        let limits = self.network_value("a1", &arguments[1].ty, Direction::ToRuntime);
        let invoke = self.invoke(
            &target,
            format!("{request}{context}"),
            DatabaseContext::Detached,
        );
        self.failure(&format!(
            "dever_runtime::http::serve{suffix}(move |request| {{ {capture} async move {{ let response = {invoke}{wait}.map_err(|error| error.to_string())?; Ok({response}) }} }}, a0, {limits}{tls_argument}).await",
        ), result.span)
    }

    pub(super) fn http_send(&self, arguments: &[Expression], result: &Expression) -> String {
        let request = self.network_value("a2", &arguments[2].ty, Direction::ToRuntime);
        let limits = self.network_value("a3", &arguments[3].ty, Direction::ToRuntime);
        self.http_result(
            &format!("dever_runtime::http::send(&a0, a1, {request}, {limits}).await"),
            result,
        )
    }

    fn http_result(&self, call: &str, result: &Expression) -> String {
        let Type::Named(id) = &result.ty else {
            unreachable!("checked HTTP result")
        };
        let Shape::Choice(variants) = &self.program.types[*id].shape else {
            unreachable!()
        };
        let read = variants
            .iter()
            .find(|variant| variant.name == "Read")
            .expect("checked Read variant");
        let response = self.network_value("response", &read.fields[0].ty, Direction::ToLanguage);
        self.resource_result(
            &format!("{call}.map(|response| {response})"),
            &result.ty,
            "Read",
            false,
        )
    }

    // 校验器固定报文记录的字段契约；桥接按字段名生成，源码重排字段不会错位。
    fn network_value(&self, value: &str, ty: &Type, direction: Direction) -> String {
        match ty {
            Type::Named(id) => {
                let definition = &self.program.types[*id];
                let Shape::Record(fields) = &definition.shape else {
                    let Shape::Choice(variants) = &definition.shape else {
                        unreachable!()
                    };
                    let arms = variants
                        .iter()
                        .enumerate()
                        .map(|(index, variant)| {
                            let runtime = format!(
                                "dever_runtime::{}::{}",
                                definition
                                    .name
                                    .strip_prefix("dever.")
                                    .unwrap()
                                    .replace('.', "::"),
                                variant.name
                            );
                            let language = format!("T{id}::V{index}");
                            let payload =
                                self.network_value("payload", &variant.fields[0].ty, direction);
                            let (source, target) = match direction {
                                Direction::ToRuntime => (language, runtime),
                                Direction::ToLanguage => (runtime, language),
                            };
                            format!("{source}(payload) => {target}({payload})")
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    return format!("match {value} {{ {arms} }}");
                };
                let name = match direction {
                    Direction::ToRuntime => format!(
                        "dever_runtime::{}",
                        definition
                            .name
                            .strip_prefix("dever.")
                            .expect("official record")
                            .replace('.', "::"),
                    ),
                    Direction::ToLanguage => format!("T{id}"),
                };
                let fields = fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| {
                        let (source, target) = match direction {
                            Direction::ToRuntime => {
                                (format!("record.f{index}"), field.name.clone())
                            }
                            Direction::ToLanguage => {
                                (format!("record.{}", field.name), format!("f{index}"))
                            }
                        };
                        format!(
                            "{target}: {}",
                            self.network_value(&source, &field.ty, direction)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{ let record = {value}; {name} {{ {fields} }} }}")
            }
            Type::List(element) => {
                let element = self.network_value("header", element, direction);
                match direction {
                    Direction::ToRuntime => {
                        format!("{value}.into_values().map(|header| {element}).collect()")
                    }
                    Direction::ToLanguage => {
                        format!("List::new({value}.into_iter().map(|header| {element}).collect())")
                    }
                }
            }
            Type::Nullable(element) => {
                let element = self.network_value("element", element, direction);
                format!("{value}.map(|element| {element})")
            }
            Type::AsyncStream(event) => {
                let conversion = match direction {
                    Direction::ToLanguage => self.resource_result("event", event, "Chunk", false),
                    Direction::ToRuntime => {
                        let Type::Named(id) = event.as_ref() else {
                            unreachable!()
                        };
                        let Shape::Choice(variants) = &self.program.types[*id].shape else {
                            unreachable!()
                        };
                        let arms = variants
                            .iter()
                            .enumerate()
                            .map(|(index, variant)| {
                                let constructor =
                                    if variant.name == "Chunk" { "Ok" } else { "Err" };
                                format!("T{id}::V{index}(payload) => {constructor}(payload)")
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("match event {{ {arms} }}")
                    }
                };
                format!("{value}.map(|event| {conversion})")
            }
            Type::Text
            | Type::Bytes
            | Type::Int
            | Type::ClientTls
            | Type::ServerTls
            | Type::HttpClient => value.into(),
            _ => unreachable!("checked HTTP field type"),
        }
    }
}
