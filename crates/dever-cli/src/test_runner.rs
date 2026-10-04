use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicU64, Ordering};

use dever_core::hir::{Program, TestCase};
use dever_core::source::SourceMap;

pub(super) fn execute(program: &Program, sources: &SourceMap) -> Result<ExitCode, String> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "running {} tests", program.tests().len()).map_err(io_error)?;

    let mut passed = 0;
    let mut failures = Vec::new();
    if !program.tests().is_empty() {
        match crate::compile::test_suite(program, sources) {
            Ok(native) => {
                for (index, test) in program.tests().iter().enumerate() {
                    match run_case(program, test, &native, index) {
                        Ok(()) => {
                            passed += 1;
                            writeln!(stdout, "test {} ... ok", test.name()).map_err(io_error)?;
                        }
                        Err(failure) => {
                            writeln!(stdout, "test {} ... FAILED", test.name())
                                .map_err(io_error)?;
                            failures.push((test.name(), failure));
                        }
                    }
                }
            }
            Err(message) => {
                for test in program.tests() {
                    writeln!(stdout, "test {} ... FAILED", test.name()).map_err(io_error)?;
                    failures.push((test.name(), CaseFailure::message(message.clone())));
                }
            }
        }
    }

    for (name, failure) in &failures {
        writeln!(stdout, "\n---- {name} failure ----").map_err(io_error)?;
        writeln!(stdout, "{}", failure.message).map_err(io_error)?;
        write_output(&mut stdout, "stdout", &failure.stdout)?;
        write_output(&mut stdout, "stderr", &failure.stderr)?;
    }

    let failed = failures.len();
    let status = if failed == 0 { "ok" } else { "FAILED" };
    writeln!(
        stdout,
        "\ntest result: {status}. {passed} passed; {failed} failed; 0 not run"
    )
    .map_err(io_error)?;
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn run_case(
    program: &Program,
    test: &TestCase,
    native: &dever_core::native::NativeProgram,
    index: usize,
) -> Result<(), CaseFailure> {
    let project = TemporaryProject::new().map_err(CaseFailure::message)?;
    if test.uses_database() {
        write_settings(project.path(), test).map_err(CaseFailure::message)?;
        let settings = dever_runtime::config::Settings::load_project(project.path())
            .map_err(CaseFailure::message)?;
        let profile = program
            .validate_database_settings(&settings)
            .map_err(CaseFailure::message)?;
        if !profile.sqlite || profile.postgres {
            return Err(CaseFailure::message(
                "generated test settings must select SQLite only".into(),
            ));
        }
    }

    let executable = project
        .path()
        .join(format!("test-program{}", std::env::consts::EXE_SUFFIX));
    native.save(&executable).map_err(|error| {
        CaseFailure::message(format!(
            "cannot stage test executable '{}': {error}",
            executable.display()
        ))
    })?;
    let output = Command::new(&executable)
        .arg(index.to_string())
        .current_dir(project.path())
        .output()
        .map_err(|error| CaseFailure::message(format!("cannot run test process: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CaseFailure {
            message: format!("test process exited with {}", output.status),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

fn write_settings(root: &Path, test: &TestCase) -> Result<(), String> {
    let config = root.join("config");
    fs::create_dir(&config)
        .map_err(|error| format!("cannot create '{}': {error}", config.display()))?;
    let mut databases = serde_json::Map::new();
    for name in test.database_connections() {
        databases.insert(
            name.clone(),
            serde_json::json!({
                "type": "sqlite",
                "path": format!("data/db/{name}.db"),
                "max_connections": 1,
                "max_page_size": 100
            }),
        );
    }
    let document = serde_json::json!({ "database": databases });
    let path = config.join("setting.json");
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| format!("cannot encode test settings: {error}"))?;
    fs::write(&path, bytes).map_err(|error| format!("cannot write '{}': {error}", path.display()))
}

fn write_output(output: &mut impl Write, label: &str, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    writeln!(output, "---- {label} ----").map_err(io_error)?;
    output.write_all(bytes).map_err(io_error)?;
    if !bytes.ends_with(b"\n") {
        writeln!(output).map_err(io_error)?;
    }
    Ok(())
}

fn io_error(error: io::Error) -> String {
    error.to_string()
}

struct CaseFailure {
    message: String,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl CaseFailure {
    fn message(message: String) -> Self {
        Self {
            message,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

struct TemporaryProject(PathBuf);

impl TemporaryProject {
    fn new() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "dever-application-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "cannot create temporary test project '{}': {error}",
                        path.display()
                    ));
                }
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
