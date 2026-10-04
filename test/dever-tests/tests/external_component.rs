use std::fs;
use std::path::PathBuf;

use dever_runtime::component::{self, Definition, Reply};
use dever_runtime::wire::{Encoded, Encoder};
use sha2::{Digest, Sha256};

#[path = "support/sandbox.rs"]
mod sandbox;
#[path = "support/temp.rs"]
mod temp;

const OPERATIONS: &[&str] = &[
    "echo",
    "fail",
    "wait",
    "timeout",
    "crash",
    "duplicate",
    "out_of_order",
    "unknown",
    "oversize",
    "truncated",
];

#[test]
fn component_protocol_and_lifecycle_are_bounded() {
    let fixture = install_fixture();
    let result = dever_runtime::task::run_entry(async {
        fixture.prepare()?;
        // One invocation keeps the same deployment grants even if settings are
        // replaced between Worker startup, cancellation and restart.
        fs::write(
            fixture.directory.path().join("config/setting.json"),
            "invalid",
        )
        .map_err(|error| error.to_string())?;
        normal_lifecycle(fixture.entry)
            .await
            .map_err(|error| format!("normal lifecycle: {error}"))?;
        shutdown_with_queued_calls_is_bounded(fixture.entry)
            .await
            .map_err(|error| format!("queued shutdown: {error}"))?;
        rejected_handshakes(fixture.entry)
            .await
            .map_err(|error| format!("handshake rejection: {error}"))?;
        malformed_responses(fixture.entry)
            .await
            .map_err(|error| format!("malformed response: {error}"))?;
        cancelled_lifecycle(&fixture)
            .await
            .map_err(|error| format!("cancelled lifecycle: {error}"))
    });
    result.expect("external component protocol fixture");
    assert!(fixture.path.with_extension("shutdown").is_file());
    assert!(dever_runtime::task::run_entry(async { fixture.prepare() }).is_err());
}

async fn cancelled_lifecycle(fixture: &InstalledFixture) -> Result<(), String> {
    let entry = fixture.entry;
    let start = dever_runtime::task::run(async move {
        component::start(definition("cancel_start", entry, "delayed_start", 2_000)).await
    })
    .await?;
    wait_for_marker(&fixture.path.with_extension("starting")).await?;
    dever_runtime::task::stop(start).await?;
    component::shutdown().await?;

    component::start(definition(
        "cancel_shutdown",
        entry,
        "delayed_shutdown",
        2_000,
    ))
    .await?;
    let stopping = dever_runtime::task::run(component::shutdown()).await?;
    wait_for_marker(&fixture.path.with_extension("stopping")).await?;
    dever_runtime::task::stop(stopping).await?;
    // The cancelled caller must leave ownership available for a complete second drain.
    component::shutdown().await?;
    Ok(())
}

async fn wait_for_marker(path: &std::path::Path) -> Result<(), String> {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !path.is_file() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .map_err(|_| "fixture did not reach its lifecycle checkpoint".to_owned())
}

async fn shutdown_with_queued_calls_is_bounded(entry: &'static str) -> Result<(), String> {
    component::start(definition("queued_shutdown", entry, "normal", 5_000)).await?;
    let mut calls = Vec::new();
    for _ in 0..80 {
        calls.push(
            dever_runtime::task::run(async {
                component::call("queued_shutdown", "wait", &payload(0))
                    .await
                    .map(|_| ())
            })
            .await?,
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    tokio::time::timeout(std::time::Duration::from_secs(1), component::shutdown())
        .await
        .map_err(|_| "external Adapter shutdown did not stop queued calls".to_owned())??;
    for call in calls {
        let error = dever_runtime::task::wait(call)
            .await
            .expect_err("shutdown must stop every pending call");
        assert!(
            error.contains("stopped"),
            "unexpected shutdown error: {error}"
        );
    }
    Ok(())
}

async fn normal_lifecycle(entry: &'static str) -> Result<(), String> {
    // A short-lived caller starts the shared Worker; ending its Scope must not stop it.
    let started = dever_runtime::task::run(async move {
        component::start(definition("normal", entry, "normal", 50)).await
    })
    .await?;
    dever_runtime::task::wait(started)
        .await
        .map_err(|error| format!("start: {error}"))?;

    for value in [1_i64, 2] {
        match component::call("normal", "echo", &payload(value))
            .await
            .map_err(|error| format!("echo {value}: {error}"))?
        {
            Reply::Result(body) => assert_eq!(body, format!(r#"{{"value":{value}}}"#)),
            Reply::Error { .. } => return Err("echo returned a business error".into()),
        }
    }
    match component::call("normal", "fail", &payload(0))
        .await
        .map_err(|error| format!("business error: {error}"))?
    {
        Reply::Error { identity, payload } => {
            assert_eq!(identity, "notification.delivery.Rejected");
            assert_eq!(payload, r#"{"reason":"fixture"}"#);
        }
        Reply::Result(_) => return Err("fail returned a result".into()),
    }

    let wait = dever_runtime::task::run(async {
        component::call("normal", "wait", &payload(0))
            .await
            .map(|_| ())
    })
    .await?;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    dever_runtime::task::stop(wait)
        .await
        .map_err(|error| format!("cancel wait: {error}"))?;

    let timeout = component::call("normal", "timeout", &payload(0))
        .await
        .expect_err("timeout operation must reach the deadline");
    assert_eq!(timeout, "external Adapter call timed out");
    assert!(matches!(
        component::call("normal", "echo", &payload(3))
            .await
            .map_err(|error| format!("echo after timeout: {error}"))?,
        Reply::Result(_)
    ));

    let crash = component::call("normal", "crash", &payload(0))
        .await
        .expect_err("crashed worker must fail its pending call");
    assert!(
        crash.contains("early eof"),
        "unexpected crash fault: {crash}"
    );
    assert!(matches!(
        component::call("normal", "echo", &payload(4))
            .await
            .map_err(|error| format!("echo after crash: {error}"))?,
        Reply::Result(_)
    ));
    component::shutdown()
        .await
        .map_err(|error| format!("shutdown: {error}"))?;
    Ok(())
}

async fn rejected_handshakes(entry: &'static str) -> Result<(), String> {
    for (key, mode, expected) in [
        ("duplicate_ready", "duplicate_ready", "duplicate JSON field"),
        (
            "capability_mismatch",
            "capability_mismatch",
            "handshake does not match",
        ),
    ] {
        let error = component::start(definition(key, entry, mode, 100))
            .await
            .expect_err("invalid handshake must be rejected");
        assert!(
            error.contains(expected),
            "unexpected handshake error: {error}"
        );
    }
    Ok(())
}

async fn malformed_responses(entry: &'static str) -> Result<(), String> {
    for operation in [
        "duplicate",
        "out_of_order",
        "unknown",
        "oversize",
        "truncated",
    ] {
        component::start(definition(operation, entry, "normal", 100)).await?;
        let error = component::call(operation, operation, &payload(0))
            .await
            .expect_err("invalid response must fail its pending call");
        let expected = match operation {
            "duplicate" => "duplicate JSON field",
            "out_of_order" => "out-of-order request id",
            "unknown" => "unknown message kind",
            "oversize" => "frame exceeds byte limit",
            "truncated" => "cannot read complete component protocol frame",
            _ => unreachable!(),
        };
        assert!(
            error.contains(expected),
            "unexpected {operation} call fault: {error}"
        );
        let cleanup = component::shutdown()
            .await
            .expect_err("protocol fault must survive cleanup");
        assert!(
            cleanup.contains(expected),
            "unexpected {operation} fault: {cleanup}"
        );
    }
    Ok(())
}

fn definition(
    key: &'static str,
    entry: &'static str,
    mode: &'static str,
    timeout_ms: u64,
) -> Definition {
    Definition {
        ecosystem: "exec".into(),
        key: key.into(),
        entry: entry.into(),
        port: "notification.delivery".into(),
        adapter: "notification.delivery.fixture".into(),
        schema: "a19ca3e615ce5e71".into(),
        capabilities: vec!["network".into(), "file".into()],
        operations: OPERATIONS.iter().map(|name| (*name).into()).collect(),
        setting: Some(format!(
            r#"{{"mode":"{mode}","marker":"/data/lifecycle/worker"}}"#
        )),
        timeout_ms,
    }
}

fn payload(value: i64) -> Encoded {
    let mut writer = Encoder::default();
    writer.begin_object().expect("payload object");
    writer.key("value").expect("payload field");
    writer.int(value).expect("payload value");
    writer.end().expect("payload end");
    writer.finish().expect("payload JSON")
}

struct InstalledFixture {
    directory: temp::TemporaryDirectory,
    path: PathBuf,
    entry: &'static str,
    resources: Vec<dever_runtime::external::Resource<'static>>,
}

impl InstalledFixture {
    fn prepare(&self) -> Result<(), String> {
        let digest = dever_runtime::external::bundle_digest(&self.resources);
        dever_runtime::external::prepare(self.directory.path(), &digest, &self.resources)
    }
}

fn install_fixture() -> InstalledFixture {
    let directory = temp::TemporaryDirectory::new();
    fs::create_dir(directory.path().join("lifecycle")).unwrap();
    fs::create_dir(directory.path().join("config")).unwrap();
    fs::write(directory.path().join("config/setting.json"), r#"{"adapter":{"notification.delivery":{"files":{"lifecycle":{"path":"lifecycle","write":true}}}}}"#).unwrap();
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut assets = sandbox::assets(&workspace);
    assets.push(sandbox::Asset {
        path: "workers/fixture/worker".into(),
        bytes: fs::read(env!("CARGO_BIN_EXE_component-fixture")).unwrap(),
        executable: true,
    });
    assets.push(sandbox::Asset {
        path: "worker.dever-worker.json".into(),
        bytes: br#"{"format":"dever-worker-launch-v1","ecosystem":"exec","entry":"worker","executable":"workers/fixture/worker","arguments":[],"working_directory":"workers/fixture"}"#.to_vec(), executable: false,
    });
    let resources = assets
        .into_iter()
        .map(|asset| dever_runtime::external::Resource {
            path: Box::leak(asset.path.into_boxed_str()),
            sha256: Box::leak(format!("{:x}", Sha256::digest(&asset.bytes)).into_boxed_str()),
            bytes: Box::leak(asset.bytes.into_boxed_slice()),
            executable: asset.executable,
        })
        .collect();
    let path = directory.path().join("lifecycle/worker");
    InstalledFixture {
        directory,
        path,
        entry: "worker",
        resources,
    }
}
