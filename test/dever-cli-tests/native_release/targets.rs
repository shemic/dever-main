use super::*;
use dever_cli::toolchain::runtime_pack::RuntimePack;
use dever_runtime::config::RuntimeProfile;

#[cfg(target_arch = "x86_64")]
fn cross_author() -> Author {
    let mut author = Author::new();
    let target = BuildTarget::LinuxAarch64;
    let input = |name: &str, bytes: Vec<u8>| {
        let source = format!("inputs/arm-{name}");
        fs::write(author.root.join(&source), bytes).unwrap();
        json!({"source":source, "sha256":sha256_file(&author.root.join(source)).unwrap()})
    };
    let mut profiles = json!({});
    for profile in ["base", "sqlite", "postgres", "both"] {
        profiles[profile] = input(profile, native_objects::archive(target, profile));
    }
    let link = |name: &str, bytes| {
        let mut artifact = input(name, bytes);
        artifact["path"] = json!(name);
        artifact
    };
    author.settings["native_targets"] = json!({"linux-aarch64": {
        "profiles":profiles,
        "start":[link("start.o", native_objects::object(target))],
        "libraries":[link("system.a", native_objects::archive(target, "system.o"))],
        "end":[link("end.o", native_objects::object(target))],
    }});
    author
}

#[test]
fn target_names_and_serde_are_exact() {
    for (name, target, triple) in [
        (
            "linux-x86_64",
            BuildTarget::LinuxX86_64,
            "x86_64-unknown-linux-gnu",
        ),
        (
            "linux-aarch64",
            BuildTarget::LinuxAarch64,
            "aarch64-unknown-linux-gnu",
        ),
    ] {
        assert_eq!(name.parse::<BuildTarget>().unwrap(), target);
        assert_eq!(target.platform(), name);
        assert_eq!(target.triple(), triple);
        assert_eq!(
            serde_json::to_string(&target).unwrap(),
            format!("\"{name}\"")
        );
    }
    for invalid in [
        "",
        "arm64",
        "Linux-aarch64",
        "linux-arm64",
        "aarch64-unknown-linux-gnu",
        "linux-aarch64 ",
    ] {
        assert!(invalid.parse::<BuildTarget>().is_err());
        assert!(serde_json::from_value::<BuildTarget>(json!(invalid)).is_err());
    }
}

#[test]
#[cfg(target_arch = "x86_64")]
fn host_core_packages_both_native_targets_with_separate_profiles_and_signed_closures() {
    let author = cross_author();
    author.save();
    let output = author.directory.path().join("cross-release");
    let manifest = packaging::create(&author.root, &output).unwrap();
    assert_eq!(manifest.platform, "linux-x86_64");
    assert_signed(&author, &output);
    for target in [BuildTarget::LinuxX86_64, BuildTarget::LinuxAarch64] {
        let closure = runtime_pack::validate(&output, env!("CARGO_PKG_VERSION"), target).unwrap();
        for input in closure {
            assert!(
                manifest
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.path == input.path
                        && artifact.sha256 == input.sha256
                        && artifact.bytes == input.bytes)
            );
        }
        for profile in [
            RuntimeProfile {
                sqlite: false,
                postgres: false,
            },
            RuntimeProfile {
                sqlite: true,
                postgres: false,
            },
            RuntimeProfile {
                sqlite: false,
                postgres: true,
            },
            RuntimeProfile {
                sqlite: true,
                postgres: true,
            },
        ] {
            assert_eq!(
                RuntimePack::load(&output, profile, target).unwrap().target,
                target
            );
        }
    }
    let profile = RuntimeProfile {
        sqlite: false,
        postgres: false,
    };
    let arm = RuntimePack::load(&output, profile, BuildTarget::LinuxAarch64).unwrap();
    let host = RuntimePack::load(&output, profile, BuildTarget::LinuxX86_64).unwrap();
    assert_ne!(arm.identity, host.identity);
    fs::write(
        output.join("runtime/linux-aarch64/archives/base.a"),
        b"tampered",
    )
    .unwrap();
    assert!(RuntimePack::load(&output, profile, BuildTarget::LinuxAarch64).is_err());
    RuntimePack::load(&output, profile, BuildTarget::LinuxX86_64).unwrap();
}

#[test]
#[cfg(target_arch = "x86_64")]
fn target_packs_reject_wrong_architecture_and_duplicate_host_declarations() {
    for (slot, source) in [
        ("/native_targets/linux-aarch64/start/0", "start.o"),
        ("/native_targets/linux-aarch64/profiles/base", "base"),
    ] {
        let mut author = cross_author();
        let replacement = if source.ends_with(".o") {
            native_objects::object(BuildTarget::LinuxX86_64)
        } else {
            native_objects::archive(BuildTarget::LinuxX86_64, "wrong.o")
        };
        fs::write(
            author.root.join(format!("inputs/arm-{source}")),
            replacement,
        )
        .unwrap();
        author.settings.pointer_mut(slot).unwrap()["sha256"] =
            json!(sha256_file(&author.root.join(format!("inputs/arm-{source}"))).unwrap());
        assert!(
            author
                .reject()
                .contains("relocatable ELF object for linux-aarch64")
        );
    }
    let mut author = cross_author();
    author.settings["native_targets"]["linux-x86_64"] =
        author.settings["native_targets"]["linux-aarch64"].clone();
    assert!(author.reject().contains("duplicate release destination"));
}

#[test]
#[cfg(target_arch = "x86_64")]
fn missing_target_pack_never_uses_host_inputs() {
    let author = Author::new();
    author.save();
    let output = author.directory.path().join("host-release");
    packaging::create(&author.root, &output).unwrap();
    let error = RuntimePack::load(
        &output,
        RuntimeProfile {
            sqlite: false,
            postgres: false,
        },
        BuildTarget::LinuxAarch64,
    )
    .err()
    .unwrap();
    assert!(error.contains("linux-aarch64"), "{error}");
}
