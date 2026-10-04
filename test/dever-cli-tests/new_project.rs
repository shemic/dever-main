use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use dever_core::source::SourceMap;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("dever-new-test-{}-{sequence}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn both_formats_create_checked_formatted_projects_with_real_application_tests() {
    let workspace = Workspace::new();
    for markdown in [false, true] {
        let root = workspace
            .0
            .join(if markdown { "markdown" } else { "plain" });
        dever_cli::project::create(&root, markdown).unwrap();
        let sources = SourceMap::load_project(&root.join("module"), &root.join("test")).unwrap();
        let result = dever_core::check(&sources);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(sources.files().len(), 3);
        for source in sources.files() {
            let formatted = dever_core::format::format(source).unwrap();
            assert_eq!(formatted, source.text(), "{}", source.path().display());
        }
        let settings: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("config/setting.json")).unwrap()).unwrap();
        assert_eq!(settings["log"]["level"], "info");
        assert!(root.join("AGENTS.md").is_file());
        assert!(!root.join("module/main.dever").exists());
    }
}

#[test]
fn existing_directories_files_and_symlinks_are_preserved() {
    let workspace = Workspace::new();
    let directory = workspace.0.join("existing");
    fs::create_dir(&directory).unwrap();
    assert!(dever_cli::project::create(&directory, false).is_err());
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
    let file = workspace.0.join("file");
    fs::write(&file, "keep me").unwrap();
    assert!(dever_cli::project::create(&file, false).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "keep me");
    #[cfg(unix)]
    {
        let link = workspace.0.join("link");
        std::os::unix::fs::symlink(workspace.0.join("absent"), &link).unwrap();
        assert!(dever_cli::project::create(&link, true).is_err());
        assert!(fs::symlink_metadata(link).unwrap().is_symlink());
    }
    assert!(fs::read_dir(&workspace.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".dever-new-")
    }));
}

#[test]
fn concurrent_creation_publishes_one_complete_project_without_staging_leftovers() {
    let workspace = Workspace::new();
    let root = workspace.0.join("project");
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let workers: Vec<_> = [false, true]
            .into_iter()
            .map(|markdown| {
                let root = &root;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    dever_cli::project::create(root, markdown)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let sources = SourceMap::load_project(&root.join("module"), &root.join("test")).unwrap();
    assert!(dever_core::check(&sources).is_ok());
    assert_eq!(sources.files().len(), 3);
    assert_eq!(fs::read_dir(&workspace.0).unwrap().count(), 1);
}

#[test]
fn command_creates_before_reading_project_configuration_and_rejects_bad_arguments() {
    let workspace = Workspace::new();
    let output = Command::new(env!("CARGO_BIN_EXE_dever"))
        .env_clear()
        .current_dir(&workspace.0)
        .args(["new", "app", "--markdown"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        workspace
            .0
            .join("app/module/hello/greeting/api.dever.md")
            .is_file()
    );
    for arguments in [
        vec!["new"],
        vec!["new", "bad", "--unknown"],
        vec!["new", "--markdown"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dever"))
            .env_clear()
            .current_dir(&workspace.0)
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
    }
    assert!(!workspace.0.join("bad").exists());
}
