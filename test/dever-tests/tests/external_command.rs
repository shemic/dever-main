use std::fs;
use std::path::Path;
use std::time::Duration;

use dever_runtime::bytes::Bytes;
use dever_runtime::component::{self, Definition, Reply};
use dever_runtime::external::{self, Resource};
use dever_runtime::wire::Encoder;
use serde_json::json;
use sha2::{Digest, Sha256};

#[path = "support/sandbox.rs"]
mod sandbox;
#[path = "support/temp.rs"]
mod temp;

const PORT: &str = "tools.command";

struct Fixture {
    directory: temp::TemporaryDirectory,
    resources: Vec<Resource<'static>>,
}

impl Fixture {
    fn new(timeout_ms: u64, output_limit: usize) -> Self {
        Self::with_program(
            timeout_ms,
            output_limit,
            fs::read(env!("CARGO_BIN_EXE_command-fixture")).unwrap(),
        )
    }

    fn with_program(timeout_ms: u64, output_limit: usize, program: Vec<u8>) -> Self {
        let directory = temp::TemporaryDirectory::new();
        fs::create_dir(directory.path().join("markers")).unwrap();
        fs::create_dir(directory.path().join("config")).unwrap();
        let settings = json!({"adapter": {PORT: {
            "files": {"markers": {"path":"markers", "write":true}},
            "command": {"timeout_ms":timeout_ms, "output_limit":output_limit}
        }}});
        fs::write(
            directory.path().join("config/setting.json"),
            settings.to_string(),
        )
        .unwrap();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut assets = sandbox::assets(&workspace);
        assets.push(sandbox::Asset {
            path: "commands/fixture/tool".into(),
            bytes: program,
            executable: true,
        });
        assets.push(sandbox::Asset {
            path: "tool.dever-command.json".into(),
            bytes: json!({"format":"dever-command-launch-v1", "ecosystem":"command", "entry":"tool",
                "executable":"commands/fixture/tool", "arguments":[], "working_directory":"commands/fixture"})
                .to_string().into_bytes(),
            executable: false,
        });
        let resources = assets
            .into_iter()
            .map(|asset| Resource {
                path: Box::leak(asset.path.into_boxed_str()),
                sha256: Box::leak(format!("{:x}", Sha256::digest(&asset.bytes)).into_boxed_str()),
                bytes: Box::leak(asset.bytes.into_boxed_slice()),
                executable: asset.executable,
            })
            .collect();
        Self {
            directory,
            resources,
        }
    }

    async fn start(&self) -> Result<(), String> {
        external::prepare(
            self.directory.path(),
            &external::bundle_digest(&self.resources),
            &self.resources,
        )?;
        component::start(Definition {
            key: PORT.into(),
            ecosystem: "command".into(),
            entry: "tool".into(),
            port: PORT.into(),
            adapter: "default".into(),
            schema: "command-fixture-v1".into(),
            capabilities: vec!["file".into(), "process".into()],
            operations: vec!["execute".into()],
            setting: None,
            timeout_ms: 30_000,
        })
        .await
    }

    fn marker(&self, name: &str) -> std::path::PathBuf {
        self.directory.path().join("markers").join(name)
    }
}

struct Output {
    code: i64,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn invoke(args: &[&str], stdin: Option<&[u8]>) -> Result<Output, String> {
    let mut writer = Encoder::default();
    writer.begin_object()?;
    writer.key("args")?;
    writer.begin_array()?;
    for arg in args {
        writer.text(arg)?;
    }
    writer.end()?;
    if let Some(stdin) = stdin {
        writer.key("stdin")?;
        writer.bytes(&Bytes::new(stdin.to_vec()))?;
    }
    writer.end()?;
    let reply = component::call(PORT, "execute", &writer.finish()?).await?;
    let Reply::Result(payload) = reply else {
        return Err("unexpected business error".into());
    };
    let node = dever_runtime::wire::parse(&payload)?;
    let output = node
        .object()?
        .get("output")
        .ok_or("missing output")?
        .object()?;
    Ok(Output {
        code: output.get("code").ok_or("missing code")?.int()?,
        stdout: output
            .get("stdout")
            .ok_or("missing stdout")?
            .bytes()?
            .values()
            .to_vec(),
        stderr: output
            .get("stderr")
            .ok_or("missing stderr")?
            .bytes()?
            .values()
            .to_vec(),
    })
}

#[test]
fn ordinary_commands_preserve_argv_binary_io_and_nonzero_status() {
    let fixture = Fixture::new(5000, 1024 * 1024);
    dever_runtime::task::run_entry(async {
        fixture.start().await?;
        assert!(
            !fixture.marker("starts").exists(),
            "startup must not execute the tool"
        );
        let output = invoke(
            &[
                "echo",
                "a b",
                "\"quoted\"",
                "$(literal);*",
                "/data/markers",
                "",
            ],
            None,
        )
        .await?;
        assert_eq!(
            output.stdout,
            b"a b\0\"quoted\"\0$(literal);*\0/data/markers\0\0"
        );
        assert_eq!(output.code, 0);
        let binary: Vec<u8> = (0..=255).cycle().take(131_072).collect();
        let output = invoke(&["stdin"], Some(&binary)).await?;
        assert_eq!(output.stdout, binary);
        assert_eq!(output.stderr, [0, 255, 128, 10]);
        assert!(
            invoke(&["stdin"], None).await?.stdout.is_empty(),
            "omitted stdin must send EOF"
        );
        let output = invoke(&["duplex", "262144"], Some(&binary)).await?;
        assert_eq!(output.stdout, vec![254; 262_144]);
        assert_eq!(output.stderr, vec![253; 262_144]);
        assert_eq!(invoke(&["exit", "37"], None).await?.code, 37);
        assert_eq!(invoke(&["exit", "1"], None).await?.code, 1);
        assert_eq!(invoke(&["exit", "127"], None).await?.code, 127);
        assert!(
            invoke(&["fds"], None).await?.stdout.is_empty(),
            "observation descriptors leaked into the command"
        );
        // Early stdin close must not replace the program's ordinary result.
        assert_eq!(invoke(&["exit", "13"], Some(&binary)).await?.code, 13);
        let nul = invoke(&["echo", "bad\0arg"], None)
            .await
            .err()
            .ok_or("NUL accepted")?;
        assert!(nul.contains("NUL"));
        let huge = "a".repeat(65_536);
        assert!(invoke(&["echo", &huge], None).await.is_err());
        assert!(
            invoke(&["stdin"], Some(&vec![0; 8 * 1024 * 1024 + 1]))
                .await
                .err()
                .is_some_and(|error| error.contains("stdin exceeds"))
        );
        for _ in 0..5 {
            assert_eq!(invoke(&["exit", "0"], None).await?.code, 0);
        }
        component::shutdown().await
    })
    .unwrap();
}

#[test]
fn combined_output_limit_closes_process_then_allows_the_next_call() {
    let fixture = Fixture::new(5000, 262_144);
    dever_runtime::task::run_entry(async {
        fixture.start().await?;
        let error = invoke(&["duplex", "200000"], Some(&vec![7; 131_072]))
            .await
            .err()
            .ok_or("limit accepted")?;
        assert!(error.contains("output_limit"), "{error}");
        assert_eq!(invoke(&["exit", "0"], None).await?.code, 0);
        component::shutdown().await
    })
    .unwrap();
}

async fn heartbeat(fixture: &Fixture) -> Result<(), String> {
    // Verified-resource preparation precedes process startup; debug hashing is
    // not the command deadline and may dominate on a busy author machine.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if fs::read_to_string(fixture.marker("heartbeat"))
                .ok()
                .and_then(|text| text.parse::<u32>().ok())
                .is_some_and(|count| count >= 2)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| {
        format!(
            "descendant heartbeat did not start; starts={:?}; ready={}; error={:?}",
            fs::read_to_string(fixture.marker("starts")),
            fixture.marker("ready").exists(),
            fs::read_to_string(fixture.marker("fixture-error"))
        )
    })
}

#[test]
fn guard_exec_failure_is_a_fault_not_a_program_exit_code() {
    let fixture = Fixture::with_program(5000, 1024 * 1024, b"not an ELF executable\n".to_vec());
    dever_runtime::task::run_entry(async {
        fixture.start().await?;
        let error = invoke(&[], None)
            .await
            .err()
            .ok_or("exec failure was returned as an exit code")?;
        assert!(
            error.contains("startup failed") && error.contains("cannot execute"),
            "{error}"
        );
        component::shutdown().await
    })
    .unwrap();
}

async fn assert_reaped(fixture: &Fixture) {
    let lifetime = fs::OpenOptions::new()
        .write(true)
        .open(fixture.marker("lifetime.lock"))
        .unwrap();
    lifetime
        .try_lock()
        .expect("descendant still holds its lifetime lock after cleanup returned");
    let before = fs::read(fixture.marker("heartbeat")).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        fs::read(fixture.marker("heartbeat")).unwrap(),
        before,
        "descendant survived cleanup"
    );
    assert_eq!(
        fs::read_to_string(fixture.marker("starts")).unwrap(),
        "started\n",
        "command was replayed"
    );
}

#[test]
fn timeout_reaps_descendants_and_keeps_the_adapter_available() {
    let fixture = Fixture::new(800, 1024 * 1024);
    dever_runtime::task::run_entry(async {
        fixture.start().await?;
        let call = dever_runtime::task::run(async {
            invoke(&["tree", "/data/markers"], None).await.map(|_| ())
        })
        .await?;
        heartbeat(&fixture).await?;
        let error = dever_runtime::task::wait(call)
            .await
            .expect_err("command did not time out");
        assert!(error.contains("timed out"), "{error}");
        assert_reaped(&fixture).await;
        assert_eq!(invoke(&["exit", "0"], None).await?.code, 0);
        component::shutdown().await
    })
    .unwrap();
}

#[test]
fn cancellation_and_root_close_reap_descendants_without_replay() {
    for root_close in [false, true] {
        let fixture = Fixture::new(5000, 1024 * 1024);
        dever_runtime::task::run_entry(async {
            fixture.start().await?;
            let call = dever_runtime::task::run(async {
                invoke(&["tree", "/data/markers"], None).await.map(|_| ())
            })
            .await?;
            heartbeat(&fixture).await?;
            if root_close {
                // Return while the call is active: the invocation closes its
                // component owner before draining the structured child task.
                return Ok::<(), String>(());
            }
            dever_runtime::task::stop(call).await?;
            assert_reaped(&fixture).await;
            assert_eq!(invoke(&["exit", "0"], None).await?.code, 0);
            component::shutdown().await
        })
        .unwrap_or_else(|error| {
            assert!(root_close && error.contains("stopped"), "{error}");
        });
        let before = fs::read(fixture.marker("heartbeat")).unwrap();
        let lifetime = fs::OpenOptions::new()
            .write(true)
            .open(fixture.marker("lifetime.lock"))
            .unwrap();
        lifetime
            .try_lock()
            .expect("root close returned before descendant cleanup");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(fs::read(fixture.marker("heartbeat")).unwrap(), before);
        assert_eq!(
            fs::read_to_string(fixture.marker("starts")).unwrap(),
            "started\n"
        );
    }
}

#[test]
fn queued_cancellation_does_not_wait_for_an_unrelated_running_command() {
    let fixture = Fixture::new(5000, 1024 * 1024);
    dever_runtime::task::run_entry(async {
        fixture.start().await?;
        let active = dever_runtime::task::run(async {
            invoke(&["tree", "/data/markers"], None).await.map(|_| ())
        })
        .await?;
        heartbeat(&fixture).await?;
        let (entered, admitted) = tokio::sync::oneshot::channel();
        let queued = dever_runtime::task::run(async move {
            let _ = entered.send(());
            invoke(&["echo", "queued"], None).await.map(|_| ())
        })
        .await?;
        admitted.await.map_err(|error| error.to_string())?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::time::timeout(
            Duration::from_millis(500),
            dever_runtime::task::stop(queued),
        )
        .await
        .map_err(|_| "queued cancellation waited for the active command")??;
        let before = fs::read(fixture.marker("heartbeat")).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_ne!(fs::read(fixture.marker("heartbeat")).unwrap(), before);
        dever_runtime::task::stop(active).await?;
        assert_reaped(&fixture).await;
        component::shutdown().await
    })
    .unwrap();
}

#[test]
fn task_deadline_waits_for_command_process_cleanup() {
    let fixture = Fixture::new(5000, 1024 * 1024);
    dever_runtime::task::run_entry(async {
        fixture.start().await?;
        let call = dever_runtime::task::run(async {
            invoke(&["tree", "/data/markers"], None).await.map(|_| ())
        })
        .await?;
        heartbeat(&fixture).await?;
        assert_eq!(dever_runtime::task::wait_timeout(call, 10).await?, None);
        assert_reaped(&fixture).await;
        component::shutdown().await
    })
    .unwrap();
}

#[test]
fn explicit_shutdown_drains_active_and_queued_commands() {
    let fixture = Fixture::new(5000, 1024 * 1024);
    dever_runtime::task::run_entry(async {
        fixture.start().await?;
        let active = dever_runtime::task::run(async {
            invoke(&["tree", "/data/markers"], None).await.map(|_| ())
        })
        .await?;
        heartbeat(&fixture).await?;
        let queued =
            dever_runtime::task::run(async { invoke(&["echo", "queued"], None).await.map(|_| ()) })
                .await?;
        component::shutdown().await?;
        assert_reaped(&fixture).await;
        assert!(dever_runtime::task::wait(active).await.is_err());
        assert!(dever_runtime::task::wait(queued).await.is_err());
        Ok::<(), String>(())
    })
    .unwrap();
}

#[test]
fn command_settings_validate_limits_without_affecting_worker_selection() {
    let directory = temp::TemporaryDirectory::new();
    fs::create_dir(directory.path().join("config")).unwrap();
    for command in [
        json!({"timeout_ms":0}),
        json!({"timeout_ms":3_600_001}),
        json!({"output_limit":0}),
        json!({"output_limit":8_388_609}),
        json!({"unknown":1}),
    ] {
        fs::write(
            directory.path().join("config/setting.json"),
            json!({"adapter":{PORT:{"command":command}}}).to_string(),
        )
        .unwrap();
        let settings = dever_runtime::config::Settings::load_project(directory.path()).unwrap();
        assert!(settings.command_limits(PORT).is_err());
        assert!(settings.select_adapter(PORT, &["default"]).is_ok());
    }
}
