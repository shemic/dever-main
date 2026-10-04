#[path = "support/temp.rs"]
mod temp;

use bytes::Bytes;
use dever_runtime::api::upload;
use dever_runtime::config::UploadLimits;
use http_body_util::StreamBody;
use hyper::body::Frame;
use std::convert::Infallible;

fn body(
    bytes: &[u8],
    chunk: usize,
) -> impl hyper::body::Body<Data = Bytes, Error = Infallible> + Unpin {
    StreamBody::new(futures_util::stream::iter(
        bytes
            .chunks(chunk)
            .map(|chunk| Ok(Frame::data(Bytes::copy_from_slice(chunk))))
            .collect::<Vec<Result<Frame<Bytes>, Infallible>>>(),
    ))
}

fn multipart(filename: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = format!("--boundary\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\nhello\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--boundary--\r\n");
    body
}

#[tokio::test]
async fn streaming_split_boundaries_preserve_bytes_and_remove_owned_files() {
    let directory = temp::TemporaryDirectory::new();
    let mut payload = vec![b'x'; 170_001];
    payload.extend_from_slice(b"\r\n--boundaryX is file content\r\n--boundary--X\r\n--boundary-");
    for chunk in [1, 7, 16_384, 250_000] {
        let bytes = multipart("photo.png", &payload);
        let mut parts = upload::decode(
            body(&bytes, chunk),
            "multipart/form-data; boundary=boundary",
            directory.path(),
            UploadLimits::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            parts
                .json("caption", dever_runtime::api::QueryValue::Text)
                .unwrap(),
            "\"hello\""
        );
        let file = parts.take("file").unwrap();
        assert_eq!(file.filename(), "photo.png");
        assert_eq!(file.content_type(), "image/png");
        assert_eq!(file.size(), payload.len() as i64);
        let files = std::fs::read_dir(directory.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read(files[0].path()).unwrap(), payload);
        parts.finish().unwrap();
        file.close();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn malformed_limits_and_duplicate_fields_cleanup_partial_files() {
    let directory = temp::TemporaryDirectory::new();
    for bytes in [
        multipart("../escape", b"hello"),
        multipart("x", b"hello")[..150].to_vec(),
        multipart("x", b"hello").repeat(2),
    ] {
        assert!(
            upload::decode(
                body(&bytes, 3),
                "multipart/form-data; boundary=boundary",
                directory.path(),
                UploadLimits::default()
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
    let bytes = multipart("x", &[0; 40_000]);
    for limits in [
        UploadLimits {
            file_bytes: 100,
            ..Default::default()
        },
        UploadLimits {
            total_bytes: 256,
            file_bytes: 100,
            ..Default::default()
        },
        UploadLimits {
            header_bytes: 10,
            ..Default::default()
        },
    ] {
        assert!(
            upload::decode(
                body(&bytes, 1024),
                "multipart/form-data; boundary=boundary",
                directory.path(),
                limits
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
    let duplicate = b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"first\"\r\n\r\nfirst\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"second\"\r\n\r\nsecond\r\n--boundary--\r\n";
    assert!(
        upload::decode(
            body(duplicate, 1),
            "multipart/form-data; boundary=boundary",
            directory.path(),
            UploadLimits::default()
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn cancellation_drops_partial_upload_owner() {
    let directory = temp::TemporaryDirectory::new();
    let bytes = multipart("x", &[0; 40_000]);
    let frames = futures_util::stream::iter(vec![Ok::<_, Infallible>(Frame::data(
        Bytes::copy_from_slice(&bytes[..30_000]),
    ))]);
    let stalled = futures_util::StreamExt::chain(frames, futures_util::stream::pending());
    let mut decoding = Box::pin(upload::decode(
        StreamBody::new(stalled),
        "multipart/form-data; boundary=boundary",
        directory.path(),
        UploadLimits::default(),
    ));
    let writing = async {
        for _ in 0..100 {
            if std::fs::read_dir(directory.path())
                .unwrap()
                .any(|file| file.unwrap().metadata().unwrap().len() > 1000)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("decoder did not spool the initial body frames");
    };
    tokio::select! {
        _ = writing => {},
        _ = &mut decoding => panic!("incomplete decoder unexpectedly finished"),
    }
    drop(decoding);
    for _ in 0..100 {
        if std::fs::read_dir(directory.path()).unwrap().count() == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("cancelled upload retained a temporary file");
}

fn sources(app: &str, route: &str) -> dever_core::source::SourceMap {
    let mut sources = dever_core::source::SourceMap::default();
    sources.add("media/image/app.dever", app);
    sources.add("media/image/api.dever", route);
    sources
}

fn storage_sources() -> dever_core::source::SourceMap {
    let mut sources = sources(
        "accept(file: Upload) (key: Uuid) { key = port.store(file) }",
        "post upload = app.accept",
    );
    sources.add(
        "media/image/port.dever",
        "store(file: Upload) (key: Uuid) fails dever.storage.PutResult",
    );
    sources.add(
        "media/image/adapter.dever",
        "port.store(file: Upload) (key: Uuid) { key = dever.storage.put(file) }",
    );
    sources
}

#[test]
fn upload_affinity_and_route_contract_are_checked() {
    for (app, route) in [
        (
            "accept(file: Upload) (size: Int) { size = dever.api.upload_size(file) }",
            "post upload = app.accept",
        ),
        (
            "accept(file: Upload) (size: Int) { dever.api.close_upload(file)\nsize = dever.api.upload_size(file) }",
            "post upload = app.accept",
        ),
        (
            "accept(file: Upload) (size: Int) { size = dever.api.upload_size(file)\ndever.api.close_upload(file) }",
            "get upload = app.accept",
        ),
        (
            "accept(file: Upload) (size: Int) { size = dever.api.upload_size(file)\ndever.api.close_upload(file) }",
            "cmd upload = app.accept",
        ),
        (
            "type Payload { file: Upload }\naccept(file: Payload) (size: Int) { size = 1 }",
            "post upload = app.accept",
        ),
        (
            "accept(file: Upload) (key: Uuid) { key = dever.storage.put(file) }",
            "post upload = app.accept",
        ),
        (
            "accept(file: Upload) (size: Int) { size = helper(file) }\nhelper(file: Upload) (size: Int) { size = dever.api.upload_size(file)\ndever.api.close_upload(file) }",
            "post upload = app.accept",
        ),
    ] {
        assert!(
            dever_core::check(&sources(app, route)).is_err(),
            "unexpected accepted: {app}"
        );
    }
    let sources = sources(
        "accept(file: Upload, caption: Text) (size: Int) { size = dever.api.upload_size(file)\nname = dever.api.upload_filename(file)\ndever.api.close_upload(file) }",
        "post upload = app.accept",
    );
    dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let sources = storage_sources();
    dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
}

#[test]
fn upload_route_native_compiles_with_owned_resource() {
    use std::io::{Read, Write};
    let sources = storage_sources();
    let program = dever_core::check(&sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(&sources))
                .collect::<String>()
        )
    });
    let native = dever_core::native::compile_project(
        &program,
        &sources,
        std::ffi::OsStr::new("/root/.cargo/bin/rustc"),
        dever_runtime::config::RuntimeProfile::default(),
    )
    .unwrap();
    assert!(native.executable().is_file());
    let directory = temp::TemporaryDirectory::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    std::fs::create_dir(directory.path().join("config")).unwrap();
    std::fs::write(
        directory.path().join("config/setting.json"),
        format!(r#"{{"http":{{"host":"127.0.0.1","port":{port},"timeout_ms":1000}}}}"#),
    )
    .unwrap();
    let executable = directory.path().join("upload-test");
    native.save(&executable).unwrap();
    let mut child = Child(
        std::process::Command::new(executable)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let started = std::time::Instant::now();
    let mut peer = loop {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "upload listener startup timeout"
        );
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("upload application exited: {status}");
        }
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(peer) => break peer,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    };
    peer.set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let bytes = b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"hello.txt\"\r\n\r\nhello\r\n--boundary--\r\n";
    write!(peer, "POST /media/image/upload HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=boundary\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
    peer.write_all(bytes).unwrap();
    let mut response = String::new();
    peer.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let envelope: serde_json::Value =
        serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    let key = envelope["data"].as_str().unwrap();
    assert!(dever_runtime::orm::Uuid::parse(key).is_ok());
    assert_eq!(
        std::fs::read(directory.path().join("data/upload").join(key)).unwrap(),
        b"hello"
    );
    assert_eq!(
        std::fs::read_dir(directory.path().join("data/tmp"))
            .unwrap()
            .count(),
        0
    );
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
