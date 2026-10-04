use super::*;
use dever_cli::toolchain::BuildTarget;

fn command_fixture(root: &Path, binary: &[u8]) -> dever_core::hir::Program {
    let mut sources = SourceMap::default();
    sources.add(
        "media/tool/port.dever",
        "execute(args: List<Text>) (output: dever.process.Output) fails dever.process.Error",
    );
    sources.add(
        "media/tool/adapter.dever",
        "external command \"bin/tool\" {}\n",
    );
    sources.add("media/tool/app.dever", "execute() (code: Int) {\n  output = port.execute([\"a b\", \"$(literal)\"])\n  code = output.code\n}\n");
    sources.add("media/tool/api.dever", "cmd execute = app.execute\n");
    let program = dever_core::check(&sources).unwrap_or_else(|errors| panic!("{errors:?}"));
    let entry = &program.external_command_contracts()[0].entry;
    fs::create_dir_all(root.join(entry).parent().unwrap()).unwrap();
    fs::write(root.join(entry), binary).unwrap();
    program
}

fn command_manifest(resources: &[EmbeddedResource]) -> serde_json::Value {
    let manifest = resources
        .iter()
        .find(|resource| resource.path.ends_with(".dever-command.json"))
        .unwrap();
    serde_json::from_slice(&manifest.bytes).unwrap()
}

#[test]
fn command_preparation_needs_no_ecosystem_lock_and_keeps_target_identity() {
    for (target, machine) in [
        (BuildTarget::LinuxX86_64, 62u16),
        (BuildTarget::LinuxAarch64, 183u16),
    ] {
        let temp = TemporaryDirectory::new();
        let mut binary = native_elf::static_executable();
        binary[18..20].copy_from_slice(&machine.to_le_bytes());
        let program = command_fixture(temp.path(), &binary);
        assert!(program.external_worker_contracts().is_empty());
        assert!(
            dever_cli::libs::prepare_program(temp.path(), &program, target)
                .unwrap()
                .is_none()
        );
        let resources =
            workers::prepare_for_target(temp.path(), &program, &[], target, &[]).unwrap();
        let manifest = command_manifest(&resources);
        assert_eq!(manifest["format"], "dever-command-launch-v1");
        assert_eq!(manifest["arguments"], json!([]));
        assert_eq!(manifest["ecosystem"], "command");
        let executable = resources
            .iter()
            .find(|resource| Some(resource.path.as_str()) == manifest["executable"].as_str())
            .unwrap();
        assert!(executable.executable);
        assert_eq!(executable.bytes, binary);
        let other = if target == BuildTarget::LinuxX86_64 {
            BuildTarget::LinuxAarch64
        } else {
            BuildTarget::LinuxX86_64
        };
        assert!(
            workers::prepare_for_target(temp.path(), &program, &[], other, &[])
                .unwrap_err()
                .contains("differs from target")
        );
        assert!(!temp.path().join("dever.lock").exists());
    }
}

#[test]
fn command_preparation_checks_recursive_library_closure_and_loader() {
    let temp = TemporaryDirectory::new();
    let program = command_fixture(temp.path(), &native_elf::compiler(&["libfirst.so"]));
    assert!(
        workers::prepare(temp.path(), &program, &[])
            .unwrap_err()
            .contains("libfirst.so")
    );
    let lib = temp.path().join("module/media/tool/bin/lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(
        lib.join("libfirst.so"),
        native_elf::library("libfirst.so", &["libsecond.so"]),
    )
    .unwrap();
    assert!(
        workers::prepare(temp.path(), &program, &[])
            .unwrap_err()
            .contains("libsecond.so")
    );
    fs::write(
        lib.join("libsecond.so"),
        native_elf::library("libwrong.so", &[]),
    )
    .unwrap();
    assert!(
        workers::prepare(temp.path(), &program, &[])
            .unwrap_err()
            .contains("SONAME")
    );
    fs::write(
        lib.join("libsecond.so"),
        native_elf::library("libsecond.so", &[]),
    )
    .unwrap();
    assert!(
        workers::prepare(temp.path(), &program, &[])
            .unwrap_err()
            .contains("signed target GNU loader")
    );
    let loader_name = dever_sandbox::loader_name();
    let mut loader = resource(
        format!("sandbox/lib/{loader_name}"),
        native_elf::static_executable(),
    );
    loader.executable = true;
    let resources = workers::prepare(temp.path(), &program, &[loader]).unwrap();
    let manifest = command_manifest(&resources);
    assert!(
        manifest["executable"]
            .as_str()
            .unwrap()
            .ends_with("/loader")
    );
    assert_eq!(manifest["arguments"][0]["literal"], "--inhibit-cache");
    for name in ["libfirst.so", "libsecond.so"] {
        assert!(resources.iter().any(
            |resource| resource.path.ends_with(&format!("/lib/{name}")) && !resource.executable
        ));
    }
}

#[test]
fn command_rejects_host_library_paths_and_signed_library_shadowing() {
    let temp = TemporaryDirectory::new();
    let program = command_fixture(
        temp.path(),
        &native_elf::elf(None, &["libfirst.so"], Some("/usr/lib")),
    );
    assert!(
        workers::prepare(temp.path(), &program, &[])
            .unwrap_err()
            .contains("$ORIGIN")
    );
    let entry = &program.external_command_contracts()[0].entry;
    for path in ["$ORIGIN/../data/grant", "${ORIGIN}/../lib"] {
        fs::write(
            temp.path().join(entry),
            native_elf::elf(None, &[], Some(path)),
        )
        .unwrap();
        assert!(
            workers::prepare(temp.path(), &program, &[])
                .unwrap_err()
                .contains("escapes its packaged tree")
        );
    }
    fs::write(
        temp.path().join(entry),
        native_elf::compiler(&["libfirst.so"]),
    )
    .unwrap();
    let lib = temp.path().join("module/media/tool/bin/lib");
    fs::create_dir_all(&lib).unwrap();
    fs::write(
        lib.join("libfirst.so"),
        native_elf::library("libfirst.so", &[]),
    )
    .unwrap();
    let signed = resource(
        "sandbox/lib/libfirst.so".into(),
        native_elf::library("libfirst.so", &[]),
    );
    assert!(
        workers::prepare(temp.path(), &program, &[signed])
            .unwrap_err()
            .contains("overrides a signed sandbox library")
    );
}

#[cfg(unix)]
#[test]
fn command_rejects_symlinks_and_library_subdirectories() {
    let temp = TemporaryDirectory::new();
    let program = command_fixture(temp.path(), &native_elf::static_executable());
    let lib = temp.path().join("module/media/tool/bin/lib");
    fs::create_dir_all(&lib).unwrap();
    std::os::unix::fs::symlink("../tool", lib.join("libalias.so")).unwrap();
    assert!(
        workers::prepare(temp.path(), &program, &[])
            .unwrap_err()
            .contains("regular library files")
    );
    fs::remove_file(lib.join("libalias.so")).unwrap();
    fs::create_dir(lib.join("hidden")).unwrap();
    assert!(
        workers::prepare(temp.path(), &program, &[])
            .unwrap_err()
            .contains("regular library files")
    );
}
