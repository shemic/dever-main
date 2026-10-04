use crate::bytes::Bytes;
use crate::http::{Header, HttpReply};

#[derive(Clone, Debug)]
pub struct Event {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
    pub retry_ms: Option<i64>,
}

pub async fn start(reply: &HttpReply, mut headers: Vec<Header>) -> Result<(), String> {
    if reply.chunk_limit() < 3 {
        return Err("SSE chunk_bytes must fit a three-byte heartbeat".into());
    }
    if headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("content-type"))
    {
        return Err("SSE owns the Content-Type header".into());
    }
    headers.push(Header {
        name: "content-type".into(),
        value: Bytes::from_string("text/event-stream; charset=utf-8".into()),
    });
    if !headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("cache-control"))
    {
        headers.push(Header {
            name: "cache-control".into(),
            value: Bytes::from_string("no-cache".into()),
        });
    }
    crate::http::reply::start_stream(reply, 200, headers, true).await
}

pub async fn send(reply: &HttpReply, event: Event) -> Result<(), String> {
    let bytes = encode(event, reply.chunk_limit())?;
    crate::http::reply::write_chunk(reply, bytes, true).await
}

// 按 EventSource 的行语义编码；先计算展开长度，避免超过单块上限后才分配。
pub fn encode(event: Event, limit: usize) -> Result<Bytes, String> {
    if event.event.contains(['\r', '\n']) {
        return Err("SSE event cannot contain a line break".into());
    }
    if event
        .id
        .as_ref()
        .is_some_and(|id| id.contains(['\r', '\n', '\0']))
    {
        return Err("SSE id cannot contain a line break or NUL".into());
    }
    if event.retry_ms.is_some_and(|retry| retry < 0) {
        return Err("SSE retry_ms must be nonnegative".into());
    }
    let retry = event.retry_ms.map(|retry| retry.to_string());
    // split_terminator 丢弃末尾空行，会改变 data 尾部换行；这里保留每一行。
    let newlines = event.data.bytes().filter(|byte| *byte == b'\n').count();
    let lines = || {
        event
            .data
            .split('\n')
            .enumerate()
            .flat_map(|(index, line)| {
                let line = if index < newlines {
                    line.strip_suffix('\r').unwrap_or(line)
                } else {
                    line
                };
                line.split('\r')
            })
    };
    let length = 1
        + lines().map(|line| 7 + line.len()).sum::<usize>()
        + if event.event.is_empty() {
            0
        } else {
            8 + event.event.len()
        }
        + event.id.as_ref().map_or(0, |id| 5 + id.len())
        + retry.as_ref().map_or(0, |retry| 8 + retry.len());
    if length > limit {
        return Err("SSE event exceeds chunk_bytes".into());
    }
    let mut encoded = String::with_capacity(length);
    let mut field = |name: &str, value: &str| {
        encoded.push_str(name);
        encoded.push_str(": ");
        encoded.push_str(value);
        encoded.push('\n');
    };
    if !event.event.is_empty() {
        field("event", &event.event);
    }
    if let Some(id) = event.id {
        field("id", &id);
    }
    if let Some(retry) = retry {
        field("retry", &retry);
    }
    for line in lines() {
        field("data", line);
    }
    encoded.push('\n');
    Ok(Bytes::from_string(encoded))
}
