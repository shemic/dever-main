//! Actual managed compilation, using ephemeral signing keys and an owned daemon.
use super::*;
use dever_cli::toolchain::compilation::{CompileKind, CompileRequest};
use dever_cli::toolchain::{BuildTarget, CacheStore, compile};
use dever_core::source::SourceMap;
use dever_runtime::config::Settings;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

fn source_project() -> TemporaryDirectory {
    let root = project();
    fs::write(
        root.path().join("module/portal/value/app.dever"),
        "value() (result: Text) { result = \"two\" }\n",
    )
    .unwrap();
    root
}

fn request(root: &Path, kind: CompileKind) -> CompileRequest {
    let sources = SourceMap::load_with_packages(
        &root.join("module"),
        if kind == CompileKind::TestSuite {
            Some(root.join("test"))
        } else {
            None
        }
        .as_deref(),
        &[],
    )
    .unwrap();
    let bindings = (kind == CompileKind::Application)
        .then(|| Settings::load_project(root).unwrap().compilation_bindings());
    CompileRequest::new(
        "0.1.0".into(),
        kind,
        &sources,
        bindings,
        &[],
        BuildTarget::host().unwrap(),
    )
    .unwrap()
}

#[test]
fn compilation_snapshot_excludes_secrets_and_rejects_ambiguous_or_unsafe_inputs() {
    let root = source_project();
    fs::write(root.path().join("config/setting.json"), serde_json::to_vec(&serde_json::json!({
        "database":{"default":{"type":"sqlite","path":"private-database-marker.sqlite"}},
        "auth":{"providers":{"session":{"verify":"user.account.verify",
            "jwtSecret":"private-jwt-marker-long-enough-for-hmac-2026","ttlSeconds":3600,"cookie":"secret-cookie-name"}}},
        "sites":{"admin":{"path":"admin","auth":"session","origin":"https://private-origin.example"}}
    })).unwrap()).unwrap();
    let original = request(root.path(), CompileKind::Application);
    let encoded = original.encode().unwrap();
    let text = std::str::from_utf8(&encoded).unwrap();
    for secret in [
        "private-database-marker",
        "private-jwt-marker",
        "secret-cookie-name",
        "private-origin",
        root.path().to_str().unwrap(),
    ] {
        assert!(!text.contains(secret), "compile input leaked {secret}");
    }
    assert!(text.contains("user.account.verify"));
    assert!(text.contains("sqlite"));
    CompileRequest::decode(&encoded).unwrap();
    let duplicate = text.replace("\"databases\":", "\"databases\":{},\"databases\":");
    assert!(CompileRequest::decode(duplicate.as_bytes()).is_err());
    for path in ["/tmp/read-me.dever", "../main.dever", "portal/value/app.rs"] {
        let mut invalid = original.clone();
        invalid.sources[0].logical_path = path.into();
        assert!(invalid.encode().is_err(), "accepted source path {path}");
    }
    let mut reserved = original.clone();
    reserved.sources[0].logical_path = "dever/auth/app.dever".into();
    assert!(
        dever_core::check(&reserved.source_map().unwrap()).is_err(),
        "reserved standard namespace bypassed checking"
    );
    let mut invalid = original.clone();
    invalid.sources.push(invalid.sources[0].clone());
    assert!(invalid.encode().is_err());
    let mut invalid = original;
    invalid.sources[0].is_test = true;
    assert!(invalid.encode().is_err());
    let suite = request(root.path(), CompileKind::TestSuite);
    assert!(suite.bindings.is_none());
    assert!(
        suite
            .source_map()
            .unwrap()
            .files()
            .iter()
            .any(|source| source.is_test())
    );
}

#[test]
fn compilation_target_is_required_canonical_and_part_of_cache_identity() {
    let root = source_project();
    let mut input = request(root.path(), CompileKind::Application);
    input.target = BuildTarget::LinuxX86_64;
    let host = input.encode().unwrap();
    input.target = BuildTarget::LinuxAarch64;
    let arm = input.encode().unwrap();
    assert_ne!(host, arm);
    assert_eq!(
        CompileRequest::decode(&arm).unwrap().target,
        BuildTarget::LinuxAarch64
    );
    let text = std::str::from_utf8(&arm).unwrap();
    for invalid in [
        text.replace("\"target\":\"linux-aarch64\",", ""),
        text.replace("linux-aarch64", "aarch64-unknown-linux-gnu"),
        text.replace("\"target\":", "\"target\":\"linux-x86_64\",\"target\":"),
    ] {
        assert!(CompileRequest::decode(invalid.as_bytes()).is_err());
    }
    let mut suite = request(root.path(), CompileKind::TestSuite);
    suite.target = if BuildTarget::host().unwrap() == BuildTarget::LinuxX86_64 {
        BuildTarget::LinuxAarch64
    } else {
        BuildTarget::LinuxX86_64
    };
    assert!(suite.encode().unwrap_err().contains("host target"));
}

#[test]
#[ignore = "requires explicitly prepared native archive; starts an owned daemon"]
fn trusted_compilation_cache_requires_sources_and_revalidates_signed_pack() {
    let root = source_project();
    let (layout, _) = installed_dever(root.path());
    let _daemon = TestDaemon::start(&layout);
    let input = request(root.path(), CompileKind::Application);
    let started = Instant::now();
    let first = compile(&layout, &input).unwrap();
    let cold_seconds = started.elapsed().as_secs_f64();
    let initial_cache = cache_status(&layout).unwrap();
    assert_eq!(initial_cache.entries, 1);
    let started = Instant::now();
    assert_eq!(compile(&layout, &input).unwrap(), first);
    let hit_seconds = started.elapsed().as_secs_f64();
    assert_eq!(cache_status(&layout).unwrap().entries, 1);
    assert!(artifact_get(&layout, &digest(&first), first.len() as u64).is_err());
    let mut invalid = input.clone();
    invalid
        .sources
        .iter_mut()
        .find(|source| source.logical_path.ends_with("app.dever"))
        .unwrap()
        .text = "value() (result: Text) { result = missing_function() }".into();
    let error = compile(&layout, &invalid).unwrap_err();
    assert!(error.contains("missing_function"), "{error}");
    assert_eq!(cache_status(&layout).unwrap().entries, 1);
    assert_eq!(compile(&layout, &input).unwrap(), first);
    let mut changed = input.clone();
    changed
        .sources
        .iter_mut()
        .find(|source| source.logical_path.ends_with("app.dever"))
        .unwrap()
        .text = "value() (result: Text) { result = \"three\" }".into();
    // 两个独立客户端同时提交同一份新源码，必须收敛到一个已验证产物。
    let barrier = std::sync::Barrier::new(2);
    let concurrent = std::thread::scope(|scope| {
        let clients: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let started = Instant::now();
                    let bytes = compile(&layout, &changed).unwrap();
                    (bytes, started.elapsed().as_secs_f64())
                })
            })
            .collect();
        clients
            .into_iter()
            .map(|client| client.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(concurrent[0].0, concurrent[1].0);
    assert_ne!(concurrent[0].0, first);
    let shared_cache = cache_status(&layout).unwrap();
    assert_eq!(shared_cache.entries, 2);

    // The worker compiles the suite; only the caller runs its failing case.
    fs::write(
        root.path().join("test/portal/value/smoke.dever"),
        "smoke() () { assert_eq(1, 2) }",
    )
    .unwrap();
    let suite = compile(&layout, &request(root.path(), CompileKind::TestSuite)).unwrap();
    let program = dever_core::native::NativeProgram::from_verified_bytes(&suite).unwrap();
    assert!(
        !Command::new(program.executable())
            .arg("0")
            .env_clear()
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(cache_status(&layout).unwrap().entries, 3);

    // A valid signature over an incomplete list must not authorize unsigned
    // link inputs, even when the native manifest still names those inputs.
    let version_root = layout.versions().join("0.1.0");
    let trusted_key = fs::read(layout.state().join("trusted-release-key")).unwrap();
    let signed_manifest = fs::read(version_root.join("manifest.json")).unwrap();
    let signature = fs::read(version_root.join("manifest.sig")).unwrap();
    let mut manifest: ReleaseManifest = serde_json::from_slice(&signed_manifest).unwrap();
    manifest
        .artifacts
        .retain(|artifact| !artifact.path.ends_with("/crt1.o"));
    let key_bytes = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    let key = Ed25519KeyPair::from_pkcs8(key_bytes.as_ref()).unwrap();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(
        layout.state().join("trusted-release-key"),
        hex(key.public_key().as_ref()),
    )
    .unwrap();
    fs::write(version_root.join("manifest.json"), &bytes).unwrap();
    fs::write(
        version_root.join("manifest.sig"),
        hex(key.sign(&bytes).as_ref()),
    )
    .unwrap();
    assert!(
        compile(&layout, &input)
            .unwrap_err()
            .contains("not covered by the signed release")
    );
    fs::write(layout.state().join("trusted-release-key"), trusted_key).unwrap();
    fs::write(version_root.join("manifest.json"), signed_manifest).unwrap();
    fs::write(version_root.join("manifest.sig"), signature).unwrap();

    let pack = layout.versions().join("0.1.0/runtime/linux-x86_64/crt1.o");
    fs::write(pack, b"changed signed link input").unwrap();
    let error = compile(&layout, &input).unwrap_err();
    assert!(error.contains("integrity"), "{error}");
    assert_eq!(cache_status(&layout).unwrap().entries, 3);
    assert_eq!(
        fs::read_dir(layout.native_cache().join("staging"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(layout.native_cache().join("operations"))
            .unwrap()
            .count(),
        0
    );
    let report = serde_json::json!({
        "format": "dever-shared-compile-measurement-v1",
        "scope": "owned signed compiler and daemon; client wall time includes verification and IPC; warm OS caches",
        "cold_seconds": cold_seconds,
        "cache_hit_seconds": hit_seconds,
        "artifact_bytes": first.len(),
        "artifact_sha256": digest(&first),
        "initial_cache": initial_cache,
        "concurrent_clients": concurrent.len(),
        "concurrent_miss_seconds": concurrent.iter().map(|(_, seconds)| seconds).collect::<Vec<_>>(),
        "concurrent_same_artifact": true,
        "shared_cache": shared_cache,
    });
    fs::write(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/closure-shared-compile-measurement.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "requires root and prepared archive; only owned clients change UID"]
fn different_machine_users_share_compilation_without_private_cache() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    assert_eq!(rustix::process::geteuid().as_raw(), 0);
    let root = source_project();
    let other = source_project();
    // Only these owned fixtures are shared; the usual temporary root is 0700.
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(other.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let (layout, core) = installed_dever(root.path());
    let _daemon = TestDaemon::start(&layout);
    assert_cli_success(
        &core,
        &[
            "run",
            root.path().to_str().unwrap(),
            "--",
            "portal.value.show",
            "{}",
        ],
    );
    assert_eq!(cache_status(&layout).unwrap().entries, 1);
    let result = Command::new(&core)
        .uid(65534)
        .gid(65534)
        .env_clear()
        .args([
            "run",
            other.path().to_str().unwrap(),
            "--",
            "portal.value.show",
            "{}",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("two"));
    assert_eq!(cache_status(&layout).unwrap().entries, 1);
    assert!(!core.parent().unwrap().join("cache").exists());
    assert!(!other.path().join("cache").exists());
}

fn frame(stream: &mut UnixStream) -> serde_json::Value {
    let mut size = [0; 4];
    stream.read_exact(&mut size).unwrap();
    let mut bytes = vec![0; u32::from_be_bytes(size) as usize];
    stream.read_exact(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn upload(layout: &Layout, input: &CompileRequest) -> UnixStream {
    let payload = input.encode().unwrap();
    let mut stream = UnixStream::connect(layout.service_socket()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let token = fs::read_to_string(layout.service_token()).unwrap();
    let control = serde_json::to_vec(&serde_json::json!({"token":token.trim(),
        "operation":{"compile":{"bytes":payload.len()}},"installed":[]}))
    .unwrap();
    stream
        .write_all(&(control.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&control).unwrap();
    assert_eq!(frame(&mut stream)["ok"], true);
    stream.write_all(&payload).unwrap();
    stream
}

fn wait_for<T>(mode: &str, phase: &str, mut action: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(value) = action() {
            return value;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "owned compilation mode={mode} timed out waiting for {phase}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires prepared archive, explicit author cc/LLVM inputs; supervises an owned signed sleeper"]
fn cancellation_disconnect_and_version_lease_reap_worker_and_staging() {
    let root = source_project();
    let sleeper = root.path().join("signed-sleeper");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/dever-cli-tests/support/compiler_sleeper.c");
    let status = Command::new("/usr/bin/cc")
        .env_clear()
        .args([
            "-B/usr/bin/",
            "-O2",
            "-Wl,--disable-new-dtags,-rpath,$ORIGIN/lib,--no-as-needed",
            "-L/usr/lib/llvm-18/lib",
            "-lLLVM-18",
            "-lstdc++",
            "-lz",
            "-lgcc_s",
        ])
        .arg(source)
        .arg("-o")
        .arg(&sleeper)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "explicit native sleeper preparation failed"
    );
    let (layout, _) = signed_core(root.path(), &sleeper, true);
    let _daemon = TestDaemon::start(&layout);
    for mode in ["cancel", "disconnect", "deadline"] {
        let mut stream = upload(&layout, &request(root.path(), CompileKind::Application));
        let pid = wait_for(mode, "worker PID publication", || {
            fs::read_dir(layout.native_cache().join("staging"))
                .ok()?
                .filter_map(Result::ok)
                .find_map(|entry| {
                    fs::read_to_string(entry.path().join("worker.pid"))
                        .ok()?
                        .parse::<i32>()
                        .ok()
                        .filter(|pid| *pid > 0)
                        .map(|pid| pid.to_string())
                })
        });
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(layout.state().join("install.lock"))
            .unwrap();
        assert!(
            fs2::FileExt::try_lock_exclusive(&lock).is_err(),
            "active compiler version was not leased"
        );
        assert!(
            CacheStore::new(&layout)
                .clean(&Default::default())
                .unwrap_err()
                .contains("busy")
        );
        if mode == "cancel" {
            stream.write_all(&[1]).unwrap();
            let response = frame(&mut stream);
            assert_eq!(response["ok"], false);
            assert!(response["error"].as_str().unwrap().contains("cancel"));
        } else if mode == "deadline" {
            stream
                .set_read_timeout(Some(Duration::from_secs(190)))
                .unwrap();
            let response = frame(&mut stream);
            assert_eq!(response["ok"], false);
            assert!(response["error"].as_str().unwrap().contains("deadline"));
        }
        drop(stream);
        wait_for(mode, "worker exit and staging/operation cleanup", || {
            let staging = fs::read_dir(layout.native_cache().join("staging"))
                .ok()?
                .count();
            let operations = fs::read_dir(layout.native_cache().join("operations"))
                .ok()?
                .count();
            (staging == 0 && operations == 0 && !Path::new("/proc").join(&pid).exists())
                .then_some(())
        });
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();
        fs2::FileExt::unlock(&lock).unwrap();
        assert_eq!(cache_status(&layout).unwrap().entries, 0);
    }
}
