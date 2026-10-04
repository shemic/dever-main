//! Synthetic runtime closures exercise the production maker without host interpreters.

use std::io::Read;

use super::*;

fn add_file(author: &mut Author, ecosystem: &str, path: &str, bytes: &[u8]) {
    let source = format!("inputs/{ecosystem}/{path}");
    let file = author.root.join(&source);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, bytes).unwrap();
    author.settings["runtimes"][ecosystem]["files"]
        .as_array_mut()
        .unwrap()
        .push(json!({"source": source, "path": path, "sha256": sha256_file(&file).unwrap()}));
}

fn set_manifest(author: &mut Author, ecosystem: &str, change: impl FnOnce(&mut Value)) {
    let input = author.settings["runtimes"][ecosystem]["files"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|input| input["path"] == "dever-runtime.json")
        .unwrap();
    let path = author.root.join(input["source"].as_str().unwrap());
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut manifest);
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    input["sha256"] = json!(sha256_file(&path).unwrap());
}

fn runtime_author() -> Author {
    let mut author = Author::new();
    for (ecosystem, name, version) in [
        ("pip", "cpython", "3.12.0"),
        ("npm", "node", "24.0.0"),
        ("go", "go", "1.25.0"),
    ] {
        author.settings["runtimes"][ecosystem] = json!({"name":name,"version":version,"files":[]});
        let mut descriptor = json!({"format":"dever-worker-runtime-v1","ecosystem":ecosystem,"target":platform_identity()});
        if ecosystem == "go" {
            let arch = if std::env::consts::ARCH == "x86_64" {
                "amd64"
            } else {
                "arm64"
            };
            descriptor["build"] = json!({"host":platform_identity(),"compiler":"bin/compile","linker":"bin/link","analyzer":"bin/analyze","stdlib_importcfg":"stdlib/importcfg","goos":"linux","goarch":arch,"go_version":format!("go version go1.25.0 linux/{arch}")});
            for path in ["bin/compile", "bin/link", "bin/analyze"] {
                add_file(
                    &mut author,
                    ecosystem,
                    path,
                    &native_elf::static_executable(),
                );
            }
            let mut importcfg = String::new();
            for name in ["runtime", "context", "encoding/json"] {
                let path = format!("stdlib/{name}.a");
                let object = format!("go object linux {arch} go1.25.0 fixture\n");
                let archive = format!(
                    "!<arch>\n{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n{object}",
                    "__.PKGDEF",
                    0,
                    0,
                    0,
                    "644",
                    object.len()
                );
                add_file(&mut author, ecosystem, &path, archive.as_bytes());
                importcfg.push_str(&format!("packagefile {name}={path}\n"));
            }
            add_file(
                &mut author,
                ecosystem,
                "stdlib/importcfg",
                importcfg.as_bytes(),
            );
        } else {
            descriptor["executable"] = json!(format!("bin/{name}"));
            descriptor["arguments"] = if ecosystem == "pip" {
                json!(["-I", "-S", "-B"])
            } else {
                json!([])
            };
            add_file(
                &mut author,
                ecosystem,
                &format!("bin/{name}"),
                &native_elf::static_executable(),
            );
        }
        add_file(
            &mut author,
            ecosystem,
            "dever-runtime.json",
            &serde_json::to_vec(&descriptor).unwrap(),
        );
    }
    author.settings["runtimes"]["pip"]["python_markers"] = json!({
        "implementation_name":"cpython","implementation_version":"3.12.0","os_name":"posix",
        "platform_machine":std::env::consts::ARCH,"platform_python_implementation":"CPython",
        "platform_release":"","platform_system":"Linux","platform_version":"",
        "python_full_version":"3.12.0","python_version":"3.12","sys_platform":"linux"
    });
    author.settings["runtimes"]["pip"]["python_wheel_tags"] = json!(["py3-none-any"]);
    let markers = author.settings["runtimes"]["pip"]["python_markers"].clone();
    set_manifest(&mut author, "pip", |manifest| {
        manifest["python_markers"] = markers;
        manifest["python_wheel_tags"] = json!(["py3-none-any"]);
        manifest["python_extension_suffixes"] = json!([".abi3.so", ".so"]);
    });
    author
}

#[test]
fn ecosystem_archives_are_deterministic_and_signed_with_computed_identities() {
    let mut author = runtime_author();
    author.save();
    let first = author.output("first");
    let manifest = packaging::create(&author.root, &first).unwrap();
    // Input order and filesystem mode are not archive metadata.
    for ecosystem in ["pip", "npm", "go"] {
        author.settings["runtimes"][ecosystem]["files"]
            .as_array_mut()
            .unwrap()
            .reverse();
    }
    fs::set_permissions(
        author.root.join("inputs/npm/bin/node"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    author.save();
    let second = author.output("second");
    assert_eq!(packaging::create(&author.root, &second).unwrap(), manifest);
    assert_eq!(contents(&first), contents(&second));
    assert_signed(&author, &first);
    for ecosystem in ["pip", "npm", "go"] {
        let prefix = format!("runtime/{ecosystem}/{}", platform_identity());
        let metadata_path = format!("{prefix}/manifest.json");
        let pack_path = format!("{prefix}/runtime.pack");
        let metadata: Value =
            serde_json::from_slice(&fs::read(first.join(&metadata_path)).unwrap()).unwrap();
        assert_eq!(metadata["format"], "dever-registry-runtime-v1");
        assert_eq!(
            metadata["runtime"]["pack"]["sha256"],
            sha256_file(&first.join(&pack_path)).unwrap()
        );
        for path in [&metadata_path, &pack_path] {
            let signed = manifest
                .artifacts
                .iter()
                .find(|artifact| &artifact.path == path)
                .unwrap();
            assert_eq!(signed.sha256, sha256_file(&first.join(path)).unwrap());
        }
        let bytes = fs::read(first.join(&pack_path)).unwrap();
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
        let mut names = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_str().unwrap().to_owned();
            assert_eq!(entry.header().mtime().unwrap(), 0);
            assert_eq!(entry.header().uid().unwrap(), 0);
            assert_eq!(entry.header().gid().unwrap(), 0);
            assert_eq!(
                entry.header().mode().unwrap(),
                if path.starts_with("bin/") {
                    0o755
                } else {
                    0o644
                }
            );
            let mut actual = Vec::new();
            entry.read_to_end(&mut actual).unwrap();
            assert_eq!(
                actual,
                fs::read(author.root.join(format!("inputs/{ecosystem}/{path}"))).unwrap()
            );
            names.push(path);
        }
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    }
    let before = contents(&first);
    fs::write(author.root.join("inputs/npm/bin/node"), b"changed").unwrap();
    assert_eq!(contents(&first), before);
}

#[test]
fn rejects_incomplete_or_mismatched_runtime_descriptors() {
    for ecosystem in ["pip", "npm", "go"] {
        for field in ["format", "ecosystem", "target"] {
            let mut author = runtime_author();
            set_manifest(&mut author, ecosystem, |manifest| {
                manifest[field] = json!("wrong")
            });
            assert!(author.reject().contains("identity"));
        }
        let mut author = runtime_author();
        set_manifest(&mut author, ecosystem, |manifest| {
            manifest["unknown"] = json!(true)
        });
        assert!(author.reject().contains("manifest"));
        let mut author = runtime_author();
        author.settings["runtimes"][ecosystem]["files"]
            .as_array_mut()
            .unwrap()
            .retain(|input| input["path"] != "dever-runtime.json");
        assert!(author.reject().contains("dever-runtime.json"));
    }
    for (ecosystem, field, value) in [
        ("pip", "arguments", json!(["-I", "-B"])),
        ("npm", "executable", json!("bin/missing")),
        ("npm", "arguments", json!(["source.js"])),
        ("go", "executable", json!("bin/compile")),
        ("go", "build", Value::Null),
    ] {
        let mut author = runtime_author();
        set_manifest(&mut author, ecosystem, |manifest| manifest[field] = value);
        author.reject();
    }
    for (field, value) in [
        ("goarch", "unknown"),
        ("compiler", "bin/missing"),
        ("stdlib_importcfg", "missing"),
    ] {
        let mut author = runtime_author();
        set_manifest(&mut author, "go", |manifest| {
            manifest["build"][field] = json!(value)
        });
        author.reject();
    }
    let mut author = runtime_author();
    author.settings["runtimes"]["go"]["files"]
        .as_array_mut()
        .unwrap()
        .retain(|input| input["path"] != "stdlib/context.a");
    assert!(author.reject().contains("archive"));
}

#[test]
fn go_cross_pack_binds_host_tools_and_arm_standard_library_separately() {
    for wrong_stdlib in [false, true] {
        let mut author = runtime_author();
        set_manifest(&mut author, "go", |manifest| {
            manifest["target"] = json!("linux-aarch64");
            manifest["build"]["goarch"] = json!("arm64");
        });
        for input in author.settings["runtimes"]["go"]["files"]
            .as_array_mut()
            .unwrap()
        {
            if !input["path"].as_str().unwrap().ends_with(".a") {
                continue;
            }
            let path = author.root.join(input["source"].as_str().unwrap());
            let architecture = if wrong_stdlib { "amd64" } else { "arm64" };
            let object = format!("go object linux {architecture} go1.25.0 fixture\n");
            let archive = format!(
                "!<arch>\n{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n{object}",
                "__.PKGDEF",
                0,
                0,
                0,
                "644",
                object.len()
            );
            fs::write(&path, archive).unwrap();
            input["sha256"] = json!(sha256_file(&path).unwrap());
        }
        let input = author.settings["runtimes"]
            .as_object_mut()
            .unwrap()
            .remove("go")
            .unwrap();
        author.settings["ecosystem_targets"] = json!({"linux-aarch64":{"go":input}});
        author.save();
        let result = packaging::create(&author.root, &author.output("cross-go"));
        if wrong_stdlib {
            assert!(
                result
                    .unwrap_err()
                    .contains("differs from its target or compiler version")
            );
        } else {
            let manifest = result.unwrap();
            assert!(
                manifest
                    .artifacts
                    .iter()
                    .any(|file| file.path == "runtime/go/linux-aarch64/runtime.pack")
            );
        }
    }
}

#[test]
fn sandbox_cross_assets_preserve_loader_mode_and_validate_deployment_target() {
    let mut files = vec![
        ("bin/bwrap".into(), native_elf::static_executable()),
        ("bin/guard".into(), native_elf::static_executable()),
        (
            "lib/ld-linux-aarch64.so.1".into(),
            native_elf::library("ld-linux-aarch64.so.1", &[]),
        ),
        (
            "lib/libc.so.6".into(),
            native_elf::library("libc.so.6", &[]),
        ),
    ];
    for (_, bytes) in &mut files {
        bytes[18..20].copy_from_slice(&183u16.to_le_bytes());
    }
    dever_sandbox::validate_assets_for_target(&files, "linux-aarch64").unwrap();
    let mut author = Author::new();
    let inputs = files
        .iter()
        .map(|(path, bytes)| {
            let source = format!("inputs/arm-sandbox/{path}");
            let file = author.root.join(&source);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, bytes).unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            json!({"source":source,"path":path,"sha256":sha256_file(&file).unwrap()})
        })
        .collect::<Vec<_>>();
    author.settings["sandbox_targets"] = json!({"linux-aarch64":inputs});
    author.save();
    let output = author.output("cross-sandbox");
    packaging::create(&author.root, &output).unwrap();
    for (path, mode) in [
        ("bin/bwrap", 0o755),
        ("bin/guard", 0o755),
        ("lib/ld-linux-aarch64.so.1", 0o755),
        ("lib/libc.so.6", 0o644),
    ] {
        assert_eq!(
            fs::metadata(output.join("sandbox/linux-aarch64").join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            mode,
            "incorrect packaged mode for {path}"
        );
    }
    files[1].1[18..20].copy_from_slice(&62u16.to_le_bytes());
    assert!(
        dever_sandbox::validate_assets_for_target(&files, "linux-aarch64")
            .unwrap_err()
            .contains("incompatible target")
    );
}

#[test]
fn rejects_invalid_registry_metadata_and_unknown_ecosystems() {
    for (ecosystem, field, value) in [
        ("pip", "python_markers", Value::Null),
        ("pip", "python_wheel_tags", json!([])),
        ("pip", "python_wheel_tags", json!(["invalid"])),
        (
            "pip",
            "python_wheel_tags",
            json!(["py3-none-any", "py3-none-any"]),
        ),
        ("npm", "python_wheel_tags", json!(["py3-none-any"])),
        ("npm", "name", json!("")),
        ("npm", "name", json!("../node")),
        ("go", "version", json!("latest")),
        ("go", "unknown", json!(true)),
    ] {
        let mut author = runtime_author();
        author.settings["runtimes"][ecosystem][field] = value;
        author.reject();
    }
    let mut author = runtime_author();
    author.settings["runtimes"]["npm"]["python_markers"] =
        author.settings["runtimes"]["pip"]["python_markers"].clone();
    author.reject();
    let mut author = runtime_author();
    author.settings["runtimes"]["exec"] = author.settings["runtimes"]["npm"].clone();
    author.reject();

    let author = runtime_author();
    author.save();
    let path = author.root.join("config/setting.json");
    let config = fs::read_to_string(&path).unwrap();
    let duplicate = config.replacen("\"runtimes\":{", "\"runtimes\":{\"npm\":null,", 1);
    assert_ne!(duplicate, config);
    fs::write(path, duplicate).unwrap();
    assert!(
        packaging::create(&author.root, &author.output("duplicate"))
            .unwrap_err()
            .contains("invalid author configuration")
    );
    assert_eq!(fs::read_dir(author.directory.path()).unwrap().count(), 1);
}

#[test]
fn rejects_runtime_path_conflicts_drift_and_private_key_aliases() {
    for path in [
        "../outside",
        "bin",
        "bin/node/child",
        "runtime.pack",
        "bin/node",
    ] {
        let mut author = runtime_author();
        let mut input = author.settings["runtimes"]["npm"]["files"][0].clone();
        input["path"] = json!(path);
        author.settings["runtimes"]["npm"]["files"]
            .as_array_mut()
            .unwrap()
            .push(input);
        author.reject();
    }
    let author = runtime_author();
    fs::write(author.root.join("inputs/npm/bin/node"), b"changed").unwrap();
    assert!(author.reject().contains("SHA-256"));
    let mut author = self::runtime_author();
    let alias = author.root.join("inputs/key-alias");
    fs::hard_link(author.root.join("signing.pk8"), &alias).unwrap();
    author.settings["runtimes"]["npm"]["files"].as_array_mut().unwrap().push(json!({"source":"inputs/key-alias","path":"secrets/key","sha256":sha256_file(&alias).unwrap()}));
    assert!(author.reject().contains("cannot be included"));
    let mut author = self::runtime_author();
    symlink("node", author.root.join("inputs/npm/bin/link")).unwrap();
    author.settings["runtimes"]["npm"]["files"][0]["source"] = json!("inputs/npm/bin/link");
    author.reject();
}
