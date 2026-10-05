//! Produce real standalone ARM Worker applications for full-system acceptance.
use super::*;
use dever_cli::libs::{
    self, LibSpec, LockFile, LockedArtifact, LockedLib, LockedWorker, RegistryRuntime,
};
use dever_cli::toolchain::{Artifact, Extension, ExtensionKind, ReleaseManifest};
use dever_core::hir::ExternalWorkerContract;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const ECOSYSTEMS: [&str; 3] = ["pip", "npm", "go"];
const TARGET: BuildTarget = BuildTarget::LinuxAarch64;

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn add_target_sandbox(author: &Path, workspace: &Path) {
    let root = workspace.join("target/arm64-cross/sandbox");
    let mut files = Vec::new();
    let mut inputs = Vec::new();
    for directory in ["bin", "lib"] {
        for entry in fs::read_dir(root.join(directory)).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            let relative = format!("{directory}/{}", entry.file_name().to_str().unwrap());
            let bytes = fs::read(entry.path()).unwrap();
            let source = format!("inputs/arm-sandbox/{relative}");
            write(author, &source, &bytes);
            inputs.push(json!({"source":source,"path":relative,"sha256":digest(&bytes)}));
            files.push((relative, bytes));
        }
    }
    dever_sandbox::validate_assets_for_target(&files, TARGET.platform()).unwrap();
    let path = author.join("config/setting.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    settings["sandbox_targets"] = json!({"linux-aarch64":inputs});
    fs::write(path, serde_json::to_vec(&settings).unwrap()).unwrap();
}

// Sign the exact prepared archives: their identities also bind each Lib lock.
// As in the native ecosystem fixture, this is an explicit private test author.
fn sign_target_runtimes(
    release: &Path,
    workspace: &Path,
    key: &Ed25519KeyPair,
    ecosystems: &[&str],
) -> BTreeMap<String, RegistryRuntime> {
    let mut manifest: ReleaseManifest =
        serde_json::from_slice(&fs::read(release.join("manifest.json")).unwrap()).unwrap();
    let mut runtimes = BTreeMap::new();
    for &ecosystem in ecosystems {
        let prepared = workspace
            .join("target/arm64-cross/ecosystems")
            .join(ecosystem);
        let settings: Value =
            serde_json::from_slice(&fs::read(prepared.join("config/setting.json")).unwrap())
                .unwrap();
        let runtime: RegistryRuntime = serde_json::from_value(settings["runtime"].clone()).unwrap();
        assert_eq!(runtime.target, TARGET.platform());
        let pack = fs::read(prepared.join("runtime.pack")).unwrap();
        assert_eq!(digest(&pack), runtime.pack.sha256);
        let prefix = format!("runtime/{ecosystem}/{}", TARGET.platform());
        write(release, &format!("{prefix}/runtime.pack"), pack);
        write(
            release,
            &format!("{prefix}/manifest.json"),
            serde_json::to_vec(&json!({"format":"dever-registry-runtime-v1","runtime":runtime}))
                .unwrap(),
        );
        let mut artifacts = Vec::new();
        for leaf in ["manifest.json", "runtime.pack"] {
            let path = format!("{prefix}/{leaf}");
            assert!(
                !manifest
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.path == path)
            );
            artifacts.push(Artifact {
                bytes: fs::metadata(release.join(&path)).unwrap().len(),
                sha256: sha256_file(&release.join(&path)).unwrap(),
                path,
            });
        }
        manifest.extensions.push(Extension {
            kind: ExtensionKind::Runtime(ecosystem.parse().unwrap()),
            target: TARGET,
            artifacts,
        });
        runtimes.insert(ecosystem.to_owned(), runtime);
    }
    manifest.extensions.sort_by_key(Extension::id);
    dever_cli::toolchain::validate_catalog(&manifest).unwrap();
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
    runtimes
}

fn application(root: &Path, ecosystem: &str) -> ExternalWorkerContract {
    let (extension, library, source) = match ecosystem {
        "pip" => (
            "py",
            "six@1.17.0",
            include_str!("../../ecosystem-release/arm64/probe.py"),
        ),
        "npm" => (
            "mjs",
            "is-number@7.0.0",
            include_str!("../../ecosystem-release/arm64/probe.mjs"),
        ),
        "go" => (
            "go",
            "github.com/google/uuid@1.6.0",
            include_str!("../../ecosystem-release/arm64/probe.go"),
        ),
        _ => unreachable!(),
    };
    write(root, "config/setting.json", "{}\n");
    write(
        root,
        "module/sample/worker/port.dever",
        "probe() (okay: Bool) fails app.ProbeResult\nreject() (okay: Bool) fails app.ProbeResult\n",
    );
    write(
        root,
        "module/sample/worker/app.dever",
        "type ProbeResult { error Unavailable(message: Text) }\nprobe() (okay: Bool) { okay = port.probe() }\nreject() (okay: Bool) { okay = port.reject() }\n",
    );
    write(
        root,
        "module/sample/worker/api.dever",
        "cmd probe = app.probe\ncmd reject = app.reject\n",
    );
    write(
        root,
        "module/sample/worker/adapter.dever",
        format!("external {ecosystem} \"worker/probe.{extension}\" {{ lib \"{library}\" }}\n"),
    );
    let sources = SourceMap::load(&root.join("module")).unwrap();
    let program = dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"));
    let contract = program.external_worker_contracts().remove(0);
    write(root, &contract.entry, source);
    contract
}

fn pure_package_lock(
    workspace: &Path,
    contract: &ExternalWorkerContract,
    runtime: &RegistryRuntime,
) -> (LockFile, BTreeMap<String, Vec<u8>>) {
    let spec: LibSpec = contract.libs[0].parse().unwrap();
    let (filename, extension) = match contract.ecosystem.as_str() {
        "pip" => ("six-1.17.0-py2.py3-none-any.whl", "whl"),
        "npm" => ("is-number-7.0.0.tgz", "tgz"),
        _ => unreachable!(),
    };
    let bytes = fs::read(
        workspace
            .join("target/arm64-cross/dependencies")
            .join(filename),
    )
    .unwrap();
    let sha256 = digest(&bytes);
    let artifact = LockedArtifact {
        source: None,
        target: TARGET.platform().into(),
        path: format!(
            "lib/{}/{}/{}/archive.{extension}",
            contract.ecosystem,
            digest(spec.distribution_name().as_bytes()),
            spec.version
        ),
        bytes: bytes.len() as u64,
        sha256: sha256.clone(),
    };
    let mut lock = LockFile::new(vec![LockedLib {
        spec: spec.clone(),
        dependencies: vec![],
        runtime: runtime.pack.clone(),
        artifacts: vec![artifact],
        schema: contract.schema.clone(),
        build: None,
    }])
    .unwrap();
    if contract.ecosystem == "npm" {
        lock.npm.push(libs::npm::Environment::single(spec));
    }
    (lock, BTreeMap::from([(sha256, bytes)]))
}

fn go_package_lock(
    workspace: &Path,
    runtime: &RegistryRuntime,
) -> (LockFile, BTreeMap<String, Vec<u8>>) {
    let prepared = workspace.join("target/go-managed/dependency");
    let mut lock = LockFile::decode(&fs::read(prepared.join("dever.lock")).unwrap()).unwrap();
    libs::doctor(&lock).unwrap();
    assert!(lock.builds.is_empty() && lock.workers.is_empty());
    assert_eq!(lock.libs.len(), 1);
    assert_eq!(lock.libs[0].spec.key(), "go:github.com/google/uuid@1.6.0");
    let authenticated =
        libs::sumdb::verify_chain(&lock.go_sumdb, &libs::sumdb::Verifier::official()).unwrap();
    let mut archives = BTreeMap::new();
    for library in &mut lock.libs {
        // Go module ZIPs are source-only; keep the verified source and sumdb
        // evidence unchanged while selecting the new runtime and build target.
        library.runtime = runtime.pack.clone();
        for artifact in &mut library.artifacts {
            let bytes = fs::read(prepared.join("artifacts").join(&artifact.sha256)).unwrap();
            assert_eq!(digest(&bytes), artifact.sha256);
            assert_eq!(bytes.len() as u64, artifact.bytes);
            authenticated
                .verify_zip(&library.spec.name, &library.spec.version, &bytes)
                .unwrap();
            artifact.target = TARGET.platform().into();
            archives.insert(artifact.sha256.clone(), bytes);
        }
    }
    (lock, archives)
}

fn prepare_locked_application(
    project: &Path,
    workspace: &Path,
    layout: &Layout,
    contract: ExternalWorkerContract,
    runtime: &RegistryRuntime,
) -> LockFile {
    let (mut lock, archives) = if contract.ecosystem == "go" {
        go_package_lock(workspace, runtime)
    } else {
        pure_package_lock(workspace, &contract, runtime)
    };
    for (sha256, bytes) in archives {
        let receipt = dever_cli::toolchain::artifact_put(layout, &bytes).unwrap();
        assert_eq!(receipt.sha256, sha256);
        assert_eq!(receipt.bytes, bytes.len() as u64);
    }
    lock.workers.push(LockedWorker {
        libs: contract
            .libs
            .iter()
            .map(|value| value.parse().unwrap())
            .collect(),
        port: contract.port,
        adapter: contract.adapter,
        ecosystem: contract.ecosystem,
        runtime: Some(runtime.pack.clone()),
        entry: contract.entry,
        schema: contract.schema,
        capabilities: contract.capabilities,
        operations: contract.operations,
    });
    libs::doctor(&lock).unwrap();
    lock.write_atomic(project).unwrap();
    lock
}

#[test]
#[ignore = "requires explicit ARM runtimes/dependencies/sandbox and native archives; builds with an owned host daemon and exports apps for ARM system QEMU"]
fn signed_arm_ecosystems_build_offline_standalone_apps_for_system_guest() {
    build_guest_apps(&ECOSYSTEMS, "ecosystem-build-acceptance.json");
}

#[test]
#[ignore = "requires explicit ARM inputs; rebuilds only Python for SDK shutdown regression under the ARM guest"]
fn signed_arm_python_worker_rebuilds_with_current_sdk() {
    build_guest_apps(&["pip"], "python-build-acceptance.json");
}

fn build_guest_apps(ecosystems: &[&str], report: &str) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let destination = workspace.join("target/arm64-cross/guest/apps");
    fs::create_dir_all(&destination).unwrap();
    for &ecosystem in ecosystems {
        assert!(
            !destination.join(ecosystem).exists(),
            "guest app already exists; preserve or remove the prior owned fixture explicitly"
        );
    }
    let author = TemporaryDirectory::new();
    let key = author_inputs(author.path(), &workspace);
    add_arm_inputs(author.path(), &workspace);
    add_target_sandbox(author.path(), &workspace);
    let machine = machine_temporary_directory(&workspace);
    let layout = Layout::new(machine.path());
    layout.initialize().unwrap();
    let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let release = layout.downloads().join(version.as_str());
    packaging::create(author.path(), &release).unwrap();
    let runtimes = sign_target_runtimes(&release, &workspace, &key, ecosystems);
    install_release(&layout, &version, &key);
    fs::remove_dir_all(&release).unwrap();
    drop(author);
    let compiler = layout.versions().join(version.as_str()).join("dever-core");
    let mut daemon = daemon::TestDaemon::start(&layout);
    let projects = TemporaryDirectory::new();
    let mut programs = serde_json::Map::new();
    for &ecosystem in ecosystems {
        let project = projects.path().join(ecosystem);
        let contract = application(&project, ecosystem);
        let lock = prepare_locked_application(
            &project,
            &workspace,
            &layout,
            contract,
            &runtimes[ecosystem],
        );
        command(
            Command::new(&compiler)
                .args(["lib", "doctor"])
                .arg(&project)
                .args(["--target", TARGET.platform()]),
        );
        let output = destination.join(ecosystem);
        fs::create_dir(&output).unwrap();
        write(&output, "config/setting.json", "{}\n");
        let executable = output.join("program");
        build(&compiler, &project, &executable, TARGET);
        programs.insert(ecosystem.into(), json!({
            "path":format!("{ecosystem}/program"), "sha256":sha256_file(&executable).unwrap(),
            "bytes":fs::metadata(&executable).unwrap().len(), "runtime":runtimes[ecosystem].pack,
            "libs":lock.libs.iter().map(|lib| lib.spec.key()).collect::<Vec<_>>(),
            "artifacts":lock.libs.iter().flat_map(|lib| &lib.artifacts).collect::<Vec<_>>()
        }));
    }
    assert_eq!(
        cache_status(&layout).unwrap().entries,
        ecosystems.len() as u64
    );
    daemon.stop();
    drop(projects);
    drop(machine);
    write(&workspace, &format!("target/arm64-cross/{report}"), serde_json::to_vec_pretty(&json!({
        "format":"dever-cross-ecosystem-build-v1", "target":TARGET.platform(), "programs":programs,
        "checks":{"signed_runtime_install":true,"offline_lib_doctor":ecosystems.len(),"actual_cli_arm_build":ecosystems.len(),"source_and_build_machine_removed":true},
        "guest_probe":{"arguments":["sample.worker.probe","{}"],"expected_data":true},
        "guest_failure":{"arguments":["sample.worker.reject","{}"],"expected":"runtime fault; next fresh probe still succeeds; no remaining Worker"},
        "guest_execution":"not_run", "sandbox_execution":"requires full ARM kernel guest"
    })).unwrap());
}
