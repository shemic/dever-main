#[path = "support/native_elf.rs"]
#[allow(
    dead_code,
    reason = "This test crate uses only the shared static ELF fixture"
)]
mod native_elf;
#[path = "../dever-tests/tests/support/sandbox.rs"]
mod sandbox;
#[path = "../dever-tests/tests/support/temp.rs"]
mod temp;
#[path = "support/wheel.rs"]
mod wheel_fixture;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use dever_cli::libs::{LibSpec, LockFile, LockedArtifact, LockedLib, LockedWorker, RuntimePack};
use dever_cli::workers;
use dever_core::native::EmbeddedResource;
use dever_core::source::SourceMap;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::json;
use sha2::{Digest, Sha256};
use temp::TemporaryDirectory;

#[path = "external_workers/command.rs"]
mod command;

#[cfg(unix)]
struct OwnedProcessGroup(std::process::Child);

#[cfg(unix)]
impl OwnedProcessGroup {
    fn stop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none()
            && let Some(pid) = rustix::process::Pid::from_raw(self.0.id() as i32)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        self.stop();
    }
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::none());
    let mut tar = tar::Builder::new(encoder);
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, name, *bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn wheel(metadata: &str) -> Vec<u8> {
    wheel_fixture::pack(
        "fixture",
        "1.0.0",
        &format!("{metadata}Tag: py3-none-any\n"),
        &wheel_fixture::metadata("fixture", "1.0.0", &[], &["extra"]),
        &[("fixture/__init__.py", b"value = 1\n".to_vec())],
    )
}

fn resource(path: String, bytes: Vec<u8>) -> EmbeddedResource {
    EmbeddedResource {
        sha256: sha(&bytes),
        path,
        bytes,
        executable: false,
    }
}

fn prepare_sandboxed_worker(
    root: &Path,
    program: &dever_core::hir::Program,
    mut resources: Vec<EmbeddedResource>,
) -> Vec<EmbeddedResource> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for asset in sandbox::assets(&workspace) {
        let mut file = resource(asset.path, asset.bytes);
        file.executable = asset.executable;
        resources.push(file);
    }
    let mut generated = workers::prepare(root, program, &resources).unwrap();
    generated.extend(
        resources
            .into_iter()
            .filter(|resource| resource.path.starts_with("sandbox/")),
    );
    generated
}

fn frame(value: serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(&value).unwrap();
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend(body);
    bytes
}

fn response(stream: &mut impl Read) -> Option<serde_json::Value> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).ok()?;
    let mut body = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn fixture(
    root: &Path,
    ecosystem: &str,
    binary: &[u8],
    package_metadata: Option<&str>,
) -> (dever_core::hir::Program, Vec<EmbeddedResource>) {
    let entry = match ecosystem {
        "npm" => "worker/main.mjs",
        "pip" => "worker/main.py",
        _ => panic!("unsupported fixture ecosystem"),
    };
    fixture_entry(root, ecosystem, binary, package_metadata, entry)
}

fn fixture_entry(
    root: &Path,
    ecosystem: &str,
    binary: &[u8],
    package_metadata: Option<&str>,
    entry: &str,
) -> (dever_core::hir::Program, Vec<EmbeddedResource>) {
    let (program, resources, _) =
        fixture_entry_with_sources(root, ecosystem, binary, package_metadata, entry);
    (program, resources)
}

fn fixture_entry_with_sources(
    root: &Path,
    ecosystem: &str,
    binary: &[u8],
    package_metadata: Option<&str>,
    entry: &str,
) -> (dever_core::hir::Program, Vec<EmbeddedResource>, SourceMap) {
    fixture_for_target(
        root,
        ecosystem,
        binary,
        package_metadata,
        entry,
        dever_cli::toolchain::BuildTarget::host().unwrap(),
    )
}

fn fixture_for_target(
    root: &Path,
    ecosystem: &str,
    binary: &[u8],
    package_metadata: Option<&str>,
    entry: &str,
    target: dever_cli::toolchain::BuildTarget,
) -> (dever_core::hir::Program, Vec<EmbeddedResource>, SourceMap) {
    let (executable, source) = match ecosystem {
        "npm" if entry.ends_with(".cjs") => (
            "bin/node",
            "module.exports = { send: ({ value }) => ({ receipt: value + 1 }) };\n",
        ),
        "npm" => (
            "bin/node",
            "export const send = ({ value }) => ({ receipt: value + 1 });\n",
        ),
        "pip" => (
            "bin/python3",
            "async def send(payload, setting):\n    return {'receipt': payload['value'] + 1}\n",
        ),
        _ => panic!("unsupported fixture ecosystem"),
    };
    let library = if package_metadata.is_some() {
        "lib \"fixture@1.0.0\""
    } else {
        ""
    };
    let sources = worker_sources(ecosystem, entry, library);
    let program = dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"));
    let contract = program.external_worker_contracts().remove(0);
    let entry_path = root.join(&contract.entry);
    fs::create_dir_all(entry_path.parent().unwrap()).unwrap();
    fs::write(entry_path, source).unwrap();
    let target = target.platform();
    let arguments = if ecosystem == "pip" {
        vec!["-I", "-S", "-B"]
    } else {
        vec![]
    };
    let mut metadata = json!({"format":"dever-worker-runtime-v1","ecosystem":ecosystem,"target":target,
        "executable":executable,"arguments":arguments});
    if ecosystem == "pip" {
        metadata["python_wheel_tags"] = json!(["py3-none-any"]);
        let architecture = target.strip_prefix("linux-").unwrap();
        metadata["python_extension_suffixes"] = json!([
            format!(".cpython-312-{architecture}-linux-gnu.so"),
            ".abi3.so",
            ".so"
        ]);
        let mut markers = serde_json::to_value(wheel_fixture::markers()).unwrap();
        markers["platform_machine"] = json!(architecture);
        metadata["python_markers"] = markers;
    }
    let pack = archive(&[
        (
            "dever-runtime.json",
            &serde_json::to_vec(&metadata).unwrap(),
        ),
        (executable, binary),
    ]);
    let runtime = RuntimePack {
        name: format!("fixture-{ecosystem}"),
        version: "1".into(),
        sha256: sha(&pack),
    };
    let mut archives = Vec::new();
    let mut libraries = Vec::new();
    if let Some(package_metadata) = package_metadata {
        let spec: LibSpec = format!("{ecosystem}:fixture@1.0.0").parse().unwrap();
        let (bytes, extension) = match ecosystem {
            "npm" => (
                archive(&[
                    ("package/package.json", package_metadata.as_bytes()),
                    ("package/index.js", b"export const value = 1;"),
                ]),
                "tgz",
            ),
            "pip" => (wheel(package_metadata), "whl"),
            _ => panic!("unsupported fixture package"),
        };
        let path = format!(
            "lib/{ecosystem}/{}/1.0.0/archive.{extension}",
            sha(b"fixture")
        );
        libraries.push(LockedLib {
            build: None,
            spec: spec.clone(),
            dependencies: vec![],
            runtime: runtime.clone(),
            artifacts: vec![LockedArtifact {
                target: target.into(),
                path: path.clone(),
                bytes: bytes.len() as u64,
                sha256: sha(&bytes),
            }],
            schema: contract.schema.clone(),
        });
        archives.push(resource(path, bytes));
    }
    let mut lock = LockFile::new(libraries).unwrap();
    if ecosystem == "npm" && package_metadata.is_some() {
        lock.npm.push(dever_cli::libs::npm::Environment::single(
            lock.libs[0].spec.clone(),
        ));
    }
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
            .map(|value| value.parse().unwrap())
            .collect(),
    });
    lock.write_atomic(root).unwrap();
    archives.push(resource(
        format!("lib/runtime/{ecosystem}/{target}/runtime.pack"),
        pack,
    ));
    (program, archives, sources)
}

fn worker_sources(ecosystem: &str, entry: &str, libraries: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add(
        "mail/send/port.dever",
        "send(value: Int) (receipt: Int) fails app.SendResult",
    );
    sources.add("mail/send/app.dever", "type SendResult { error Unavailable(message: Text) }\nsend(value: Int) (receipt: Int) { receipt = port.send(value) }");
    sources.add("mail/send/api.dever", "cmd send = app.send");
    sources.add(
        "mail/send/adapter.dever",
        format!("external {ecosystem} \"{entry}\" {{ {libraries} }}"),
    );
    sources
}

#[test]
fn packages_checked_javascript_worker_without_host_resolution() {
    let temp = TemporaryDirectory::new();
    let (program, resources) = fixture(temp.path(), "npm", &native_elf::static_executable(), None);
    let generated = workers::prepare(temp.path(), &program, &resources).unwrap();
    let entry = &program.external_worker_contracts()[0].entry;
    let launch = generated
        .iter()
        .find(|resource| resource.path == format!("{entry}.dever-worker.json"))
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&launch.bytes).unwrap();
    let executable = manifest["executable"].as_str().unwrap();
    let runner = manifest["arguments"].as_array().unwrap().last().unwrap()["resource"]
        .as_str()
        .unwrap();
    assert!(
        generated
            .iter()
            .any(|resource| resource.path == executable && resource.executable)
    );
    assert!(generated.iter().any(|resource| resource.path == runner));
    assert_eq!(manifest["entry"], entry.as_str());
    assert_eq!(manifest["ecosystem"], "npm");
    assert!(
        generated
            .iter()
            .any(|resource| resource.path.ends_with("/contract.json"))
    );

    assert!(
        workers::prepare(temp.path(), &program, &[])
            .unwrap_err()
            .contains("runtime pack is missing")
    );
}

#[test]
fn cross_target_interpreters_and_dependency_archives_remain_offline_and_target_bound() {
    use dever_cli::toolchain::BuildTarget;
    let target = BuildTarget::LinuxAarch64;
    let mut arm = native_elf::static_executable();
    arm[18..20].copy_from_slice(&183u16.to_le_bytes());
    for (ecosystem, entry, metadata) in [
        (
            "pip",
            "worker/main.py",
            "Wheel-Version: 1.0\nRoot-Is-Purelib: true\n",
        ),
        (
            "npm",
            "worker/main.mjs",
            r#"{"name":"fixture","version":"1.0.0"}"#,
        ),
    ] {
        let temp = TemporaryDirectory::new();
        let (program, resources, _) =
            fixture_for_target(temp.path(), ecosystem, &arm, Some(metadata), entry, target);
        let generated =
            workers::prepare_for_target(temp.path(), &program, &resources, target, &[]).unwrap();
        let binary = generated
            .iter()
            .find(|file| file.executable && file.path.contains("/runtime/bin/"))
            .unwrap();
        assert_eq!(
            goblin::elf::Elf::parse(&binary.bytes)
                .unwrap()
                .header
                .e_machine,
            183
        );
        assert!(
            generated
                .iter()
                .any(|file| file.path.contains(if ecosystem == "pip" {
                    "site-packages/fixture/__init__.py"
                } else {
                    "node_modules/fixture/index.js"
                }))
        );
        assert!(
            workers::prepare_for_target(
                temp.path(),
                &program,
                &resources,
                BuildTarget::LinuxX86_64,
                &[]
            )
            .unwrap_err()
            .contains("runtime pack is missing")
        );

        let mut lock =
            LockFile::decode(&fs::read(temp.path().join("dever.lock")).unwrap()).unwrap();
        lock.libs[0].artifacts[0].target = "linux-x86_64".into();
        lock.write_atomic(temp.path()).unwrap();
        assert!(
            workers::prepare_for_target(temp.path(), &program, &resources, target, &[])
                .unwrap_err()
                .contains("archive for linux-aarch64 is missing")
        );
    }
}

#[test]
fn cross_target_runtime_rejects_host_binary_even_with_matching_pack_hash() {
    use dever_cli::toolchain::BuildTarget;
    let mut host = native_elf::static_executable();
    host[18..20].copy_from_slice(&62u16.to_le_bytes());
    for (ecosystem, entry) in [("pip", "worker/main.py"), ("npm", "worker/main.mjs")] {
        let temp = TemporaryDirectory::new();
        let (program, resources, _) = fixture_for_target(
            temp.path(),
            ecosystem,
            &host,
            None,
            entry,
            BuildTarget::LinuxAarch64,
        );
        assert!(
            workers::prepare_for_target(
                temp.path(),
                &program,
                &resources,
                BuildTarget::LinuxAarch64,
                &[]
            )
            .unwrap_err()
            .contains("differs from target linux-aarch64")
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires explicit Go runtime pack, official UUID proof/archive and sandbox assets"]
fn compiles_go_worker_with_locked_tools_and_runs_without_source_or_go() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TemporaryDirectory::new();
    let mut sources = SourceMap::default();
    sources.add(
        "mail/send/port.dever",
        "send(value: Int) (receipt: Int) fails app.SendResult",
    );
    sources.add("mail/send/app.dever", "type SendResult { error Unavailable(message: Text) }\nsend(value: Int) (receipt: Int) { receipt = port.send(value) }");
    sources.add("mail/send/api.dever", "cmd send = app.send");
    sources.add(
        "mail/send/adapter.dever",
        "external go \"worker/main.go\" { lib \"github.com/google/uuid@1.6.0\" }",
    );
    let program = dever_core::check(&sources).unwrap();
    let contract = program.external_worker_contracts().remove(0);
    let entry = temp.path().join(&contract.entry);
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::write(&entry, r#"package main
import (
    "context"
    "encoding/json"
    _ "embed"
    "github.com/google/uuid"
)
//go:embed note.txt
var note string
func Send(_ context.Context, payload any, _ any) (any, error) {
    value := payload.(map[string]any)["value"].(json.Number)
    number, err := value.Int64()
    if err != nil { return nil, err }
    parsed, err := uuid.Parse("00000000-0000-0000-0000-000000000001")
    if err != nil { return nil, err }
    return map[string]any{"receipt": number + int64(parsed[15]) + TargetBonus() + int64(len(note)) - 1}, nil
}
"#).unwrap();
    fs::write(entry.with_file_name("note.txt"), "x").unwrap();
    fs::write(
        entry.with_file_name("target_linux.go"),
        "//go:build linux\npackage main\nfunc TargetBonus() int64 { return 0 }\n",
    )
    .unwrap();
    fs::write(
        entry.with_file_name("target_windows.go"),
        "//go:build windows\npackage main\nfunc TargetBonus() int64 { return 100 }\n",
    )
    .unwrap();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let prepared = workspace.join("target/go-managed");
    let pack = fs::read(prepared.join("runtime.pack")).unwrap();
    let target = dever_cli::toolchain::platform_identity();
    let mut lock =
        LockFile::decode(&fs::read(prepared.join("dependency/dever.lock")).unwrap()).unwrap();
    assert_eq!(lock.libs.len(), 1);
    let dependency = &mut lock.libs[0];
    assert_eq!(dependency.spec.key(), "go:github.com/google/uuid@1.6.0");
    assert_eq!(dependency.runtime.sha256, sha(&pack));
    dependency.schema = contract.schema.clone();
    let spec = dependency.spec.clone();
    let runtime = dependency.runtime.clone();
    let mut resources = vec![resource(
        format!("lib/runtime/go/{target}/runtime.pack"),
        pack,
    )];
    for artifact in &dependency.artifacts {
        let bytes = fs::read(prepared.join("dependency/artifacts").join(&artifact.sha256)).unwrap();
        assert_eq!(sha(&bytes), artifact.sha256);
        resources.push(resource(artifact.path.clone(), bytes));
    }
    for asset in sandbox::assets(&workspace) {
        let mut file = resource(asset.path, asset.bytes);
        file.executable = asset.executable;
        resources.push(file);
    }
    lock.workers.push(LockedWorker {
        port: contract.port.clone(),
        adapter: contract.adapter.clone(),
        ecosystem: "go".into(),
        runtime: Some(runtime),
        entry: contract.entry.clone(),
        schema: contract.schema.clone(),
        capabilities: contract.capabilities.clone(),
        operations: contract.operations.clone(),
        libs: vec![spec],
    });
    lock.write_atomic(temp.path()).unwrap();
    let generated = workers::prepare(temp.path(), &program, &resources).unwrap();
    assert!(
        !generated
            .iter()
            .any(|resource| resource.path.contains("/source/")
                || resource.path.contains("/runtime/"))
    );
    let binary = generated
        .iter()
        .find(|resource| resource.path.ends_with("/worker") && resource.executable)
        .unwrap();
    let rebuilt = workers::prepare(temp.path(), &program, &resources).unwrap();
    let identities = |files: &[EmbeddedResource]| {
        files
            .iter()
            .map(|file| (file.path.clone(), file.sha256.clone(), file.executable))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        identities(&generated),
        identities(&rebuilt),
        "fresh Go build directories must produce identical Worker resources"
    );
    let working_directory = temp
        .path()
        .join(binary.path.strip_suffix("/worker").unwrap());
    for file in generated.iter().chain(
        resources
            .iter()
            .filter(|file| file.path.starts_with("sandbox/")),
    ) {
        let path = temp.path().join(&file.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &file.bytes).unwrap();
        if file.executable {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    fs::remove_dir_all(entry.parent().unwrap()).unwrap();
    let child = dever_sandbox::Launch {
        assets: &temp.path().join("sandbox"),
        worker: &working_directory,
        executable: &temp.path().join(&binary.path),
        arguments: &[],
        working_directory: &working_directory,
        capabilities: dever_sandbox::Capabilities::default(),
        grants: &[],
    }
    .command()
    .unwrap()
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    assert_fixture_protocol(child, &contract);
}

#[test]
fn packages_zero_lib_python_worker_with_checked_manifest() {
    let temp = TemporaryDirectory::new();
    let (program, resources) = fixture(temp.path(), "pip", &native_elf::static_executable(), None);
    let generated = workers::prepare(temp.path(), &program, &resources).unwrap();
    let contract = program.external_worker_contracts().remove(0);
    let launch = generated
        .iter()
        .find(|resource| resource.path == format!("{}.dever-worker.json", contract.entry))
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&launch.bytes).unwrap();
    assert_eq!(manifest["ecosystem"], "pip");
    assert!(
        manifest["executable"]
            .as_str()
            .unwrap()
            .ends_with("/runtime/bin/python3")
    );
    assert!(
        generated
            .iter()
            .any(|resource| resource.path.ends_with("/dever_component.py"))
    );
    assert!(
        generated
            .iter()
            .any(|resource| resource.path.ends_with("/runner.py"))
    );
}

#[test]
fn installs_locked_npm_package_and_rejects_lifecycle_hooks() {
    let pure = TemporaryDirectory::new();
    let (program, resources) = fixture(
        pure.path(),
        "npm",
        &native_elf::static_executable(),
        Some(r#"{"name":"fixture","version":"1.0.0","type":"module"}"#),
    );
    let generated = workers::prepare(pure.path(), &program, &resources).unwrap();
    assert!(generated.iter().any(|resource| {
        resource
            .path
            .ends_with("/node_modules/fixture/package.json")
    }));
    assert!(
        generated
            .iter()
            .any(|resource| resource.path.ends_with("/node_modules/fixture/index.js"))
    );

    let hooked = TemporaryDirectory::new();
    let (program, resources) = fixture(
        hooked.path(),
        "npm",
        &native_elf::static_executable(),
        Some(r#"{"name":"fixture","version":"1.0.0","scripts":{"install":"node install.js"}}"#),
    );
    assert!(
        workers::prepare(hooked.path(), &program, &resources)
            .unwrap_err()
            .contains("lifecycle scripts")
    );

    let collision = TemporaryDirectory::new();
    let (program, mut resources) = fixture(
        collision.path(),
        "npm",
        &native_elf::static_executable(),
        Some(r#"{"name":"fixture","version":"1.0.0"}"#),
    );
    let archive_resource = resources
        .iter_mut()
        .find(|resource| resource.path.ends_with("archive.tgz"))
        .unwrap();
    *archive_resource = resource(
        archive_resource.path.clone(),
        archive(&[
            (
                "package/package.json",
                br#"{"name":"fixture","version":"1.0.0"}"#,
            ),
            ("package/overlap", b"file"),
            ("package/overlap/child.js", b"file"),
        ]),
    );
    let mut lock =
        LockFile::decode(&fs::read(collision.path().join("dever.lock")).unwrap()).unwrap();
    lock.libs[0].artifacts[0].sha256 = archive_resource.sha256.clone();
    lock.libs[0].artifacts[0].bytes = archive_resource.bytes.len() as u64;
    lock.write_atomic(collision.path()).unwrap();
    assert!(
        workers::prepare(collision.path(), &program, &resources)
            .unwrap_err()
            .contains("file/directory conflict")
    );
}

#[test]
fn installs_purelib_and_platlib_python_wheels() {
    let pure = TemporaryDirectory::new();
    let (program, resources) = fixture(
        pure.path(),
        "pip",
        &native_elf::static_executable(),
        Some("Wheel-Version: 1.0\nRoot-Is-Purelib: true\n"),
    );
    let generated = workers::prepare(pure.path(), &program, &resources).unwrap();
    assert!(generated.iter().any(|resource| {
        resource
            .path
            .ends_with("/site-packages/fixture/__init__.py")
    }));

    let platform = TemporaryDirectory::new();
    let (program, resources) = fixture(
        platform.path(),
        "pip",
        &native_elf::static_executable(),
        Some("Wheel-Version: 1.0\nRoot-Is-Purelib: false\n"),
    );
    assert!(
        workers::prepare(platform.path(), &program, &resources)
            .unwrap()
            .iter()
            .any(|resource| resource
                .path
                .ends_with("/site-packages/fixture/__init__.py"))
    );
}

#[test]
#[ignore = "requires the explicit official native-wheel and complete Python runtime fixture at target/python-managed"]
fn runs_official_native_python_wheel_in_sandbox() {
    run_native_python_fixture("python-managed");
}

#[test]
#[ignore = "requires explicitly prepared official PEP517 source build at target/source-build"]
fn runs_official_python_source_build_in_sandbox() {
    run_native_python_fixture("source-build");
}

fn run_native_python_fixture(fixture: &str) {
    let temp = TemporaryDirectory::new();
    let sources = worker_sources(
        "pip",
        "worker/main.py",
        "lib \"simplejson@3.20.1\"\nlib \"prefixfixture@1.0.0\"",
    );
    let program = dever_core::check(&sources).unwrap();
    let contract = program.external_worker_contracts().remove(0);
    let entry = temp.path().join(&contract.entry);
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    fs::write(&entry, "import simplejson\nimport prefixfixture\nfrom simplejson import _speedups\nassert _speedups.__file__.endswith('.so')\nassert prefixfixture.VALUE == 'prefix-ok'\nasync def send(payload, setting):\n    value = simplejson.loads(simplejson.dumps(payload))\n    return {'receipt': value['value'] + 1}\n").unwrap();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let prepared = workspace.join("target").join(fixture);
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(prepared.join("config/setting.json")).unwrap()).unwrap();
    let runtime_file = config
        .get("runtime_file")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("runtime.pack");
    let pack = fs::read(prepared.join(runtime_file)).unwrap();
    let mut lock =
        LockFile::decode(&fs::read(prepared.join("dependency/dever.lock")).unwrap()).unwrap();
    assert_eq!(lock.libs[0].spec.key(), "pip:simplejson@3.20.1");
    assert_eq!(lock.libs[0].runtime.sha256, sha(&pack));
    let runtime = lock.libs[0].runtime.clone();
    let mut resources = vec![resource(
        format!(
            "lib/runtime/pip/{}/runtime.pack",
            dever_cli::toolchain::platform_identity()
        ),
        pack,
    )];
    for library in &mut lock.libs {
        library.schema = contract.schema.clone();
        for artifact in &library.artifacts {
            let bytes =
                fs::read(prepared.join("dependency/artifacts").join(&artifact.sha256)).unwrap();
            assert_eq!(sha(&bytes), artifact.sha256);
            resources.push(resource(artifact.path.clone(), bytes));
        }
    }
    let probe = wheel_fixture::pack("prefixfixture", "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        &wheel_fixture::metadata("prefixfixture", "1.0.0", &[], &[]), &[
            ("prefixfixture/__init__.py", b"import sys, sysconfig\nfrom pathlib import Path\nassert sysconfig.get_path('data') == sys.prefix\nVALUE = (Path(sys.prefix) / 'share/prefixfixture/message.txt').read_text()\n".to_vec()),
            ("prefixfixture-1.0.0.data/data/share/prefixfixture/message.txt", b"prefix-ok".to_vec()),
            ("prefixfixture-1.0.0.data/scripts/prefix-probe", b"#!python\nimport prefixfixture\nprint(prefixfixture.VALUE)\n".to_vec()),
        ]);
    let probe_path = format!("lib/pip/{}/1.0.0/archive.whl", sha(b"prefixfixture"));
    lock.libs.push(LockedLib {
        build: None,
        spec: "pip:prefixfixture@1.0.0".parse().unwrap(),
        dependencies: vec![],
        runtime: runtime.clone(),
        artifacts: vec![LockedArtifact {
            target: dever_cli::toolchain::platform_identity(),
            path: probe_path.clone(),
            bytes: probe.len() as u64,
            sha256: sha(&probe),
        }],
        schema: contract.schema.clone(),
    });
    resources.push(resource(probe_path, probe));
    lock.workers.push(LockedWorker {
        port: contract.port.clone(),
        adapter: contract.adapter.clone(),
        ecosystem: "pip".into(),
        runtime: Some(runtime),
        entry: contract.entry.clone(),
        schema: contract.schema.clone(),
        capabilities: contract.capabilities.clone(),
        operations: contract.operations.clone(),
        libs: contract
            .libs
            .iter()
            .map(|spec| spec.parse().unwrap())
            .collect(),
    });
    lock.write_atomic(temp.path()).unwrap();
    let generated = prepare_sandboxed_worker(temp.path(), &program, resources);
    assert!(
        generated
            .iter()
            .any(|resource| resource.path.contains("_speedups.")
                && resource.bytes.starts_with(b"\x7fELF"))
    );
    let (deployment, launch) = run_packaged_protocol(&temp, &contract, "pip", &generated);
    let output = dever_sandbox::Launch {
        assets: &deployment.join("sandbox"),
        worker: &launch.working_directory,
        executable: &launch.working_directory.join("runtime/bin/prefix-probe"),
        arguments: &[],
        working_directory: &launch.working_directory,
        capabilities: dever_sandbox::Capabilities::default(),
        grants: &[],
    }
    .command()
    .unwrap()
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "standalone wheel script failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"prefix-ok\n");
}

fn run_packaged_protocol(
    temp: &TemporaryDirectory,
    contract: &dever_core::hir::ExternalWorkerContract,
    ecosystem: &str,
    generated: &[EmbeddedResource],
) -> (PathBuf, dever_runtime::external::WorkerLaunch) {
    let borrowed = generated
        .iter()
        .map(|resource| dever_runtime::external::Resource {
            path: &resource.path,
            bytes: &resource.bytes,
            sha256: &resource.sha256,
            executable: resource.executable,
        })
        .collect::<Vec<_>>();
    let deployment = dever_runtime::external::extract(
        temp.path(),
        &dever_runtime::external::bundle_digest(&borrowed),
        &borrowed,
    )
    .unwrap();
    let launch = dever_runtime::external::verified_worker_launch(
        &deployment,
        &borrowed,
        &contract.entry,
        ecosystem,
    )
    .unwrap();
    let child = dever_sandbox::Launch {
        assets: &deployment.join("sandbox"),
        worker: &launch.working_directory,
        executable: &launch.executable,
        arguments: &launch.arguments,
        working_directory: &launch.working_directory,
        capabilities: dever_sandbox::Capabilities::default(),
        grants: &[],
    }
    .command()
    .unwrap()
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    assert_fixture_protocol(child, contract);
    (deployment, launch)
}

#[path = "external_workers/npm.rs"]
mod npm_native;

#[test]
fn installs_one_wheel_for_base_and_extra_variants() {
    let temp = TemporaryDirectory::new();
    let (_, resources) = fixture_entry(
        temp.path(),
        "pip",
        &native_elf::static_executable(),
        Some("Wheel-Version: 1.0\nRoot-Is-Purelib: true\n"),
        "worker/main.py",
    );
    let sources = worker_sources(
        "pip",
        "worker/main.py",
        "lib \"fixture@1.0.0\"\nlib \"fixture[extra]@1.0.0\"",
    );
    let program = dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"));
    let contract = program.external_worker_contracts().remove(0);
    let mut lock = LockFile::decode(&fs::read(temp.path().join("dever.lock")).unwrap()).unwrap();
    let mut extra = lock.libs[0].clone();
    extra.spec = "pip:fixture[extra]@1.0.0".parse().unwrap();
    lock.libs.push(extra);
    lock.workers[0].schema = contract.schema.clone();
    for library in &mut lock.libs {
        library.schema = contract.schema.clone();
    }
    lock.workers[0].libs = contract
        .libs
        .iter()
        .map(|spec| spec.parse().unwrap())
        .collect();
    lock.write_atomic(temp.path()).unwrap();
    let generated = workers::prepare(temp.path(), &program, &resources).unwrap();
    assert_eq!(
        generated
            .iter()
            .filter(|resource| resource
                .path
                .ends_with("/site-packages/fixture/__init__.py"))
            .count(),
        1
    );

    lock.libs[1].artifacts[0].sha256 = "0".repeat(64);
    lock.write_atomic(temp.path()).unwrap();
    assert!(workers::prepare(temp.path(), &program, &resources).is_err());
}

#[test]
fn rejects_runtime_identity_and_adapter_source_links() {
    let temp = TemporaryDirectory::new();
    let (program, mut resources) =
        fixture(temp.path(), "npm", &native_elf::static_executable(), None);
    let target = dever_cli::toolchain::platform_identity();
    let invalid = json!({"format":"dever-worker-runtime-v1","ecosystem":"pip","target":target,
        "executable":"bin/node","arguments":[]});
    let pack = archive(&[
        ("dever-runtime.json", &serde_json::to_vec(&invalid).unwrap()),
        ("bin/node", &native_elf::static_executable()),
    ]);
    resources[0] = resource(resources[0].path.clone(), pack);
    let mut lock = LockFile::decode(&fs::read(temp.path().join("dever.lock")).unwrap()).unwrap();
    lock.workers[0].runtime.as_mut().unwrap().sha256 = resources[0].sha256.clone();
    lock.write_atomic(temp.path()).unwrap();
    assert!(
        workers::prepare(temp.path(), &program, &resources)
            .unwrap_err()
            .contains("runtime pack identity")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let linked = TemporaryDirectory::new();
        let (linked_program, linked_resources) =
            fixture(linked.path(), "npm", &native_elf::static_executable(), None);
        let entry = &linked_program.external_worker_contracts()[0].entry;
        symlink(
            linked.path().join(entry),
            linked.path().join(entry).with_file_name("helper.mjs"),
        )
        .unwrap();
        assert!(
            workers::prepare(linked.path(), &linked_program, &linked_resources)
                .unwrap_err()
                .contains("symbolic link")
        );
    }
}

#[test]
fn rejects_python_runtime_without_isolation_flags() {
    let temp = TemporaryDirectory::new();
    let (program, mut resources) =
        fixture(temp.path(), "pip", &native_elf::static_executable(), None);
    let target = dever_cli::toolchain::platform_identity();
    let metadata = json!({"format":"dever-worker-runtime-v1","ecosystem":"pip","target":target,
        "executable":"bin/python3","arguments":[]});
    let pack = archive(&[
        (
            "dever-runtime.json",
            &serde_json::to_vec(&metadata).unwrap(),
        ),
        ("bin/python3", &native_elf::static_executable()),
    ]);
    resources[0] = resource(resources[0].path.clone(), pack);
    let mut lock = LockFile::decode(&fs::read(temp.path().join("dever.lock")).unwrap()).unwrap();
    lock.workers[0].runtime.as_mut().unwrap().sha256 = resources[0].sha256.clone();
    lock.write_atomic(temp.path()).unwrap();
    assert!(
        workers::prepare(temp.path(), &program, &resources)
            .unwrap_err()
            .contains("-I -S -B")
    );
}

#[test]
#[ignore = "requires the explicit host Node fixture path"]
fn runs_javascript_runner_from_explicit_fixture_runtime_path() {
    run_fixture_runtime("npm", "/root/.nvm/versions/node/v24.15.0/bin/node");
}

#[test]
#[ignore = "requires the explicit host Python fixture path"]
fn runs_python_runner_from_explicit_fixture_runtime_path() {
    run_fixture_runtime("pip", "/usr/bin/python3.12");
}

#[test]
#[ignore = "requires the explicit host Node fixture path"]
fn binds_commonjs_exports_without_a_registration_table() {
    let temp = TemporaryDirectory::new();
    let (program, resources) = fixture_entry(
        temp.path(),
        "npm",
        &native_elf::static_executable(),
        None,
        "worker/main.cjs",
    );
    execute_generated_fixture(
        &temp,
        &program,
        &resources,
        Some("/root/.nvm/versions/node/v24.15.0/bin/node"),
    );
}

#[test]
#[ignore = "requires the explicit host Node fixture path"]
fn binds_esm_exports_without_a_registration_table() {
    let temp = TemporaryDirectory::new();
    let (program, resources) = fixture(temp.path(), "npm", &native_elf::static_executable(), None);
    execute_generated_fixture(
        &temp,
        &program,
        &resources,
        Some("/root/.nvm/versions/node/v24.15.0/bin/node"),
    );
}

#[test]
#[ignore = "requires the explicit host Python fixture path"]
fn imports_packaged_sdk_before_adapter_modules() {
    let temp = TemporaryDirectory::new();
    let (program, resources) = fixture(temp.path(), "pip", &native_elf::static_executable(), None);
    let entry = &program.external_worker_contracts()[0].entry;
    fs::write(
        temp.path().join(entry).with_file_name("dever_component.py"),
        "raise RuntimeError('shadowed SDK')\n",
    )
    .unwrap();
    execute_generated_fixture(&temp, &program, &resources, Some("/usr/bin/python3.12"));
}

#[cfg(unix)]
#[test]
#[ignore = "requires the explicit host Node fixture and native Rust compiler"]
fn compiled_managed_worker_runs_after_source_and_pack_are_removed() {
    use std::os::unix::process::CommandExt;

    let temp = TemporaryDirectory::new();
    let binary = fs::read("/root/.nvm/versions/node/v24.15.0/bin/node").unwrap();
    let (program, resources, sources) =
        fixture_entry_with_sources(temp.path(), "npm", &binary, None, "worker/main.mjs");
    let source_entry = program.external_worker_contracts()[0].entry.clone();
    let raw_pack_path = temp.path().join("runtime.pack");
    fs::write(&raw_pack_path, &resources[0].bytes).unwrap();
    let raw = resource(resources[0].path.clone(), fs::read(&raw_pack_path).unwrap());
    let packaged = prepare_sandboxed_worker(temp.path(), &program, vec![raw]);
    let native = dever_core::native::compile_project_with_resources(
        &program,
        &sources,
        std::ffi::OsStr::new("/root/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc"),
        dever_runtime::config::RuntimeProfile::default(),
        &packaged,
    )
    .unwrap();
    let executable = temp.path().join("program");
    native.save(&executable).unwrap();
    drop(native);
    drop(sources);
    drop(program);
    drop(resources);
    fs::remove_file(temp.path().join(source_entry)).unwrap();
    fs::remove_file(temp.path().join("dever.lock")).unwrap();
    fs::remove_file(raw_pack_path).unwrap();
    fs::create_dir(temp.path().join("config")).unwrap();
    fs::write(
        temp.path().join("config/setting.json"),
        r#"{"adapter":{"mail.send":{"use":"default"}}}"#,
    )
    .unwrap();

    let child = Command::new(&executable)
        .process_group(0)
        .current_dir(temp.path())
        .env_clear()
        .args(["mail.send.send", "{\"value\":7}"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut owned = OwnedProcessGroup(child);
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = owned.0.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    if status.is_none() {
        owned.stop();
    }
    let mut stdout = Vec::new();
    let mut stderr = String::new();
    owned
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    owned
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        status.is_some_and(|status| status.success()),
        "native managed Worker failed or timed out: {stderr}"
    );
    let response: serde_json::Value = serde_json::from_slice(&stdout).unwrap_or_else(|error| {
        panic!(
            "invalid CMD response: {error}; stdout={:?}; stderr={stderr}",
            String::from_utf8_lossy(&stdout)
        )
    });
    assert_eq!(response["code"], 0, "{response}; stderr={stderr}");
    assert_eq!(response["data"], 8, "{response}; stderr={stderr}");
}

fn run_fixture_runtime(ecosystem: &str, binary_path: &str) {
    let temp = TemporaryDirectory::new();
    let binary = fs::read(binary_path).unwrap();
    let (program, resources) = fixture(temp.path(), ecosystem, &binary, None);
    execute_generated_fixture(&temp, &program, &resources, None);
}

fn execute_generated_fixture(
    temp: &TemporaryDirectory,
    program: &dever_core::hir::Program,
    resources: &[EmbeddedResource],
    host_runtime: Option<&str>,
) {
    let generated = workers::prepare(temp.path(), program, resources).unwrap();
    let entry = &program.external_worker_contracts()[0].entry;
    let launch = generated
        .iter()
        .find(|resource| resource.path == format!("{entry}.dever-worker.json"))
        .unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&launch.bytes).unwrap();
    let executable = manifest["executable"].as_str().unwrap();
    let runner = manifest["arguments"].as_array().unwrap().last().unwrap()["resource"]
        .as_str()
        .unwrap();

    for resource in &generated {
        let path = temp.path().join(&resource.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &resource.bytes).unwrap();
        #[cfg(unix)]
        if resource.executable {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let contract = program.external_worker_contracts().remove(0);
    let runtime_path = host_runtime
        .map(PathBuf::from)
        .unwrap_or_else(|| temp.path().join(executable));
    let mut command = Command::new(runtime_path);
    for flag in manifest["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .take(manifest["arguments"].as_array().unwrap().len() - 1)
    {
        command.arg(flag["literal"].as_str().unwrap());
    }
    let child = command
        .arg(temp.path().join(runner))
        .arg("--dever-component")
        .current_dir(
            temp.path()
                .join(manifest["working_directory"].as_str().unwrap()),
        )
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_fixture_protocol(child, &contract);
}

fn assert_fixture_protocol(
    mut child: std::process::Child,
    contract: &dever_core::hir::ExternalWorkerContract,
) {
    let hello = json!({"kind":"hello","version":"dever-component-1","port":contract.port,
        "adapter":contract.adapter,"schema":contract.schema,"operations":contract.operations,
        "capabilities":contract.capabilities,"setting":null});
    let (sender, receiver) = mpsc::channel();
    let mut stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        while let Some(message) = response(&mut stdout) {
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    let exchange = (|| -> Result<Vec<serde_json::Value>, String> {
        let mut messages = Vec::new();
        for request in [
            hello,
            json!({"kind":"call","id":1,"operation":"send","payload":{"value":7}}),
            json!({"kind":"shutdown"}),
        ] {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&frame(request))
                .map_err(|error| error.to_string())?;
            let message = receiver
                .recv_timeout(Duration::from_secs(10))
                .map_err(|error| error.to_string())?;
            messages.push(message);
        }
        Ok(messages)
    })();
    drop(child.stdin.take());
    if exchange.is_err() {
        let _ = child.kill();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("managed fixture Worker did not stop");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut stderr = String::new();
    reader.join().unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "{stderr}");
    let messages = exchange.unwrap_or_else(|error| panic!("{error}; stderr={stderr}"));
    assert_eq!(messages.len(), 3, "{messages:?}; stderr={stderr}");
    assert_eq!(messages[0]["kind"], "ready");
    assert_eq!(
        messages[1],
        json!({"kind":"result","id":1,"payload":{"receipt":8}})
    );
    assert_eq!(messages[2], json!({"kind":"shutdown"}));
}
