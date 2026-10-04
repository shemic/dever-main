//! Explicit x86 compiler -> ARM application acceptance; QEMU user mode does
//! not establish the namespace/seccomp contract of external Workers.
use super::*;
use dever_cli::toolchain::BuildTarget;
use dever_cli::toolchain::compilation::{CompileKind, CompileRequest};
use dever_core::source::SourceMap;
use dever_runtime::config::Settings;
use std::io::{Read, Seek, SeekFrom, Write};

#[path = "cross_ecosystems.rs"]
mod ecosystems;

fn add_arm_inputs(author: &Path, workspace: &Path) {
    let prepared = workspace.join("target/arm64-cross");
    let mut profiles = serde_json::Map::new();
    for profile in PROFILES {
        let name = format!("{profile}.a");
        profiles.insert(
            profile.into(),
            input(
                author,
                &prepared.join("archives").join(&name),
                &format!("arm/{name}"),
            ),
        );
    }
    let libc = prepared.join("tools/usr/aarch64-linux-gnu/lib");
    let gcc = prepared.join("tools/usr/lib/gcc-cross/aarch64-linux-gnu/13");
    let links = |root: &Path, names: &[&str]| -> Vec<Value> {
        names
            .iter()
            .map(|name| {
                let mut entry = input(author, &root.join(name), &format!("arm/{name}"));
                entry["path"] = json!(name);
                entry
            })
            .collect()
    };
    let start = [
        links(&libc, &["crt1.o", "crti.o"]),
        links(&gcc, &["crtbeginT.o"]),
    ]
    .concat();
    let libraries = [
        links(&libc, &["libc.a", "libm-2.39.a", "libmvec.a"]),
        links(&gcc, &["libgcc.a", "libgcc_eh.a"]),
    ]
    .concat();
    let end = [links(&gcc, &["crtend.o"]), links(&libc, &["crtn.o"])].concat();
    let path = author.join("config/setting.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    settings["native_targets"] = json!({"linux-aarch64": {"profiles": profiles, "start":start, "libraries":libraries, "end":end}});
    fs::write(path, serde_json::to_vec(&settings).unwrap()).unwrap();
}

fn build(compiler: &Path, project: &Path, output: &Path, target: BuildTarget) {
    command(
        Command::new(compiler)
            .arg("build")
            .arg(project)
            .args(["--target", target.platform(), "--output"])
            .arg(output),
    );
    let bytes = fs::read(output).unwrap();
    let elf = goblin::elf::Elf::parse(&bytes).unwrap();
    assert!(elf.is_64 && elf.little_endian);
    assert!(elf.interpreter.is_none() && elf.libraries.is_empty());
    assert_eq!(
        elf.header.e_machine,
        match target {
            BuildTarget::LinuxX86_64 => goblin::elf::header::EM_X86_64,
            BuildTarget::LinuxAarch64 => goblin::elf::header::EM_AARCH64,
        }
    );
}

fn request(project: &Path, target: BuildTarget) -> CompileRequest {
    let sources = SourceMap::load_with_packages(&project.join("module"), None, &[]).unwrap();
    let bindings = Settings::load_project(project)
        .unwrap()
        .compilation_bindings();
    CompileRequest::new(
        env!("CARGO_PKG_VERSION").into(),
        CompileKind::Application,
        &sources,
        Some(bindings),
        &[],
        target,
    )
    .unwrap()
}

fn private_compiler(
    installed: &Path,
    destination: &Path,
    manifest: &dever_cli::toolchain::ReleaseManifest,
) -> PathBuf {
    for artifact in &manifest.artifacts {
        if artifact.path != "dever-core"
            && !artifact.path.starts_with("lib/")
            && !artifact.path.starts_with("runtime/linux-aarch64/")
        {
            continue;
        }
        let relative = if artifact.path == "dever-core" {
            "dever"
        } else {
            &artifact.path
        };
        let output = destination.join(relative);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::hard_link(installed.join(&artifact.path), output).unwrap();
    }
    destination.join("dever")
}

fn reject_tampered_cache_hit(layout: &Layout, archive: &Path, request: &CompileRequest) {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(archive)
        .unwrap();
    let mut original = [0; 1];
    file.read_exact(&mut original).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&[original[0] ^ 1]).unwrap();
    file.sync_all().unwrap();
    let rejected = dever_cli::toolchain::compile(layout, request);
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&original).unwrap();
    file.sync_all().unwrap();
    assert!(rejected.unwrap_err().contains("integrity verification"));
}

fn export_http_application(compiler: &Path, projects: &Path, workspace: &Path) -> Value {
    let project = projects.join("http");
    for (path, source) in [
        (
            "user/account/model.dever",
            "global type Account { name: Text(1, 32) }",
        ),
        (
            "platform/tenant/model.dever",
            "global type Tenant { name: Text(1, 32) }",
        ),
        (
            "user/account/app.dever",
            "type Identity { id: Text\nuser_id: user.account.model.id?\ntenant_id: platform.tenant.model.id? }\nverify(claims: dever.auth.Claims) (identity: Identity) { identity = Identity { id = claims.subject\nuser_id = null\ntenant_id = null } }",
        ),
        (
            "sample/http/app.dever",
            "hello() (message: Text) { message = \"arm-http\" }",
        ),
        ("sample/http/api.dever", "public get hello = app.hello"),
    ] {
        write(&project, &format!("module/{path}"), source);
    }
    let settings = serde_json::to_vec(&json!({
        "http":{"host":"127.0.0.1","port":18080},
        "runtime":{"mode":"api","shutdown_ms":2000}, "log":{"level":"error"},
        "database":{"default":{"type":"sqlite","path":"data/http.sqlite","max_connections":1}},
        "auth":{"providers":{"session":{"verify":"user.account.verify","jwtSecret":"cross-acceptance-only-test-key-2026"}}},
        "sites":{"test":{"path":"","auth":"session"}}
    })).unwrap();
    write(&project, "config/setting.json", &settings);
    let program = project.join("application");
    build(compiler, &project, &program, BuildTarget::LinuxAarch64);
    let exported = workspace.join("target/arm64-cross/guest/apps/http");
    write(&exported, "config/setting.json", settings);
    fs::copy(&program, exported.join("application")).unwrap();
    json!({"sha256":sha256_file(&program).unwrap(), "profile":"sqlite", "url":"http://127.0.0.1:18080/sample/http/hello", "expected_data":"arm-http", "execution":"pending_guest_validation"})
}

#[test]
#[ignore = "requires prepared x86/ARM four-profile runtime archives and private QEMU; starts only an owned daemon"]
fn signed_host_compiler_builds_arm_profiles_and_runs_standalone_under_qemu() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let emulator = workspace.join("target/arm64-cross/tools/usr/bin/qemu-aarch64-static");
    assert!(
        emulator.is_file(),
        "prepare the task-owned static QEMU emulator"
    );
    let author = TemporaryDirectory::new();
    let key = author_inputs(author.path(), &workspace);
    add_arm_inputs(author.path(), &workspace);
    let machine = machine_temporary_directory(&workspace);
    let layout = Layout::new(machine.path());
    layout.initialize().unwrap();
    let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let release = layout.downloads().join(version.as_str());
    let manifest = packaging::create(author.path(), &release).unwrap();
    assert_eq!(manifest.platform, BuildTarget::LinuxX86_64.platform());
    install_release(&layout, &version, &key);
    fs::remove_dir_all(&release).unwrap();
    drop(author);
    let compiler = layout.versions().join(version.as_str()).join("dever-core");
    let mut daemon = daemon::TestDaemon::start(&layout);
    let projects = TemporaryDirectory::new();
    let standalone = TemporaryDirectory::new();
    let mut programs = Vec::new();
    let mut evidence = serde_json::Map::new();
    for profile in PROFILES {
        let project = projects.path().join(profile);
        application(&project, profile);
        let destination = standalone.path().join(profile);
        fs::create_dir_all(destination.join("config")).unwrap();
        fs::copy(
            project.join("config/setting.json"),
            destination.join("config/setting.json"),
        )
        .unwrap();
        let executable = destination.join("application");
        build(&compiler, &project, &executable, BuildTarget::LinuxAarch64);
        let before = cache_status(&layout).unwrap().entries;
        let repeated = project.join("repeated");
        build(&compiler, &project, &repeated, BuildTarget::LinuxAarch64);
        assert_eq!(
            sha256_file(&executable).unwrap(),
            sha256_file(&repeated).unwrap()
        );
        assert_eq!(cache_status(&layout).unwrap().entries, before);
        evidence.insert(profile.into(), json!({"sha256":sha256_file(&executable).unwrap(), "bytes":fs::metadata(&executable).unwrap().len(), "cache_hit":true}));
        programs.push((destination, executable));
    }
    assert_eq!(cache_status(&layout).unwrap().entries, 4);
    let base_project = projects.path().join("base");
    let host = standalone.path().join("base/host-application");
    build(&compiler, &base_project, &host, BuildTarget::LinuxX86_64);
    assert_response(command(
        Command::new(&host).args(["sample.value.exercise", "{}"]),
    ));
    assert_eq!(cache_status(&layout).unwrap().entries, 5);

    let private = private_compiler(
        compiler.parent().unwrap(),
        &machine.path().join("private"),
        &manifest,
    );
    let private_directory = standalone.path().join("private-base");
    write(&private_directory, "config/setting.json", b"{}");
    let private_output = private_directory.join("application");
    build(
        &private,
        &base_project,
        &private_output,
        BuildTarget::LinuxAarch64,
    );
    programs.push((private_directory, private_output));

    // Use the same canonical request to prove integrity is checked before a hit.
    let arm_request = request(&base_project, BuildTarget::LinuxAarch64);
    assert_eq!(
        dever_cli::toolchain::compile(&layout, &arm_request).unwrap(),
        fs::read(&programs[0].1).unwrap()
    );
    let archive = compiler
        .parent()
        .unwrap()
        .join("runtime/linux-aarch64/archives/base.a");
    reject_tampered_cache_hit(&layout, &archive, &arm_request);
    assert_eq!(cache_status(&layout).unwrap().entries, 5);
    let http = export_http_application(&compiler, projects.path(), &workspace);
    assert_eq!(cache_status(&layout).unwrap().entries, 6);
    daemon.stop();
    drop(projects);
    drop(machine);

    for (directory, executable) in programs {
        assert!(!directory.join("module").exists());
        assert_response(command(
            Command::new(&emulator)
                .arg(executable)
                .current_dir(directory)
                .args(["sample.value.exercise", "{}"]),
        ));
    }
    let report = json!({
        "format":"dever-cross-application-acceptance-v1", "host":"linux-x86_64", "target":"linux-aarch64",
        "programs":evidence, "http":http, "qemu_sha256":sha256_file(&emulator).unwrap(),
        "checks":{"signed_install":true,"profiles":4,"private_arm_build":true,"standalone_after_source_and_machine_removal":true,"sqlite_crud":true,"cache_entries":6,"cache_target_isolation":true,"tampered_target_cache_hit_rejected":true},
        "postgres_server":"not_run", "worker_sandbox":"requires_arm_kernel_system_emulation"
    });
    write(
        &workspace,
        "target/arm64-cross/compile-acceptance.json",
        serde_json::to_vec_pretty(&report).unwrap(),
    );
}
