use std::io::{self, Read, Write};

use serde_json::{Value, json};

const MAX_BYTES: usize = 16 * 1024 * 1024;

fn main() {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--dever-component"] {
        std::process::exit(2);
    }
    if let Err(error) = serve() {
        eprintln!("component fixture: {error}");
        std::process::exit(1);
    }
}

fn serve() -> Result<(), String> {
    let hello = read_json()?;
    require_kind(&hello, "hello")?;
    let mode = hello
        .pointer("/setting/mode")
        .and_then(Value::as_str)
        .unwrap_or("normal");
    if mode == "delayed_start" {
        mark(&hello, "starting")?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let ready = json!({
        "kind": "ready",
        "version": field(&hello, "version")?,
        "port": field(&hello, "port")?,
        "schema": field(&hello, "schema")?,
        "adapter": field(&hello, "adapter")?,
        "capabilities": field(&hello, "capabilities")?,
        "operations": field(&hello, "operations")?,
    });
    match mode {
        "duplicate_ready" => write_raw(r#"{"kind":"ready","kind":"ready"}"#)?,
        "capability_mismatch" => {
            let mut mismatched = ready;
            mismatched["capabilities"] = json!(["network", "process"]);
            write_json(&mismatched)?;
        }
        _ => write_json(&ready)?,
    }
    if matches!(mode, "duplicate_ready" | "capability_mismatch") {
        return Ok(());
    }

    let health = read_json()?;
    require_kind(&health, "health")?;
    write_json(&health)?;
    eprintln!("fixture ready");

    loop {
        let message = read_json()?;
        match string_field(&message, "kind")? {
            "call" => handle_call(&message, mode, &hello)?,
            "shutdown" => {
                if mode == "delayed_shutdown" {
                    mark(&hello, "stopping")?;
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                write_json(&json!({"kind": "shutdown"}))?;
                if mode != "embedded" {
                    mark(&hello, "shutdown")?;
                }
                return Ok(());
            }
            kind => return Err(format!("unexpected {kind} message")),
        }
    }
}

fn mark(hello: &Value, extension: &str) -> Result<(), String> {
    let Some(marker) = hello.pointer("/setting/marker").and_then(Value::as_str) else {
        return Ok(());
    };
    let path = std::path::Path::new(marker).with_extension(extension);
    std::fs::write(path, b"ready").map_err(io_error)
}

fn handle_call(call: &Value, mode: &str, hello: &Value) -> Result<(), String> {
    let id = field(call, "id")?;
    // Deliberately malformed business payloads exercise the caller's typed boundary.
    let payload = match mode {
        "missing_output" => Some(json!({})),
        "extra_output" => Some(json!({"value": 7, "unknown": true})),
        "wrong_output" => Some(json!({"value": "invalid number"})),
        _ => None,
    };
    if let Some(payload) = payload {
        return write_json(&json!({"kind": "result", "id": id, "payload": payload}));
    }
    if matches!(mode, "declared_error" | "invalid_error" | "missing_error") {
        let identity = hello
            .pointer("/setting/identity")
            .ok_or("missing error identity")?;
        let payload = match mode {
            "invalid_error" => json!({"reason": 7}),
            "missing_error" => json!({}),
            _ => json!({"reason": "fixture"}),
        };
        return write_json(
            &json!({"kind": "error", "id": id, "error": identity, "payload": payload}),
        );
    }
    match string_field(call, "operation")? {
        "echo" => write_json(&json!({
            "kind": "result",
            "id": id,
            "payload": field(call, "payload")?,
        })),
        "read" => write_json(&json!({
            "kind": "result",
            "id": id,
            "payload": {"value": 7},
        })),
        "fail" => write_json(&json!({
            "kind": "error",
            "id": id,
            "error": "notification.delivery.Rejected",
            "payload": {"reason": "fixture"},
        })),
        "wait" => {
            let cancel = read_json()?;
            require_kind(&cancel, "cancel")?;
            if field(&cancel, "id")? != id {
                return Err("cancel id does not match call".into());
            }
            write_json(&json!({
                "kind": "error",
                "id": id,
                "error": "notification.delivery.Cancelled",
                "payload": {},
            }))
        }
        "timeout" => {
            let cancel = read_json()?;
            require_kind(&cancel, "cancel")?;
            if field(&cancel, "id")? != id {
                return Err("cancel id does not match timed out call".into());
            }
            write_json(&json!({"kind": "result", "id": id, "payload": {}}))
        }
        "crash" => std::process::exit(7),
        "duplicate" => write_raw(&format!(
            r#"{{"kind":"result","id":{id},"id":{id},"payload":{{}}}}"#,
            id = id
        )),
        "out_of_order" => {
            let id = id.as_u64().ok_or_else(|| "id is not u64".to_owned())?;
            write_json(&json!({"kind": "result", "id": id + 1, "payload": {}}))
        }
        "unknown" => write_json(&json!({"kind": "surprise", "id": id})),
        "oversize" => {
            let mut stdout = io::stdout().lock();
            stdout
                .write_all(&((MAX_BYTES as u32) + 1).to_be_bytes())
                .map_err(io_error)?;
            stdout.flush().map_err(io_error)?;
            std::process::exit(0);
        }
        "truncated" => {
            let mut stdout = io::stdout().lock();
            stdout.write_all(&32_u32.to_be_bytes()).map_err(io_error)?;
            stdout.write_all(b"{").map_err(io_error)?;
            stdout.flush().map_err(io_error)?;
            std::process::exit(0);
        }
        operation => Err(format!("unknown operation {operation}")),
    }
}

fn read_json() -> Result<Value, String> {
    let text = read_frame()?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn read_frame() -> Result<String, String> {
    let mut stdin = io::stdin().lock();
    let mut length = [0_u8; 4];
    stdin.read_exact(&mut length).map_err(io_error)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_BYTES {
        return Err("invalid frame length".into());
    }
    let mut body = vec![0_u8; length];
    stdin.read_exact(&mut body).map_err(io_error)?;
    String::from_utf8(body).map_err(|error| error.to_string())
}

fn write_json(value: &Value) -> Result<(), String> {
    write_raw(&serde_json::to_string(value).map_err(|error| error.to_string())?)
}

fn write_raw(body: &str) -> Result<(), String> {
    let length = u32::try_from(body.len()).map_err(|_| "fixture frame is too large")?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&length.to_be_bytes()).map_err(io_error)?;
    stdout.write_all(body.as_bytes()).map_err(io_error)?;
    stdout.flush().map_err(io_error)
}

fn require_kind(message: &Value, expected: &str) -> Result<(), String> {
    if string_field(message, "kind")? == expected {
        Ok(())
    } else {
        Err(format!("expected {expected} message"))
    }
}

fn string_field<'a>(message: &'a Value, name: &str) -> Result<&'a str, String> {
    field(message, name)?
        .as_str()
        .ok_or_else(|| format!("{name} is not a string"))
}

fn field<'a>(message: &'a Value, name: &str) -> Result<&'a Value, String> {
    message.get(name).ok_or_else(|| format!("missing {name}"))
}

fn io_error(error: io::Error) -> String {
    error.to_string()
}
