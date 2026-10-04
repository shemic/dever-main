//! Authoring contracts use tiny synthetic payloads; real link/install checks are opt-in.
#![cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use dever_cli::toolchain::{BuildTarget, packaging, platform_identity, runtime_pack, sha256_file};
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde_json::{Value, json};

#[path = "../dever-tests/tests/support/temp.rs"]
mod temp;
use temp::TemporaryDirectory;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "native_release/acceptance.rs"]
mod acceptance;

#[path = "native_release/ecosystems.rs"]
mod ecosystems;

#[cfg(target_os = "linux")]
#[path = "native_release/bootstrap.rs"]
mod bootstrap;

#[path = "support/native_elf.rs"]
mod native_elf;
#[path = "support/native_objects.rs"]
mod native_objects;

#[path = "native_release/targets.rs"]
mod targets;

#[test]
fn sandbox_bootstrap_requires_static_elf_and_complete_guard_libraries() {
    let loader = format!("lib/{}", dever_sandbox::loader_name());
    let mut files = vec![
        ("bin/bwrap".into(), native_elf::static_executable()),
        (
            "bin/guard".into(),
            native_elf::elf(None, &["libc.so.6"], None),
        ),
        (
            loader,
            native_elf::library(dever_sandbox::loader_name(), &[]),
        ),
        (
            "lib/libc.so.6".into(),
            native_elf::library("libc.so.6", &[]),
        ),
    ];
    dever_sandbox::validate_assets(&files).unwrap();
    files[0].1 = native_elf::elf(None, &[], None);
    assert!(
        dever_sandbox::validate_assets(&files)
            .unwrap_err()
            .contains("static target ELF")
    );
    files[0].1 = native_elf::library("bwrap", &["libc.so.6"]);
    assert!(
        dever_sandbox::validate_assets(&files)
            .unwrap_err()
            .contains("static target ELF")
    );
    files[0].1 = native_elf::static_executable();
    files.pop();
    assert!(
        dever_sandbox::validate_assets(&files)
            .unwrap_err()
            .contains("sandbox dependency 'lib/libc.so.6' is missing")
    );
}

struct Author {
    directory: TemporaryDirectory,
    root: PathBuf,
    settings: Value,
    public_key: Vec<u8>,
}

impl Author {
    fn new() -> Self {
        let directory = TemporaryDirectory::new();
        let root = directory.path().join("author");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir(root.join("inputs")).unwrap();
        let key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        fs::write(root.join("signing.pk8"), key.as_ref()).unwrap();
        fs::set_permissions(root.join("signing.pk8"), fs::Permissions::from_mode(0o600)).unwrap();
        let public_key = Ed25519KeyPair::from_pkcs8(key.as_ref())
            .unwrap()
            .public_key()
            .as_ref()
            .to_vec();
        let input = |name: &str, bytes: &[u8]| {
            let source = format!("inputs/{name}");
            fs::write(root.join(&source), bytes).unwrap();
            json!({ "source": source, "sha256": sha256_file(&root.join(source)).unwrap() })
        };
        let core = input(
            "core",
            &native_elf::compiler(&["libLLVM.so.18.1", "libc.so.6"]),
        );
        let mut llvm = input(
            "llvm",
            &native_elf::library("libLLVM.so.18.1", &["libc.so.6"]),
        );
        llvm["path"] = json!("libLLVM.so.18.1");
        let mut profiles = json!({});
        for name in ["base", "sqlite", "postgres", "both"] {
            profiles[name] = input(
                name,
                &native_objects::archive(BuildTarget::host().unwrap(), name),
            );
        }
        let link_input = |name: &str, bytes: &[u8]| {
            let mut value = input(name, bytes);
            value["path"] = json!(name);
            value
        };
        let start = link_input(
            "start.o",
            &native_objects::object(BuildTarget::host().unwrap()),
        );
        let end = link_input(
            "end.o",
            &native_objects::object(BuildTarget::host().unwrap()),
        );
        let library = link_input("system.a", b"!<arch>\n");
        let skill = link_input(
            "SKILL.md",
            b"---\nname: dever-language\ndescription: Develop Dever applications.\n---\n",
        );
        let settings = json!({
            "format": "dever-native-release-input-v1",
            "version": env!("CARGO_PKG_VERSION"),
            "target": format!("{}-unknown-linux-gnu", std::env::consts::ARCH),
            "signing_key": "signing.pk8", "core": core, "core_libraries": [llvm],
            "skill": [skill],
            "profiles": profiles, "start": [start], "libraries": [library], "end": [end],
        });
        Self {
            directory,
            root,
            settings,
            public_key,
        }
    }

    fn save(&self) {
        fs::write(
            self.root.join("config/setting.json"),
            serde_json::to_vec(&self.settings).unwrap(),
        )
        .unwrap();
    }

    fn output(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    fn machine(&self) -> dever_cli::toolchain::Layout {
        let layout = dever_cli::toolchain::Layout::new(self.output("machine"));
        layout.initialize().unwrap();
        let key_path = layout.state().join("trusted-release-key");
        fs::write(
            &key_path,
            self.public_key
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        )
        .unwrap();
        fs::set_permissions(key_path, fs::Permissions::from_mode(0o644)).unwrap();
        layout
    }

    fn reject(&self) -> String {
        self.save();
        let output = self.output("rejected");
        let error = packaging::create(&self.root, &output).unwrap_err();
        assert!(!output.exists());
        assert_eq!(
            fs::read_dir(self.directory.path()).unwrap().count(),
            1,
            "staging leaked: {error}"
        );
        assert!(
            !error.contains("signing.pk8"),
            "private key location leaked: {error}"
        );
        error
    }
}

fn contents(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(directory, directory, &mut files);
    files
}

fn assert_signed(author: &Author, release: &Path) {
    let encoded = fs::read_to_string(release.join("manifest.sig")).unwrap();
    let (pairs, remainder) = encoded.trim().as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    let signature = pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
    UnparsedPublicKey::new(&ED25519, &author.public_key)
        .verify(
            &fs::read(release.join("manifest.json")).unwrap(),
            &signature,
        )
        .unwrap();
}

#[test]
fn release_skill_rejects_missing_entry_and_oversized_files() {
    let mut author = Author::new();
    author.settings["skill"] = json!([]);
    author.save();
    assert!(packaging::create(&author.root, &author.output("missing")).is_err());
    assert!(!author.output("missing").exists());
    let mut author = Author::new();
    fs::write(
        author.root.join("inputs/SKILL.md"),
        vec![b'x'; 1024 * 1024 + 1],
    )
    .unwrap();
    author.settings["skill"][0]["sha256"] =
        json!(sha256_file(&author.root.join("inputs/SKILL.md")).unwrap());
    author.save();
    assert!(
        packaging::create(&author.root, &author.output("large"))
            .unwrap_err()
            .contains("limit")
    );
}

#[test]
fn identical_inputs_produce_identical_signed_independent_releases() {
    let author = Author::new();
    author.save();
    let first = author.output("first");
    let second = author.output("second");
    let manifest = packaging::create(&author.root, &first).unwrap();
    assert_eq!(packaging::create(&author.root, &second).unwrap(), manifest);
    let before = contents(&first);
    assert_eq!(contents(&second), before);
    assert_eq!(before.len(), manifest.artifacts.len() + 2);
    assert!(
        manifest
            .artifacts
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
    );
    for artifact in &manifest.artifacts {
        assert_eq!(
            sha256_file(&first.join(&artifact.path)).unwrap(),
            artifact.sha256
        );
        assert_eq!(fs::metadata(first.join(&artifact.path)).unwrap().nlink(), 1);
    }
    assert_signed(&author, &first);
    let runtime = runtime_pack::validate(
        &first,
        env!("CARGO_PKG_VERSION"),
        BuildTarget::host().unwrap(),
    )
    .unwrap();
    assert_eq!(runtime.len() + 3, manifest.artifacts.len());
    let metadata: Value = serde_json::from_slice(
        &before[&PathBuf::from(format!("runtime/{}/manifest.json", platform_identity()))],
    )
    .unwrap();
    for profile in ["base", "sqlite", "postgres", "both"] {
        assert_eq!(
            metadata["profiles"][profile]["runtime"],
            format!("archives/{profile}.a")
        );
    }
    fs::write(author.root.join("inputs/base"), b"changed source").unwrap();
    assert_eq!(contents(&first), before);
    assert_eq!(contents(&second), before);
}

#[test]
fn compiler_libraries_must_form_a_closed_target_and_search_path() {
    let invalid_libraries = [
        (
            native_elf::library("libLLVM.so.18.1", &["libz.so.1"]),
            "not supplied",
        ),
        (
            native_elf::library("different.so", &["libc.so.6"]),
            "SONAME",
        ),
        (
            native_elf::elf(Some("libLLVM.so.18.1"), &[], Some("/usr/lib")),
            "search path",
        ),
        (
            native_elf::elf(Some("libLLVM.so.18.1"), &[], Some("$ORIGIN/../../lib")),
            "search path",
        ),
        (
            native_elf::library("libLLVM.so.18.1", &["/usr/lib/libz.so.1"]),
            "plain library",
        ),
    ];
    for (bytes, diagnostic) in invalid_libraries {
        let mut author = Author::new();
        fs::write(author.root.join("inputs/llvm"), bytes).unwrap();
        author.settings["core_libraries"][0]["sha256"] =
            json!(sha256_file(&author.root.join("inputs/llvm")).unwrap());
        assert!(author.reject().contains(diagnostic));
    }
    let mut author = Author::new();
    let mut bytes = native_elf::compiler(&["libLLVM.so.18.1"]);
    bytes[18..20].copy_from_slice(&0u16.to_le_bytes());
    fs::write(author.root.join("inputs/core"), bytes).unwrap();
    author.settings["core"]["sha256"] =
        json!(sha256_file(&author.root.join("inputs/core")).unwrap());
    assert!(author.reject().contains("incompatible target"));
    let mut author = Author::new();
    author.settings["core_libraries"] = json!([]);
    assert!(author.reject().contains("not supplied"));
    let mut author = Author::new();
    let bytes = native_elf::elf(None, &["libLLVM.so.18.1"], Some("/usr/lib"));
    fs::write(author.root.join("inputs/core"), bytes).unwrap();
    author.settings["core"]["sha256"] =
        json!(sha256_file(&author.root.join("inputs/core")).unwrap());
    assert!(author.reject().contains("inherited RPATH"));
}

#[test]
fn installation_rechecks_the_signed_native_compiler_closure_before_health_execution() {
    let author = Author::new();
    author.save();
    let layout = author.machine();
    let release = layout.downloads().join(env!("CARGO_PKG_VERSION"));
    let mut manifest = packaging::create(&author.root, &release).unwrap();
    manifest
        .artifacts
        .retain(|artifact| artifact.path != "lib/libLLVM.so.18.1");
    fs::remove_file(release.join("lib/libLLVM.so.18.1")).unwrap();
    let encoded = serde_json::to_vec(&manifest).unwrap();
    let key =
        Ed25519KeyPair::from_pkcs8(&fs::read(author.root.join("signing.pk8")).unwrap()).unwrap();
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
    let error = dever_cli::toolchain::MachineManager::new(layout.clone())
        .install_requested(env!("CARGO_PKG_VERSION"))
        .unwrap_err();
    assert!(error.contains("not supplied"), "{error}");
    assert!(!layout.versions().join(env!("CARGO_PKG_VERSION")).exists());
}

#[test]
fn native_core_resolution_rejects_unsigned_loader_search_entries() {
    for entry in [
        "lib/libc.so.6",
        "lib/glibc-hwcaps/x86-64-v3/libLLVM.so.18.1",
    ] {
        let author = Author::new();
        author.save();
        let layout = author.machine();
        let version = dever_cli::toolchain::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        let release = layout.versions().join(version.as_str());
        packaging::create(&author.root, &release).unwrap();
        let extra = release.join(entry);
        fs::create_dir_all(extra.parent().unwrap()).unwrap();
        fs::write(extra, b"unsigned loader input").unwrap();
        let error = dever_cli::toolchain::MachineManager::new(layout)
            .resolve_core(&version)
            .unwrap_err();
        assert!(
            error.contains("unsigned file, directory or link"),
            "{error}"
        );
    }
}

#[test]
fn rejects_invalid_configuration_and_rolls_back_partial_payloads() {
    let changes = [
        ("/format", json!("unknown")),
        ("/target", json!("x86_64-pc-windows-msvc")),
        ("/version", json!("1.0")),
        ("/unexpected", json!(true)),
        ("/core/unexpected", json!(true)),
        ("/core/source", json!("../outside")),
        ("/core/source", json!("/etc/passwd")),
        ("/core/source", json!("inputs/./core")),
        ("/core/source", json!("inputs/missing")),
        ("/profiles/base/sha256", json!("0".repeat(64))),
        ("/profiles/base/sha256", json!("A".repeat(64))),
        ("/profiles/postgres", Value::Null),
        ("/start", json!([])),
        ("/libraries", json!([])),
        ("/end", json!([])),
        ("/end/0/path", json!("start.o")),
        ("/end/0/path", json!("archives/base.a")),
        ("/end/0/path", json!("manifest.json")),
        ("/end/0/path", json!("../../escape.o")),
        ("/end/0/path", json!("end.txt")),
    ];
    for (pointer, value) in changes {
        let mut author = Author::new();
        if let Some(slot) = author.settings.pointer_mut(pointer) {
            *slot = value;
        } else if pointer == "/unexpected" {
            author.settings["unexpected"] = value;
        } else {
            author.settings["core"]["unexpected"] = value;
        }
        author.reject();
    }
    let mut author = Author::new();
    author.settings["profiles"]
        .as_object_mut()
        .unwrap()
        .remove("sqlite");
    author.reject();
    for bytes in [b"!<thin>\n".as_slice(), b"linker script".as_slice()] {
        let mut author = Author::new();
        fs::write(author.root.join("inputs/base"), bytes).unwrap();
        author.settings["profiles"]["base"]["sha256"] =
            json!(sha256_file(&author.root.join("inputs/base")).unwrap());
        assert!(author.reject().contains("self-contained archive"));
    }
}

#[test]
fn private_key_and_symlink_inputs_never_enter_the_release() {
    let author = Author::new();
    fs::set_permissions(
        author.root.join("signing.pk8"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(author.reject().contains("permissions"));
    let mut author = Author::new();
    symlink("signing.pk8", author.root.join("key-link")).unwrap();
    author.settings["signing_key"] = json!("key-link");
    author.reject();
    let mut author = Author::new();
    symlink("inputs/core", author.root.join("payload-link")).unwrap();
    author.settings["core"]["source"] = json!("payload-link");
    author.reject();
    let mut author = Author::new();
    symlink("inputs", author.root.join("directory-link")).unwrap();
    author.settings["core"]["source"] = json!("directory-link/core");
    author.reject();
    let mut author = Author::new();
    fs::copy(
        author.root.join("signing.pk8"),
        author.root.join("inputs/key-copy"),
    )
    .unwrap();
    author.settings["core"] = json!({ "source":"inputs/key-copy", "sha256":sha256_file(&author.root.join("inputs/key-copy")).unwrap() });
    assert!(author.reject().contains("cannot be included"));
}

#[test]
fn existing_outputs_are_preserved_and_concurrent_creation_has_one_winner() {
    let author = Author::new();
    author.save();
    let existing = author.output("existing");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("keep"), b"original").unwrap();
    let before = contents(&existing);
    assert!(packaging::create(&author.root, &existing).is_err());
    assert_eq!(contents(&existing), before);
    let empty = author.output("empty");
    fs::create_dir(&empty).unwrap();
    assert!(packaging::create(&author.root, &empty).is_err());
    assert!(empty.is_dir());
    let link = author.output("link");
    symlink(&existing, &link).unwrap();
    assert!(packaging::create(&author.root, &link).is_err());
    assert_eq!(contents(&existing), before);
    let output = author.output("concurrent");
    let outcomes = std::thread::scope(|scope| {
        let first = scope.spawn(|| packaging::create(&author.root, &output));
        let second = scope.spawn(|| packaging::create(&author.root, &output));
        [first.join().unwrap(), second.join().unwrap()]
    });
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    runtime_pack::validate(
        &output,
        env!("CARGO_PKG_VERSION"),
        BuildTarget::host().unwrap(),
    )
    .unwrap();
    assert_eq!(fs::read_dir(author.directory.path()).unwrap().count(), 5);
}

#[test]
fn duplicate_fields_and_invalid_private_keys_fail_before_publication() {
    let author = Author::new();
    author.save();
    let config = author.root.join("config/setting.json");
    let original = fs::read_to_string(&config).unwrap();
    let duplicate = original.replacen('{', "{\"signing_key\":\"signing.pk8\",", 1);
    fs::write(&config, duplicate).unwrap();
    let error = packaging::create(&author.root, &author.output("duplicate")).unwrap_err();
    assert!(error.contains("invalid author configuration"));
    assert!(!error.contains("signing.pk8"));
    assert_eq!(fs::read_dir(author.directory.path()).unwrap().count(), 1);
    fs::write(author.root.join("signing.pk8"), b"invalid private key").unwrap();
    assert!(author.reject().contains("PKCS#8"));
}
