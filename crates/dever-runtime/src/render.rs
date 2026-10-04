use std::fmt::{Display, Write};

use crate::Id;
use crate::bytes::Bytes;
use crate::collections::{List, Map, MapEntry};
use crate::number::DecimalValue;
use crate::resource::{Resource, Stream};

pub trait Render {
    fn render_to(&self, output: &mut String);

    fn render(&self) -> String {
        let mut output = String::new();
        self.render_to(&mut output);
        output
    }
}

fn write_display(output: &mut String, value: impl Display) {
    write!(output, "{value}").expect("writing to a String cannot fail");
}

impl Render for String {
    fn render_to(&self, output: &mut String) {
        output.push_str(self);
    }
}
impl Render for bool {
    fn render_to(&self, output: &mut String) {
        write_display(output, self);
    }
}
impl Render for i64 {
    fn render_to(&self, output: &mut String) {
        write_display(output, self);
    }
}
impl Render for f64 {
    fn render_to(&self, output: &mut String) {
        write_display(output, self);
    }
}
impl Render for DecimalValue {
    fn render_to(&self, output: &mut String) {
        write_display(output, self);
    }
}

impl Render for crate::orm::Uuid {
    fn render_to(&self, output: &mut String) {
        use std::fmt::Write as _;
        write!(output, "{self}").expect("string formatting");
    }
}
impl Render for Id {
    fn render_to(&self, output: &mut String) {
        output.push_str(&self.0);
    }
}
impl Render for Bytes {
    fn render_to(&self, output: &mut String) {
        write!(output, "{:?}", self.values()).expect("writing to a String cannot fail");
    }
}
impl<T> Render for Resource<T> {
    fn render_to(&self, output: &mut String) {
        output.push_str("<resource>");
    }
}
impl<T> Render for Stream<T> {
    fn render_to(&self, output: &mut String) {
        output.push_str("<stream>");
    }
}
impl<T> Render for crate::async_stream::AsyncStream<T> {
    fn render_to(&self, output: &mut String) {
        output.push_str("<async-stream>");
    }
}
impl Render for crate::net::Socket {
    fn render_to(&self, output: &mut String) {
        output.push_str("<resource>");
    }
}
impl Render for crate::net::Listener {
    fn render_to(&self, output: &mut String) {
        output.push_str("<resource>");
    }
}

impl Render for crate::http::HttpReply {
    fn render_to(&self, output: &mut String) {
        output.push_str("HttpReply(<resource>)");
    }
}
impl Render for crate::websocket::WebSocket {
    fn render_to(&self, output: &mut String) {
        output.push_str("WebSocket(<resource>)");
    }
}

impl Render for crate::tls::ClientTls {
    fn render_to(&self, output: &mut String) {
        output.push_str("ClientTls(<resource>)");
    }
}
impl Render for crate::tls::ServerTls {
    fn render_to(&self, output: &mut String) {
        output.push_str("ServerTls(<resource>)");
    }
}
impl Render for crate::http::HttpClient {
    fn render_to(&self, output: &mut String) {
        output.push_str("HttpClient(<resource>)");
    }
}

impl<T> Render for crate::channel::Channel<T> {
    fn render_to(&self, output: &mut String) {
        output.push_str("Channel(<resource>)");
    }
}
impl<T: Render> Render for Option<T> {
    fn render_to(&self, output: &mut String) {
        match self {
            Some(value) => value.render_to(output),
            None => output.push_str("null"),
        }
    }
}
impl<T: Render> Render for List<T> {
    fn render_to(&self, output: &mut String) {
        output.push('[');
        for (index, value) in self.values().iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            value.render_to(output);
        }
        output.push(']');
    }
}
impl<K: Render, V: Render> Render for Map<K, V> {
    fn render_to(&self, output: &mut String) {
        output.push('{');
        for (index, (key, value)) in self.pairs().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            key.render_to(output);
            output.push_str(" = ");
            value.render_to(output);
        }
        output.push('}');
    }
}
impl<K: Render, V: Render> Render for MapEntry<K, V> {
    fn render_to(&self, output: &mut String) {
        output.push_str("{key = ");
        self.key.render_to(output);
        output.push_str(", value = ");
        self.value.render_to(output);
        output.push('}');
    }
}
