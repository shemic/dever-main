use super::*;
use dever_cli::libs::build::{Inputs, Session, pack};
use serde::{Deserialize, Serialize};

#[path = "../../dever-tests/tests/support/sandbox.rs"]
pub(super) mod sandbox;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    runtime: RegistryRuntime,
    runtime_file: String,
    build: pack::Descriptor,
    build_file: String,
}

struct AuthorInputs {
    root: PathBuf,
    settings: Settings,
}

#[test]
fn signed_build_pack_binds_runtime_layout_tools_and_archive_digest() {
    let runtime = registry_runtime(Ecosystem::Pip);
    let mut descriptor = pack::Descriptor {
        format: "dever-registry-build-v1".into(),
        ecosystem: Ecosystem::Pip,
        target: runtime.target.clone(),
        runtime_sha256: runtime.pack.sha256.clone(),
        python_runtime_sha256: None,
        sha256: "0".repeat(64),
    };
    descriptor.validate(&runtime, None).unwrap();
    let manifest = pack::Manifest {
        format: "dever-lib-build-v1".into(),
        ecosystem: Ecosystem::Pip,
        target: runtime.target.clone(),
        runtime_sha256: runtime.pack.sha256.clone(),
        python_runtime_sha256: None,
        executables: ["sh", "cc", "c++", "as", "ld", "ar", "ranlib", "make"]
            .into_iter()
            .map(|name| format!("rootfs/usr/bin/{name}"))
            .collect(),
    };
    let mut files = manifest
        .executables
        .iter()
        .map(|path| (path.clone(), b"#!/bin/sh\nexit 0\n".to_vec()))
        .collect::<Vec<_>>();
    files.push((
        "dever-build.json".into(),
        serde_json::to_vec(&manifest).unwrap(),
    ));
    manifest.validate(&files, &descriptor).unwrap();
    let mut changed = descriptor.clone();
    changed.runtime_sha256 = "f".repeat(64);
    assert!(changed.validate(&runtime, None).is_err());
    assert!(manifest.validate(&files, &changed).is_err());
    let mut changed = files.clone();
    changed.push(("rootfs/etc/ld.so.preload".into(), b"injected".to_vec()));
    assert!(manifest.validate(&changed, &descriptor).is_err());
    let mut changed = manifest.clone();
    changed.executables.push("rootfs/usr/bin/absent".into());
    assert!(changed.validate(&files, &descriptor).is_err());
    let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, path, bytes.as_slice())
            .unwrap();
    }
    let bytes = archive.into_inner().unwrap().finish().unwrap();
    descriptor.sha256 = digest(&bytes);
    pack::unpack(&bytes, &descriptor).unwrap();
    let mut changed = bytes;
    changed[0] ^= 1;
    assert!(pack::unpack(&changed, &descriptor).is_err());
}

impl Inputs for AuthorInputs {
    fn diagnostic(&self, output: &[u8]) {
        eprintln!("{}", String::from_utf8_lossy(output));
    }

    fn runtime(&self, ecosystem: Ecosystem) -> Result<(RegistryRuntime, Vec<u8>), String> {
        if ecosystem != Ecosystem::Pip {
            return Err("fixture only supplies Python".into());
        }
        let bytes = fs::read(self.root.join(&self.settings.runtime_file))
            .map_err(|error| error.to_string())?;
        if digest(&bytes) != self.settings.runtime.pack.sha256 {
            return Err("author runtime drift".into());
        }
        Ok((self.settings.runtime.clone(), bytes))
    }
    fn tools(&self, ecosystem: Ecosystem) -> Result<(pack::Descriptor, Vec<u8>), String> {
        if ecosystem != Ecosystem::Pip {
            return Err("fixture only supplies Python".into());
        }
        Ok((
            self.settings.build.clone(),
            fs::read(self.root.join(&self.settings.build_file))
                .map_err(|error| error.to_string())?,
        ))
    }
    fn assets(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        Ok(sandbox::assets(&workspace)
            .into_iter()
            .map(|asset| {
                let _mode = asset.executable;
                (
                    asset.path.trim_start_matches("sandbox/").into(),
                    asset.bytes,
                )
            })
            .collect())
    }
}

#[test]
#[ignore = "explicit author operation: assembles previously hashed native-build-inputs; no network or host execution"]
fn prepare_source_build_tool_fixture() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime: RegistryRuntime = serde_json::from_value(
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(workspace.join("target/python-managed/config/setting.json")).unwrap(),
        )
        .unwrap()["runtime"]
            .clone(),
    )
    .unwrap();
    let bytes = fs::read(workspace.join("target/python-managed/runtime.pack")).unwrap();
    assert_eq!(digest(&bytes), runtime.pack.sha256);
    drop(bytes);
    let root = workspace.join("target/source-build");
    let descriptor = prepare_tool_pack(Ecosystem::Pip, &runtime, None, &root);
    fs::write(
        root.join("config/setting.json"),
        serde_json::to_vec_pretty(&Settings {
            runtime,
            runtime_file: "../python-managed/runtime.pack".into(),
            build: descriptor,
            build_file: "build.pack".into(),
        })
        .unwrap(),
    )
    .unwrap();
}

pub(super) fn prepare_tool_pack(
    ecosystem: Ecosystem,
    runtime: &RegistryRuntime,
    python: Option<&RegistryRuntime>,
    root: &std::path::Path,
) -> pack::Descriptor {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let inputs = workspace.join("target/native-build-inputs");
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(inputs.join("inputs.json")).unwrap()).unwrap();
    let mut descriptor = pack::Descriptor {
        format: "dever-registry-build-v1".into(),
        ecosystem: ecosystem.clone(),
        target: runtime.target.clone(),
        runtime_sha256: runtime.pack.sha256.clone(),
        python_runtime_sha256: python.map(|runtime| runtime.pack.sha256.clone()),
        sha256: "0".repeat(64),
    };
    descriptor.validate(runtime, python).unwrap();
    let manifest = pack::Manifest {
        format: "dever-lib-build-v1".into(),
        ecosystem: ecosystem.clone(),
        target: runtime.target.clone(),
        runtime_sha256: runtime.pack.sha256.clone(),
        python_runtime_sha256: descriptor.python_runtime_sha256.clone(),
        executables: serde_json::from_value(document[ecosystem.as_str()]["executables"].clone())
            .unwrap(),
    };
    let mut files: Vec<(String, Vec<u8>)> = vec![(
        "dever-build.json".into(),
        serde_json::to_vec(&manifest).unwrap(),
    )];
    for input in document[ecosystem.as_str()]["files"].as_array().unwrap() {
        let source = inputs.join(input["source"].as_str().unwrap());
        assert!(fs::symlink_metadata(&source).unwrap().is_file());
        let bytes = fs::read(&source).unwrap();
        assert_eq!(digest(&bytes), input["sha256"].as_str().unwrap());
        files.push((input["path"].as_str().unwrap().into(), bytes));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    manifest.validate(&files, &descriptor).unwrap();
    let assets = sandbox::assets(&workspace)
        .into_iter()
        .map(|asset| {
            (
                asset.path.trim_start_matches("sandbox/").into(),
                asset.bytes,
            )
        })
        .collect::<Vec<_>>();
    pack::validate_tools(&manifest, &files, &assets).unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    let encoder = flate2::GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(
            fs::File::create(root.join("build.pack")).unwrap(),
            flate2::Compression::default(),
        );
    let mut archive = tar::Builder::new(encoder);
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(if manifest.executables.contains(&path) {
            0o755
        } else {
            0o644
        });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive
            .append_data(&mut header, path, bytes.as_slice())
            .unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap();
    descriptor.sha256 = digest(&fs::read(root.join("build.pack")).unwrap());
    descriptor
}

struct SourceOnly(dever_cli::libs::HttpRegistry);

#[test]
fn incompatible_sdist_candidate_backtracks_before_loading_build_tools() {
    let mut registry = LocalRegistry(BTreeMap::new());
    python_release(&mut registry, "app", "1.0.0", &[], &["backend>=1"]);
    python_release(&mut registry, "backend", "1.0.0", &[], &[]);
    python_release(&mut registry, "backend", "2.0.0", &[], &[]);
    let path = (Ecosystem::Pip, "/pypi/backend/2.0.0/json".into());
    let mut release: serde_json::Value = serde_json::from_slice(&registry.0[&path]).unwrap();
    release["urls"] = serde_json::json!([{"packagetype":"sdist","requires_python":">=3.13","filename":"backend-2.0.0.tar.gz","url":"must-not-fetch","digests":{"sha256":"0".repeat(64)}}]);
    registry
        .0
        .insert(path, serde_json::to_vec(&release).unwrap());
    let lock = python_lock(&registry, &["pip:app@1.0.0"]).unwrap();
    assert!(
        lock.libs
            .iter()
            .any(|lib| lib.spec.key() == "pip:backend@1.0.0")
    );
}

fn backend_fixture(mutate: bool) -> LocalRegistry {
    let mut registry = LocalRegistry(BTreeMap::new());
    python_release(&mut registry, "buildhelper", "1.0.0", &[], &[]);
    registry.0.insert(
        (Ecosystem::Pip, "/pypi/sample/json".into()),
        serde_json::to_vec(&serde_json::json!({"releases":{"1.0.0":[]}})).unwrap(),
    );
    let helper = wheel_fixture::pack(
        "buildhelper",
        "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        &wheel_fixture::metadata("buildhelper", "1.0.0", &[], &[]),
        &[
            (
                "buildhelper.py",
                b"def cli():\n    print('isolated-cli')\n".to_vec(),
            ),
            (
                "buildhelper-1.0.0.dist-info/entry_points.txt",
                b"[console_scripts]\nbuild-probe = buildhelper:cli\n".to_vec(),
            ),
        ],
    );
    let helper_source = source_archive(vec![
        ("buildhelper-1.0.0/pyproject.toml", b"[build-system]\nrequires = []\nbuild-backend = 'backend'\nbackend-path = ['.']\n".to_vec()),
        ("buildhelper-1.0.0/backend.py", b"import shutil\nfrom pathlib import Path\ndef build_wheel(directory, config_settings=None, metadata_directory=None):\n    name = 'buildhelper-1.0.0-py3-none-any.whl'\n    shutil.copy('result.whl', Path(directory) / name)\n    return name\n".to_vec()),
        ("buildhelper-1.0.0/result.whl", helper),
    ]);
    registry.0.insert((Ecosystem::Pip, "/pypi/buildhelper/1.0.0/json".into()),
        serde_json::to_vec(&serde_json::json!({"info":{},"urls":[{"packagetype":"sdist","filename":"buildhelper-1.0.0.tar.gz","url":"helper-source","digests":{"sha256":digest(&helper_source)}}]})).unwrap());
    registry
        .0
        .insert((Ecosystem::Pip, "helper-source".into()), helper_source);
    let original = wheel_fixture::metadata("sample", "1.0.0", &[], &[]);
    let changed = format!("{original}Summary: changed by wheel hook\n");
    let wheel = wheel_fixture::pack(
        "sample",
        "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        if mutate { &changed } else { &original },
        &[],
    );
    let backend = format!(
        r#"import os, shutil, subprocess
from pathlib import Path
def get_requires_for_build_wheel(config_settings=None):
    try:
        import buildhelper
        return []
    except ImportError:
        return ['buildhelper==1.0.0']
def prepare_metadata_for_build_wheel(directory, config_settings=None):
    assert subprocess.check_output(['build-probe'], text=True).strip() == 'isolated-cli'
    metadata = Path(directory) / 'sample-1.0.0.dist-info'
    metadata.mkdir()
    shutil.copy('original-metadata', metadata / 'METADATA')
    os.chdir('/')
    return metadata.name
def build_wheel(directory, config_settings=None, metadata_directory=None):
    assert Path.cwd().name == 'source', 'previous hook leaked its cwd'
    if {mutate}:
        shutil.copy('changed-metadata', Path(metadata_directory) / 'METADATA')
    name = 'sample-1.0.0-py3-none-any.whl'
    shutil.copy('result.whl', Path(directory) / name)
    return name
"#,
        mutate = if mutate { "True" } else { "False" }
    );
    let sourcefiles = vec![
        (
            "sample-1.0.0/pyproject.toml",
            b"[build-system]\nrequires = []\nbuild-backend = 'backend'\nbackend-path = ['.']\n"
                .to_vec(),
        ),
        ("sample-1.0.0/backend.py", backend.into_bytes()),
        ("sample-1.0.0/original-metadata", original.into_bytes()),
        ("sample-1.0.0/changed-metadata", changed.into_bytes()),
        ("sample-1.0.0/result.whl", wheel),
    ];
    let source = source_archive(sourcefiles);
    registry.0.insert((Ecosystem::Pip,"/pypi/sample/1.0.0/json".into()),serde_json::to_vec(&serde_json::json!({"info":{},"urls":[{"packagetype":"sdist","filename":"sample-1.0.0.tar.gz","url":"sample-source","digests":{"sha256":digest(&source)}}]})).unwrap());
    registry
        .0
        .insert((Ecosystem::Pip, "sample-source".into()), source);
    registry
}

fn source_archive(files: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes.as_slice())
            .unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

#[test]
#[ignore = "isolated offline PEP517 contracts; requires explicit target/source-build tool/runtime fixture"]
fn pep517_hooks_keep_requirements_cli_and_metadata_boundaries() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/source-build");
    let settings: Settings =
        serde_json::from_slice(&fs::read(root.join("config/setting.json")).unwrap()).unwrap();
    let inputs = AuthorInputs { root, settings };
    for mutate in [false, true] {
        let session = Session::new(&inputs);
        let transport = backend_fixture(mutate);
        let store = FixtureArtifactStore::default();
        let resolver = RegistryResolver {
            transport: &transport,
            store: &store,
            runtimes: BTreeMap::from([(Ecosystem::Pip, inputs.settings.runtime.clone())]),
            go_sumdb: dever_cli::libs::sumdb::ChecksumDatabase::official(None).unwrap(),
            build: Some(&session),
        };
        let result = dever_cli::libs::LibResolver::resolve(
            &resolver,
            &["pip:sample@1.0.0".parse().unwrap()],
        );
        if mutate {
            assert!(result.unwrap_err().contains("metadata differs"));
            continue;
        }
        let lock = result.unwrap();
        dever_cli::libs::doctor(&lock).unwrap();
        assert_eq!(lock.builds[0].dynamic_requires, vec!["buildhelper==1.0.0"]);
        assert_eq!(
            lock.builds[0].inputs.builds.len(),
            1,
            "nested build requirement receipt must be retained"
        );
        let before = lock.encode().unwrap();
        let cold = FixtureArtifactStore::default();
        let archives = super::restore_tests::archives_only(&lock, &transport);
        dever_cli::libs::restore::restore_artifacts(
            &lock,
            &archives,
            &cold,
            &inputs,
            &inputs.settings.runtime.target,
            &dever_cli::libs::sumdb::Verifier::official(),
        )
        .unwrap();
        assert_eq!(lock.encode().unwrap(), before);
        let mut mismatch = lock.clone();
        mismatch.builds[0].output.sha256 = "0".repeat(64);
        mismatch.libs[0].artifacts = vec![mismatch.builds[0].output.clone()];
        mismatch.libs[0].build = Some(mismatch.builds[0].identity().unwrap());
        let rejected = FixtureArtifactStore::default();
        assert!(
            dever_cli::libs::restore::restore_artifacts(
                &mismatch,
                &archives,
                &rejected,
                &inputs,
                &inputs.settings.runtime.target,
                &dever_cli::libs::sumdb::Verifier::official()
            )
            .unwrap_err()
            .contains("not byte reproducible")
        );
        assert!(
            !rejected
                .has(
                    &mismatch.builds[0].output.sha256,
                    &inputs.settings.runtime.target
                )
                .unwrap()
        );
        for mutation in 0..3 {
            let mut altered = lock.clone();
            match mutation {
                0 => altered.builds[0].tools.sha256 = "not-a-hash".into(),
                1 => altered.builds[0].dynamic_requires = vec!["buildhelper>=2".into()],
                _ => *altered.builds[0].inputs = LockFile::new(vec![]).unwrap(),
            }
            altered.libs[0].build = Some(altered.builds[0].identity().unwrap());
            assert!(
                dever_cli::libs::doctor(&altered).is_err(),
                "accepted contradictory receipt {mutation}"
            );
        }
    }
}
impl RegistryTransport for SourceOnly {
    fn get(&self, ecosystem: Ecosystem, path: &str, limit: usize) -> Result<Vec<u8>, String> {
        let bytes = self.0.get(ecosystem, path, limit)?;
        // Explicit author acceptance forces this one package's official sdist.
        // Backend requirements still use their ordinary authenticated wheels.
        if path == "/pypi/simplejson/3.20.1/json" {
            let mut document: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            document["urls"]
                .as_array_mut()
                .ok_or("official release lacks files")?
                .retain(|file| file["packagetype"] == "sdist");
            serde_json::to_vec(&document).map_err(|error| error.to_string())
        } else {
            Ok(bytes)
        }
    }
}

#[test]
#[ignore = "explicit official PyPI source build: downloads fixed source/build requirements and executes signed managed build tools"]
fn builds_official_python_source_with_isolated_pep517() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/source-build");
    let settings: Settings =
        serde_json::from_slice(&fs::read(root.join("config/setting.json")).unwrap()).unwrap();
    let inputs = AuthorInputs {
        root: root.clone(),
        settings,
    };
    let session = Session::new(&inputs);
    let transport = SourceOnly(dever_cli::libs::HttpRegistry::official());
    let store = FixtureArtifactStore::default();
    let resolver = RegistryResolver {
        transport: &transport,
        store: &store,
        runtimes: BTreeMap::from([(Ecosystem::Pip, inputs.settings.runtime.clone())]),
        go_sumdb: dever_cli::libs::sumdb::ChecksumDatabase::official(None).unwrap(),
        build: Some(&session),
    };
    let requested = vec!["pip:simplejson@3.20.1".parse().unwrap()];
    let lock = dever_cli::libs::LibResolver::resolve(&resolver, &requested).unwrap();
    dever_cli::libs::doctor(&lock).unwrap();
    assert_eq!(lock.builds.len(), 1);
    assert!(!lock.builds[0].inputs.libs.is_empty());
    let artifact = &lock
        .libs
        .iter()
        .find(|lib| lib.spec.name == "simplejson")
        .unwrap()
        .artifacts[0];
    let wheel = store
        .verify_exact(&artifact.sha256, artifact.bytes, &artifact.target)
        .unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(wheel)).unwrap();
    assert!((0..archive.len()).any(|index| {
        let file = archive.by_index(index).unwrap();
        file.name().contains("_speedups.") && file.name().ends_with(".so")
    }));
    assert_eq!(
        lock,
        dever_cli::libs::LibResolver::resolve(&resolver, &requested).unwrap(),
        "same session must replay one exact built artifact"
    );
    publish_fixture_artifacts(&root.join("dependency"), &lock, &store);
    let mut altered = lock.clone();
    altered.builds[0].frontend = "0".repeat(64);
    assert!(dever_cli::libs::doctor(&altered).is_err());
}
