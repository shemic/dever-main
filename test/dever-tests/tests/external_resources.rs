#[path = "support/sandbox.rs"]
mod sandbox;
#[path = "support/temp.rs"]
mod temp;

use std::fs;

use dever_runtime::external::{
    Resource, bundle_digest, extract, verified_entry, verified_worker_launch,
};
use sha2::{Digest, Sha256};
use temp::TemporaryDirectory;

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn entry_resolution_is_bound_to_the_manifest_and_rechecks_bytes() {
    let root = TemporaryDirectory::new();
    let bytes = b"verified worker";
    let hash = digest(bytes);
    let resources = [Resource {
        path: "module/mail/delivery/worker",
        bytes,
        sha256: &hash,
        executable: true,
    }];
    let bundle = bundle_digest(&resources);
    let directory = extract(root.path(), &bundle, &resources).unwrap();
    let entry = verified_entry(&directory, &resources, resources[0].path).unwrap();
    assert!(entry.starts_with(&directory));
    assert!(
        verified_entry(&directory, &resources, "other-worker")
            .unwrap_err()
            .contains("manifest")
    );
    fs::write(&entry, b"changed worker").unwrap();
    assert!(
        verified_entry(&directory, &resources, resources[0].path)
            .unwrap_err()
            .contains("integrity")
    );
}

#[test]
fn large_resource_verification_checks_every_chunk_and_exact_length() {
    let root = TemporaryDirectory::new();
    let bytes = vec![157; 262_147];
    let hash = digest(&bytes);
    let resources = [Resource {
        path: "runtime.bin",
        bytes: &bytes,
        sha256: &hash,
        executable: true,
    }];
    let directory = extract(root.path(), &bundle_digest(&resources), &resources).unwrap();
    let path = verified_entry(&directory, &resources, "runtime.bin").unwrap();
    let mut tampered = bytes.clone();
    tampered[128 * 1024] ^= 1;
    for altered in [&tampered[..], &bytes[..bytes.len() - 1]] {
        fs::write(&path, altered).unwrap();
        assert!(
            verified_entry(&directory, &resources, "runtime.bin")
                .unwrap_err()
                .contains("integrity")
        );
    }
    let mut appended = bytes.clone();
    appended.push(1);
    fs::write(&path, &appended).unwrap();
    assert!(
        verified_entry(&directory, &resources, "runtime.bin")
            .unwrap_err()
            .contains("integrity")
    );
    fs::write(&path, &bytes).unwrap();
    assert_eq!(
        verified_entry(&directory, &resources, "runtime.bin").unwrap(),
        path
    );
}

#[test]
fn managed_launch_is_bound_to_its_checked_entry_and_private_execution_tree() {
    let root = TemporaryDirectory::new();
    let entry = "module/mail/delivery/adapter/worker.py";
    let manifest_path = format!("{entry}.dever-worker.json");
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format": "dever-worker-launch-v1", "ecosystem": "pip", "entry": entry,
        "executable": "workers/fixture/runtime/bin/python3",
        "arguments": [{"literal":"-I"},{"resource":"workers/fixture/runner.py"}],
        "working_directory": "workers/fixture"
    }))
    .unwrap();
    let interpreter = b"managed interpreter bytes";
    let runner = b"generated checked runner";
    let hashes = [digest(&manifest), digest(interpreter), digest(runner)];
    let resources = [
        Resource {
            path: &manifest_path,
            bytes: &manifest,
            sha256: &hashes[0],
            executable: false,
        },
        Resource {
            path: "workers/fixture/runtime/bin/python3",
            bytes: interpreter,
            sha256: &hashes[1],
            executable: true,
        },
        Resource {
            path: "workers/fixture/runner.py",
            bytes: runner,
            sha256: &hashes[2],
            executable: false,
        },
    ];
    let directory = extract(root.path(), &bundle_digest(&resources), &resources).unwrap();
    let launch = verified_worker_launch(&directory, &resources, entry, "pip").unwrap();
    assert_eq!(launch.executable, directory.join(resources[1].path));
    assert_eq!(
        launch.arguments,
        [
            std::ffi::OsString::from("-I"),
            directory.join(resources[2].path).into_os_string()
        ]
    );
    assert_eq!(launch.working_directory, directory.join("workers/fixture"));
    assert!(
        verified_worker_launch(&directory, &resources, entry, "npm")
            .unwrap_err()
            .contains("identity")
    );
    assert!(
        verified_worker_launch(&directory, &resources, "other.py", "pip")
            .unwrap_err()
            .contains("missing")
    );
    fs::write(&launch.executable, b"changed interpreter").unwrap();
    assert!(
        verified_worker_launch(&directory, &resources, entry, "pip")
            .unwrap_err()
            .contains("integrity")
    );
}

#[test]
fn managed_launch_rejects_cross_worker_paths_and_unbounded_arguments() {
    let entry = "module/mail/delivery/adapter/worker.mjs";
    let manifest_path = format!("{entry}.dever-worker.json");
    let base = serde_json::json!({
        "format": "dever-worker-launch-v1", "ecosystem": "npm", "entry": entry,
        "executable": "workers/fixture/node",
        "arguments": [{"resource":"workers/fixture/runner.mjs"}],
        "working_directory": "workers/fixture"
    });
    let mut cases = Vec::new();
    for (field, value) in [
        ("working_directory", serde_json::json!("workers/../outside")),
        ("executable", serde_json::json!("workers/neighbor/node")),
        (
            "arguments",
            serde_json::json!([{"resource":"workers/neighbor/runner.mjs"}]),
        ),
        (
            "arguments",
            serde_json::json!([{"literal":"bad\u{0000}argument"}]),
        ),
        (
            "arguments",
            serde_json::json!([{"resource":"/usr/bin/node"}]),
        ),
        (
            "arguments",
            serde_json::json!([{"resource":"workers/fixture/runner.mjs","literal":"hidden"}]),
        ),
        (
            "arguments",
            serde_json::json!(
                (0..65)
                    .map(|_| serde_json::json!({"literal":"a"}))
                    .collect::<Vec<_>>()
            ),
        ),
        (
            "arguments",
            serde_json::json!([{"literal":"a".repeat(65_537)}]),
        ),
    ] {
        let mut case = base.clone();
        case[field] = value;
        cases.push(case);
    }
    for case in cases {
        let root = TemporaryDirectory::new();
        let manifest = serde_json::to_vec(&case).unwrap();
        let hash = digest(&manifest);
        let bytes = b"fixture";
        let bytes_hash = digest(bytes);
        let resources = [
            Resource {
                path: &manifest_path,
                bytes: &manifest,
                sha256: &hash,
                executable: false,
            },
            Resource {
                path: "workers/fixture/node",
                bytes,
                sha256: &bytes_hash,
                executable: true,
            },
            Resource {
                path: "workers/fixture/runner.mjs",
                bytes,
                sha256: &bytes_hash,
                executable: false,
            },
            Resource {
                path: "workers/neighbor/node",
                bytes,
                sha256: &bytes_hash,
                executable: true,
            },
            Resource {
                path: "workers/neighbor/runner.mjs",
                bytes,
                sha256: &bytes_hash,
                executable: false,
            },
        ];
        let directory = extract(root.path(), &bundle_digest(&resources), &resources).unwrap();
        assert!(
            verified_worker_launch(&directory, &resources, entry, "npm").is_err(),
            "accepted {case}"
        );
    }
}

#[test]
fn manifest_identity_includes_execute_permission_and_rejects_duplicates() {
    let root = TemporaryDirectory::new();
    let bytes = b"worker";
    let hash = digest(bytes);
    let mut resources = [Resource {
        path: "worker",
        bytes,
        sha256: &hash,
        executable: false,
    }];
    let original = bundle_digest(&resources);
    let directory = extract(root.path(), &original, &resources).unwrap();
    assert!(
        verified_entry(&directory, &resources, "worker")
            .unwrap_err()
            .contains("not executable")
    );
    resources[0].executable = true;
    assert_ne!(original, bundle_digest(&resources));
    assert!(
        extract(root.path(), &original, &resources)
            .unwrap_err()
            .contains("bundle identity")
    );
    let duplicate = [resources[0], resources[0]];
    assert!(
        extract(root.path(), &bundle_digest(&duplicate), &duplicate)
            .unwrap_err()
            .contains("duplicate")
    );
}

#[test]
fn concurrent_first_extraction_publishes_one_complete_manifest() {
    let root = TemporaryDirectory::new();
    let bytes = b"worker";
    let hash = digest(bytes);
    let resources = [Resource {
        path: "worker",
        bytes,
        sha256: &hash,
        executable: true,
    }];
    let bundle = bundle_digest(&resources);
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..4 {
            let root = root.path();
            let resources = &resources;
            let bundle = &bundle;
            let barrier = &barrier;
            workers.push(scope.spawn(move || {
                barrier.wait();
                let directory = extract(root, bundle, resources).unwrap();
                verified_entry(&directory, resources, "worker").unwrap()
            }));
        }
        let paths = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert!(paths.iter().all(|path| path == &paths[0]));
    });
    assert_eq!(
        fs::read_dir(root.path().join("data/cache/lib"))
            .unwrap()
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn extraction_and_launch_reject_symlinks_in_every_path_segment() {
    use std::os::unix::fs::symlink;
    let root = TemporaryDirectory::new();
    let outside = TemporaryDirectory::new();
    let bytes = b"worker";
    let hash = digest(bytes);
    let resources = [Resource {
        path: "nested/worker",
        bytes,
        sha256: &hash,
        executable: true,
    }];
    let bundle = bundle_digest(&resources);
    symlink(outside.path(), root.path().join("data")).unwrap();
    assert!(
        extract(root.path(), &bundle, &resources)
            .unwrap_err()
            .contains("symbolic link")
    );
    fs::remove_file(root.path().join("data")).unwrap();
    let directory = extract(root.path(), &bundle, &resources).unwrap();
    fs::rename(directory.join("nested"), outside.path().join("nested")).unwrap();
    symlink(outside.path().join("nested"), directory.join("nested")).unwrap();
    assert!(
        verified_entry(&directory, &resources, "nested/worker")
            .unwrap_err()
            .contains("symbolic link")
    );
}

#[cfg(unix)]
#[test]
fn existing_cache_permissions_are_checked_before_reuse_and_launch() {
    use std::os::unix::fs::PermissionsExt;
    let root = TemporaryDirectory::new();
    let bytes = b"worker";
    let hash = digest(bytes);
    let resources = [Resource {
        path: "worker",
        bytes,
        sha256: &hash,
        executable: true,
    }];
    let bundle = bundle_digest(&resources);
    fs::create_dir(root.path().join("data")).unwrap();
    fs::set_permissions(root.path().join("data"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        extract(root.path(), &bundle, &resources)
            .unwrap_err()
            .contains("permissions")
    );
    fs::set_permissions(root.path().join("data"), fs::Permissions::from_mode(0o700)).unwrap();
    let directory = extract(root.path(), &bundle, &resources).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        extract(root.path(), &bundle, &resources)
            .unwrap_err()
            .contains("permissions")
    );
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(directory.join("worker"), fs::Permissions::from_mode(0o770)).unwrap();
    assert!(
        verified_entry(&directory, &resources, "worker")
            .unwrap_err()
            .contains("permissions")
    );
}

#[test]
#[ignore = "requires explicitly prepared sandbox assets and the private Rust bootstrap compiler"]
fn packaged_exec_runs_without_source_worker_or_inherited_environment() {
    let root = TemporaryDirectory::new();
    let mut sources = dever_core::source::SourceMap::default();
    sources.add(
        "notification/mail/port.dever",
        "read() (value: Int) fails app.ReadResult\n",
    );
    sources.add("notification/mail/app.dever", "type ReadResult { error Unavailable(message: Text) }\nread() (value: Int) { value = port.read() }\n");
    sources.add(
        "notification/mail/adapter.dever",
        "setting {\n mode: Text\n marker: Text\n}\nexternal exec \"worker\" { allow file }\n",
    );
    sources.add("notification/mail/api.dever", "cmd read = app.read\n");
    let program = dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"));
    let worker = fs::read(env!("CARGO_BIN_EXE_component-fixture")).unwrap();
    let entry = program.external_worker_entries().pop().unwrap();
    let resource = dever_core::native::EmbeddedResource {
        path: "workers/fixture/worker".into(),
        sha256: digest(&worker),
        bytes: worker,
        executable: true,
    };
    let mut copy = resource.clone();
    copy.path = "assets/worker-copy.bin".into();
    copy.executable = false;
    let mut resources = vec![resource, copy];
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format":"dever-worker-launch-v1", "ecosystem":"exec", "entry":entry,
        "executable":"workers/fixture/worker", "arguments":[],
        "working_directory":"workers/fixture"
    }))
    .unwrap();
    resources.push(dever_core::native::EmbeddedResource {
        path: format!("{entry}.dever-worker.json"),
        sha256: digest(&manifest),
        bytes: manifest,
        executable: false,
    });
    for asset in sandbox::assets(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")) {
        resources.push(dever_core::native::EmbeddedResource {
            path: asset.path,
            sha256: digest(&asset.bytes),
            bytes: asset.bytes,
            executable: asset.executable,
        });
    }
    let native = dever_core::native::compile_project_with_resources(
        &program,
        &sources,
        std::ffi::OsStr::new("rustc"),
        dever_runtime::config::RuntimeProfile::default(),
        &resources,
    )
    .unwrap();
    let executable = root.path().join("program");
    native.save(&executable).unwrap();
    fs::create_dir(root.path().join("config")).unwrap();
    fs::create_dir(root.path().join("lifecycle")).unwrap();
    fs::write(
        root.path().join("config/setting.json"),
        r#"{"adapter":{"notification.mail":{"use":"default","setting":{"mode":"normal","marker":"/data/lifecycle/worker"},"files":{"lifecycle":{"path":"lifecycle","write":true}}}}}"#,
    )
    .unwrap();
    assert!(!root.path().join(&entry).exists());
    let shutdown = root.path().join("lifecycle/worker.shutdown");
    for _ in 0..2 {
        let result = std::process::Command::new(&executable)
            .env_clear()
            .current_dir(root.path())
            .args(["notification.mail.read", "{}"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let response: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(response["code"], 0);
        assert_eq!(response["data"], 7);
        assert!(
            shutdown.is_file(),
            "entry cleanup must complete the Worker shutdown handshake"
        );
        // 下一次执行必须留下新的确认，不能复用上一次的关机标记。
        fs::remove_file(&shutdown).unwrap();
    }
    let bundle = fs::read_dir(root.path().join("data/cache/lib"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::read(bundle.join("assets/worker-copy.bin")).unwrap(),
        resources[0].bytes
    );
    fs::write(bundle.join("workers/fixture/worker"), b"tampered").unwrap();
    let result = std::process::Command::new(&executable)
        .env_clear()
        .args(["notification.mail.read", "{}"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("integrity verification"));
}
