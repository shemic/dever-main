//! Owned signed CLI and relocated output; the tool is an ordinary dynamic ELF.
use super::*;
use dever_cli::libs::LockFile;
use dever_cli::packages::LockedPackage;
use dever_cli::toolchain::artifact_put;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};

fn project(root: &Path, workspace: &Path) {
    write(
        root,
        "module/media/tool/port.dever",
        "execute(args: List<Text>, stdin: Bytes?) (output: dever.process.Output) fails dever.process.Error\n",
    );
    write(
        root,
        "module/media/tool/adapter.dever",
        "external command \"bin/tool\" {}\n",
    );
    write(
        root,
        "module/media/tool/app.dever",
        r#"execute() (passed: Bool) {
  input = dever.bytes.from_ints([0, 255, 128, 10])
  output = port.execute(["a b", "$(literal);*"], input)
  passed = output.code == 23 and output.stdout == input and dever.bytes.to_text(output.stderr) == "ordinary command"
}
"#,
    );
    write(
        root,
        "module/media/tool/api.dever",
        "cmd execute = app.execute\n",
    );
    write(root, "config/setting.json", "{}");
    let directory = root.join("module/media/tool/bin");
    fs::create_dir_all(directory.join("lib")).unwrap();
    command(
        Command::new("/usr/bin/cc")
            .arg("-B/usr/bin/")
            .args(["-shared", "-fPIC", "-Wl,-soname,libcommand.so", "-o"])
            .arg(directory.join("lib/libcommand.so"))
            .arg(workspace.join("test/dever-cli-tests/support/command_library.c")),
    );
    command(
        Command::new("/usr/bin/cc")
            .arg("-B/usr/bin/")
            .arg(workspace.join("test/dever-cli-tests/support/command_tool.c"))
            .arg("-L")
            .arg(directory.join("lib"))
            .arg("-lcommand")
            .arg("-o")
            .arg(directory.join("tool")),
    );
}

fn packaged_project(root: &Path, source: &Path, layout: &Layout) {
    let manifest = br#"{"format":"dever-package-v1","name":"media","version":"1.0.0","dependencies":{},"lib":[]}"#;
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        "dever-package.json",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(manifest).unwrap();
    for file in [
        "port.dever",
        "adapter.dever",
        "app.dever",
        "api.dever",
        "bin/tool",
        "bin/lib/libcommand.so",
    ] {
        let name = format!("module/media/tool/{file}");
        zip.start_file(&name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&fs::read(source.join(name)).unwrap())
            .unwrap();
    }
    let archive = zip.finish().unwrap().into_inner();
    let receipt = artifact_put(layout, &archive).unwrap();
    fs::create_dir_all(root.join("module")).unwrap();
    write(
        root,
        "config/setting.json",
        r#"{"package":{"registry":"https://packages.example.test","use":["media@1.0.0"]}}"#,
    );
    let mut lock = LockFile::new(Vec::new()).unwrap();
    lock.packages.push(LockedPackage {
        name: "media".into(),
        version: "1.0.0".into(),
        sha256: receipt.sha256,
        bytes: archive.len() as u64,
        dependencies: Vec::new(),
        manifest_sha256: format!("{:x}", Sha256::digest(manifest)),
    });
    lock.write_atomic(root).unwrap();
}

#[test]
#[ignore = "requires explicit current runtime archive, author C/LLVM inputs and owned daemon/sandbox"]
fn signed_cli_runs_and_packages_ordinary_command_with_private_library() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let author = TemporaryDirectory::new();
    let key = author_inputs(author.path(), &workspace);
    // This test validates the command boundary, not release-profile trimming.
    // Use the explicitly rebuilt archive containing the current wire ABI.
    let current = input(
        author.path(),
        &workspace.join("target/native-runtime-abi/debug/libdever_backend_bridge.a"),
        "command-base.a",
    );
    let settings_path = author.path().join("config/setting.json");
    let mut settings: Value = serde_json::from_slice(&fs::read(&settings_path).unwrap()).unwrap();
    settings["profiles"]["base"] = current;
    fs::write(settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();

    let machine = machine_temporary_directory(&workspace);
    let layout = Layout::new(machine.path());
    layout.initialize().unwrap();
    let version = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let release = layout.downloads().join(version.as_str());
    packaging::create(author.path(), &release).unwrap();
    install_release(&layout, &version, &key);
    let compiler = layout.versions().join(version.as_str()).join("dever-core");
    let mut daemon = daemon::TestDaemon::start(&layout);
    let source = TemporaryDirectory::new();
    project(source.path(), &workspace);
    command(Command::new(&compiler).arg("check").arg(source.path()));
    assert_response(command(
        Command::new(&compiler).arg("run").arg(source.path()).args([
            "--",
            "media.tool.execute",
            "{}",
        ]),
    ));
    assert!(!source.path().join("dever.lock").exists());

    let deployment = TemporaryDirectory::new();
    let executable = deployment.path().join("program");
    command(
        Command::new(&compiler)
            .arg("build")
            .arg(source.path())
            .arg("--output")
            .arg(&executable),
    );
    assert_eq!(cache_status(&layout).unwrap().entries, 1);
    let package_source = TemporaryDirectory::new();
    packaged_project(package_source.path(), source.path(), &layout);
    assert_response(command(
        Command::new(&compiler)
            .arg("run")
            .arg(package_source.path())
            .args(["--", "media.tool.execute", "{}"]),
    ));
    let package_executable = deployment.path().join("package-program");
    command(
        Command::new(&compiler)
            .arg("build")
            .arg(package_source.path())
            .arg("--output")
            .arg(&package_executable),
    );
    write(deployment.path(), "config/setting.json", "{}");
    daemon.stop();
    drop(source);
    drop(package_source);
    drop(author);
    drop(machine);
    assert_response(command(
        Command::new(&executable)
            .args(["media.tool.execute", "{}"])
            .current_dir(deployment.path()),
    ));
    assert_response(command(
        Command::new(&package_executable)
            .args(["media.tool.execute", "{}"])
            .current_dir(deployment.path()),
    ));
    let cache = deployment.path().join("data/cache/lib");
    let bundle = fs::read_dir(cache).unwrap().next().unwrap().unwrap().path();
    let commands = bundle.join("commands");
    let tree = fs::read_dir(commands)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(tree.join("lib/libcommand.so").is_file());
    fs::write(tree.join("lib/libcommand.so"), b"tampered").unwrap();
    let capture = deployment.path().join("tamper.stderr");
    let status = process::status(
        Command::new(&executable)
            .env_clear()
            .current_dir(deployment.path())
            .args(["media.tool.execute", "{}"])
            .stdout(Stdio::null())
            .stderr(Stdio::from(fs::File::create(&capture).unwrap())),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(!status.success());
    assert!(fs::read_to_string(capture).unwrap().contains("resource"));
}
