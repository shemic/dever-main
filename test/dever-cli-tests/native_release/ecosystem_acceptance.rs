//! Explicit signed-release acceptance with no host language trees in the final root.
use super::*;
use std::os::unix::fs::{MetadataExt, chown};

const ECOSYSTEMS: [&str; 3] = ["pip", "npm", "go"];
const STANDALONE_UID: u32 = 65534;

fn nonroot_standalone(source: &Path, expected_program: &Value) -> Value {
    let deployment = TemporaryDirectory::new_in(Path::new("/tmp"));
    let program = deployment.path().join("program");
    let config = deployment.path().join("config");
    let settings = config.join("setting.json");
    fs::create_dir(&config).unwrap();
    fs::copy(source.join("program"), &program).unwrap();
    fs::copy(source.join("config/setting.json"), &settings).unwrap();
    let identity = json!({
        "bytes": fs::metadata(&program).unwrap().len(),
        "sha256": sha256_file(&program).unwrap(),
    });
    assert_eq!(&identity, expected_program);
    for (path, mode) in [
        (program.as_path(), 0o700),
        (config.as_path(), 0o700),
        (settings.as_path(), 0o600),
        (deployment.path(), 0o700),
    ] {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        chown(path, Some(STANDALONE_UID), Some(STANDALONE_UID)).unwrap();
    }
    // Only the standalone executable and private config cross this boundary.
    // The actual non-root process must extract its own embedded Worker bundle.
    assert!(!deployment.path().join("data").exists());
    let output = command(
        Command::new("/usr/bin/setpriv")
            .arg(format!("--reuid={STANDALONE_UID}"))
            .arg(format!("--regid={STANDALONE_UID}"))
            .args([
                "--clear-groups",
                "--inh-caps=-all",
                "--ambient-caps=-all",
                "--",
            ])
            .arg(&program)
            .args(["sample.worker.probe", "{}"])
            .current_dir(deployment.path())
            .stdin(Stdio::null()),
    );
    eprintln!(
        "standalone UID {STANDALONE_UID} {}: {}\n{}",
        source.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_response(output);
    assert_eq!(
        fs::metadata(deployment.path().join("data/cache/lib"))
            .unwrap()
            .uid(),
        STANDALONE_UID
    );
    json!({"passed": true, "uid": STANDALONE_UID, "program": identity})
}

fn dependency_fixture(workspace: &Path, ecosystem: &str) -> PathBuf {
    workspace.join(match ecosystem {
        "pip" => "target/source-build",
        "npm" => "target/npm-build",
        _ => unreachable!(),
    })
}

/// The fixtures were built against these exact pack bytes. The test author
/// signs those inputs, rather than rewriting receipts for a recompressed pack.
fn sign_build_runtimes(
    release: &Path,
    workspace: &Path,
    key: &Ed25519KeyPair,
) -> dever_cli::toolchain::ReleaseManifest {
    use dever_cli::libs::{LockFile, RegistryRuntime, RuntimePack};

    let mut manifest: dever_cli::toolchain::ReleaseManifest =
        serde_json::from_slice(&fs::read(release.join("manifest.json")).unwrap()).unwrap();
    for ecosystem in ["pip", "npm"] {
        let fixture = dependency_fixture(workspace, ecosystem);
        let settings: Value =
            serde_json::from_slice(&fs::read(fixture.join("config/setting.json")).unwrap())
                .unwrap();
        let runtime: RegistryRuntime = serde_json::from_value(settings["runtime"].clone()).unwrap();
        assert_eq!(runtime.target, "linux-x86_64");
        let pack = fixture.join(settings["runtime_file"].as_str().unwrap());
        assert_eq!(sha256_file(&pack).unwrap(), runtime.pack.sha256);
        let lock =
            LockFile::decode(&fs::read(fixture.join("dependency/dever.lock")).unwrap()).unwrap();
        assert!(
            !lock.builds.is_empty(),
            "acceptance requires actual source build receipts"
        );
        for receipt in &lock.builds {
            assert_eq!(receipt.runtime, runtime.pack);
            if let Some(python) = &receipt.auxiliary_runtime {
                let python_config: Value = serde_json::from_slice(
                    &fs::read(dependency_fixture(workspace, "pip").join("config/setting.json"))
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(
                    *python,
                    serde_json::from_value::<RuntimePack>(python_config["runtime"]["pack"].clone())
                        .unwrap()
                );
            }
        }
        let prefix = format!("runtime/{ecosystem}/linux-x86_64");
        fs::copy(pack, release.join(&prefix).join("runtime.pack")).unwrap();
        fs::write(
            release.join(&prefix).join("manifest.json"),
            serde_json::to_vec(&json!({"format":"dever-registry-runtime-v1","runtime":runtime}))
                .unwrap(),
        )
        .unwrap();
        for suffix in ["runtime.pack", "manifest.json"] {
            let name = format!("{prefix}/{suffix}");
            let artifact = manifest
                .extensions
                .iter_mut()
                .flat_map(|extension| &mut extension.artifacts)
                .find(|artifact| artifact.path == name)
                .unwrap();
            artifact.bytes = fs::metadata(release.join(&name)).unwrap().len();
            artifact.sha256 = sha256_file(&release.join(&name)).unwrap();
        }
    }
    let encoded = serde_json::to_vec(&manifest).unwrap();
    fs::write(release.join("manifest.json"), &encoded).unwrap();
    fs::write(
        release.join("manifest.sig"),
        key.sign(&encoded)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .unwrap();
    manifest
}

fn import_built_dependencies(project: &Path, fixture: &Path, layout: &Layout) {
    use dever_cli::libs::{LockFile, LockedWorker, build};

    let mut lock =
        LockFile::decode(&fs::read(fixture.join("dependency/dever.lock")).unwrap()).unwrap();
    for artifact in lock
        .libs
        .iter()
        .flat_map(|lib| &lib.artifacts)
        .chain(build::npm_outputs(&lock).map(|receipt| &receipt.output))
    {
        let bytes = fs::read(fixture.join("dependency/artifacts").join(&artifact.sha256)).unwrap();
        let stored = dever_cli::toolchain::artifact_put(layout, &bytes).unwrap();
        assert_eq!(stored.sha256, artifact.sha256);
        assert_eq!(stored.bytes, artifact.bytes);
    }
    let sources = dever_core::source::SourceMap::load(&project.join("module")).unwrap();
    let program = dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"));
    let contract = program.external_worker_contracts().remove(0);
    let runtime = lock.libs[0].runtime.clone();
    assert!(lock.libs.iter().all(|lib| lib.runtime == runtime));
    assert!(lock.workers.is_empty());
    lock.workers.push(LockedWorker {
        port: contract.port,
        adapter: contract.adapter,
        ecosystem: contract.ecosystem,
        runtime: Some(runtime),
        entry: contract.entry,
        schema: contract.schema,
        capabilities: contract.capabilities,
        operations: contract.operations,
        libs: contract
            .libs
            .iter()
            .map(|spec| spec.parse().unwrap())
            .collect(),
    });
    dever_cli::libs::doctor(&lock).unwrap();
    lock.write_atomic(project).unwrap();
}

fn ecosystem_inputs(author: &Path, workspace: &Path) {
    let prepared = workspace.join("target/ecosystem-release-inputs/prepared");
    let fragment: Value =
        serde_json::from_slice(&fs::read(prepared.join("config/setting.json")).unwrap()).unwrap();
    for ecosystem in ECOSYSTEMS {
        for entry in fragment["runtimes"][ecosystem]["files"].as_array().unwrap() {
            let relative = entry["source"].as_str().unwrap();
            let output = author.join(relative);
            fs::create_dir_all(output.parent().unwrap()).unwrap();
            // Author staging only; the maker materializes its own immutable pack.
            fs::hard_link(prepared.join(relative), output).unwrap();
        }
    }
    let config = author.join("config/setting.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    settings["runtimes"] = fragment["runtimes"].clone();
    fs::write(config, serde_json::to_vec(&settings).unwrap()).unwrap();
}

fn application(root: &Path, ecosystem: &str) {
    let (extension, source) = match ecosystem {
        "pip" => ("py", include_str!("../../ecosystem-release/probe.py")),
        "npm" => ("mjs", include_str!("../../ecosystem-release/probe.mjs")),
        "go" => ("go", include_str!("../../ecosystem-release/probe.go")),
        _ => unreachable!(),
    };
    write(root, "config/setting.json", "{}\n");
    write(
        root,
        "module/sample/worker/port.dever",
        "probe() (okay: Bool) fails app.ProbeResult\n",
    );
    write(
        root,
        "module/sample/worker/app.dever",
        "type ProbeResult { error Unavailable(message: Text) }\nprobe() (okay: Bool) { okay = port.probe() }\n",
    );
    write(
        root,
        "module/sample/worker/api.dever",
        "cmd probe = app.probe\n",
    );
    write(
        root,
        "module/sample/worker/adapter.dever",
        format!(
            "external {ecosystem} \"worker/probe.{extension}\" {{\n{}\n}}\n",
            match ecosystem {
                "pip" => "lib \"simplejson@3.20.1\"",
                "npm" => "lib \"bufferutil@4.0.9\"",
                "go" => "",
                _ => unreachable!(),
            }
        ),
    );
    let sources = dever_core::source::SourceMap::load(&root.join("module")).unwrap();
    let program = dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"));
    let entry = &program.external_worker_contracts()[0].entry;
    write(root, entry, source);
}

pub(super) fn minimal_os(root: &Path) {
    // This root contains only the named Linux OS ABI libraries. No host language
    // executable, standard library, PATH, package manager or source tree is visible.
    for name in [
        "libc.so.6",
        "libdl.so.2",
        "libm.so.6",
        "libpthread.so.0",
        "librt.so.1",
        "libutil.so.1",
        "libresolv.so.2",
        "ld-linux-x86-64.so.2",
    ] {
        let source = Path::new("/lib/x86_64-linux-gnu").join(name);
        let output = root.join("lib/x86_64-linux-gnu").join(name);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::copy(source, output).unwrap();
    }
    fs::create_dir(root.join("lib64")).unwrap();
    fs::copy(
        "/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2",
        root.join("lib64/ld-linux-x86-64.so.2"),
    )
    .unwrap();
    fs::create_dir(root.join("tmp")).unwrap();
    fs::create_dir(root.join("proc")).unwrap();
    fs::create_dir(root.join("dev")).unwrap();
    // The outer namespace supplies only its own proc and synthetic device tree.
    assert!(!root.join("usr").exists());
}

#[test]
#[ignore = "requires prepared pinned ecosystem inputs and root for owned namespace; no existing services"]
fn signed_ecosystems_build_and_run_without_system_language_installations() {
    assert_eq!(
        rustix::process::geteuid().as_raw(),
        0,
        "explicit no-host acceptance requires the owned root namespace fixture"
    );
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let projects = TemporaryDirectory::new();
    for ecosystem in ECOSYSTEMS {
        application(&projects.path().join(ecosystem), ecosystem);
    }
    let author = TemporaryDirectory::new();
    let key = author_inputs(author.path(), &workspace);
    ecosystem_inputs(author.path(), &workspace);
    let machine = machine_temporary_directory(&workspace);
    let layout = Layout::new(machine.path());
    layout.initialize().unwrap();
    let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let release = layout.downloads().join(version.as_str());
    command_with_timeout(
        Command::new(workspace.join("target/debug/examples/native-release"))
            .arg(author.path())
            .arg("--output")
            .arg(&release),
        // Debug gzip compresses complete language distributions; this is author
        // preparation, independent of the service's bounded compile deadline.
        Duration::from_secs(900),
    );
    let signed = sign_build_runtimes(&release, &workspace, &key);
    let mut runtimes = serde_json::Map::new();
    for ecosystem in ECOSYSTEMS {
        let path = format!("runtime/{ecosystem}/linux-x86_64/runtime.pack");
        let artifact = signed
            .extensions
            .iter()
            .flat_map(|extension| &extension.artifacts)
            .find(|artifact| artifact.path == path)
            .unwrap();
        runtimes.insert(
            ecosystem.into(),
            json!({"bytes":artifact.bytes,"sha256":artifact.sha256}),
        );
    }
    install_release(&layout, &version, &key);
    fs::remove_dir_all(&release).unwrap();
    drop(author);
    let core = layout.versions().join(version.as_str()).join("dever-core");
    let mut daemon = daemon::TestDaemon::start(&layout);
    let isolated = TemporaryDirectory::new();
    minimal_os(isolated.path());
    let mut programs = serde_json::Map::new();
    for ecosystem in ECOSYSTEMS {
        let project = projects.path().join(ecosystem);
        if ecosystem == "go" {
            command(Command::new(&core).args(["lib", "add"]).arg(&project));
        } else {
            import_built_dependencies(
                &project,
                &dependency_fixture(&workspace, ecosystem),
                &layout,
            );
        }
        let lock =
            dever_cli::libs::LockFile::decode(&fs::read(project.join("dever.lock")).unwrap())
                .unwrap();
        assert_eq!(lock.workers.len(), 1);
        assert_eq!(lock.libs.is_empty(), ecosystem == "go");
        assert_eq!(lock.builds.is_empty(), ecosystem == "go");
        assert_eq!(
            lock.workers[0].runtime.as_ref().unwrap().sha256,
            runtimes[ecosystem]["sha256"].as_str().unwrap()
        );
        command(Command::new(&core).arg("check").arg(&project));
        assert_response(command(
            Command::new(&core)
                .arg("run")
                .arg(&project)
                .args(["--", "sample.worker.probe", "{}"]),
        ));
        let destination = isolated.path().join(ecosystem);
        fs::create_dir(&destination).unwrap();
        write(&destination, "config/setting.json", "{}\n");
        let executable = destination.join("program");
        command(
            Command::new(&core)
                .arg("build")
                .arg(&project)
                .arg("--output")
                .arg(&executable),
        );
        programs.insert(ecosystem.into(), json!({"bytes":fs::metadata(&executable).unwrap().len(),"sha256":sha256_file(&executable).unwrap()}));
    }
    assert_eq!(cache_status(&layout).unwrap().entries, 3);
    daemon.stop();
    drop(machine);
    drop(projects);
    for ecosystem in ECOSYSTEMS {
        assert_response(command(
            isolated_os_command(isolated.path())
                .arg(format!("/{ecosystem}/program"))
                .args(["sample.worker.probe", "{}"]),
        ));
    }
    let mut standalone_nonroot = serde_json::Map::new();
    for ecosystem in ECOSYSTEMS {
        standalone_nonroot.insert(
            ecosystem.into(),
            nonroot_standalone(&isolated.path().join(ecosystem), &programs[ecosystem]),
        );
    }
    write(&workspace, "target/closure-sandbox-ecosystem-release-acceptance.json", serde_json::to_vec_pretty(&json!({
        "format":"dever-ecosystem-release-acceptance-v1", "platform":"linux-x86_64",
        "runtimes":runtimes,"programs":programs,"standalone_nonroot":standalone_nonroot,
        "checks":{"signed_install":true,"locked_runtimes":3,"managed_check_run_build":3,"standalone_isolated_root":3,"standalone_nonroot":3,"shared_compile_entries":3,"python_stdlib_paths_in_bundle":true,"os_sandbox":true,"offline_build_receipt_replay":true},
        "os_baseline":"explicit Linux GNU OS libraries and named device nodes inside an owned root",
        "runtime_pack_provenance":"test author signed the exact pip/npm packs bound to the completed source builds; maker determinism is verified separately",
        "third_party_packages":{"pip":"simplejson@3.20.1 _speedups C extension","npm":"bufferutil@4.0.9 build/Release addon"}, "other_platforms":"not_run", "public_release":"not_published"
    })).unwrap());
}
