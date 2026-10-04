use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::hir::Program;
use crate::source::SourceMap;

mod cache;
mod cleanup;
use cleanup::{BuildDirectory, build_directory, build_directory_at, clean_stale_build_directories};
pub use cleanup::{CleanupSummary, clean_project_artifacts};

/// 单个生成源码统一优化，避免分区使标准集合的泛型内联取决于程序大小。
pub const OPTIMIZATION_ARGS: [&str; 8] = [
    "-C",
    "opt-level=3",
    "-C",
    "codegen-units=1",
    "-C",
    "lto=fat",
    "-C",
    "strip=symbols",
];

const TEST_COMPILATION_ARGS: [&str; 6] = [
    "-C",
    "opt-level=1",
    "-C",
    "codegen-units=16",
    "-C",
    "strip=symbols",
];

/// Owns the temporary build directory for as long as the executable is needed.
pub struct NativeProgram {
    directory: BuildDirectory,
    executable: PathBuf,
    cache_hit: bool,
}

impl NativeProgram {
    /// The authenticated compile client verifies the service response before
    /// transferring these bytes into a caller-owned executable lifetime.
    pub fn from_verified_bytes(bytes: &[u8]) -> Result<Self, String> {
        let artifact = Self::new()?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o700);
        }
        let mut output = options
            .open(&artifact.executable)
            .map_err(|error| format!("cannot create compiled executable: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output
                .set_permissions(fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("cannot set compiled executable permissions: {error}"))?;
        }
        output
            .write_all(bytes)
            .and_then(|()| output.sync_all())
            .map_err(|error| format!("cannot write compiled executable: {error}"))?;
        Ok(artifact)
    }

    fn new() -> Result<Self, String> {
        let directory = build_directory()
            .map_err(|error| format!("cannot create native build directory: {error}"))?;
        Ok(Self {
            executable: directory.join(format!("program{}", std::env::consts::EXE_SUFFIX)),
            directory: BuildDirectory(directory),
            cache_hit: false,
        })
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn cache_hit(&self) -> bool {
        self.cache_hit
    }

    /// Creates a new output file; existing files (including sources) are never replaced.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut input = File::open(&self.executable)?;
        let mut output = OpenOptions::new().write(true).create_new(true).open(path)?;
        let result = (|| {
            io::copy(&mut input, &mut output)?;
            output.set_permissions(input.metadata()?.permissions())?;
            output.sync_all()
        })();
        if result.is_err() {
            drop(output);
            let _ = fs::remove_file(path);
        }
        result
    }
}

/// Compile already identified inputs using the shared private artifact owner.
/// The callback receives the owned build directory and a new executable path.
/// It must link only the exact inputs represented by `identity`.
pub fn compile_artifact(
    cache_directory: &Path,
    identity: &str,
    build: impl FnOnce(&Path, &Path) -> Result<(), String>,
) -> Result<NativeProgram, String> {
    let _ = clean_stale_build_directories(SystemTime::now());
    let cache = cache::Cache::for_artifact(cache_directory, identity);
    compile_cached(&cache, cache_directory, |artifact| {
        build(&artifact.directory.0, &artifact.executable)
    })
}

fn compile_cached(
    cache: &cache::Cache,
    cache_directory: &Path,
    build: impl FnOnce(&NativeProgram) -> Result<(), String>,
) -> Result<NativeProgram, String> {
    let mut artifact = NativeProgram::new()?;
    if cache
        .restore(&artifact.executable)
        .map_err(|error| format!("cannot restore native cache: {error}"))?
    {
        artifact.cache_hit = true;
        return Ok(artifact);
    }
    build(&artifact)?;
    publish_artifact(cache, cache_directory, &artifact)?;
    Ok(artifact)
}

fn publish_artifact(
    cache: &cache::Cache,
    cache_directory: &Path,
    artifact: &NativeProgram,
) -> Result<(), String> {
    fs::create_dir_all(cache_directory)
        .map_err(|error| format!("cannot create native cache: {error}"))?;
    let staging = BuildDirectory(
        build_directory_at(cache_directory)
            .map_err(|error| format!("cannot stage native cache: {error}"))?,
    );
    cache
        .publish(&artifact.executable, &staging.0)
        .map_err(|error| format!("cannot publish native cache: {error}"))
}

pub fn compile(
    program: &Program,
    sources: &SourceMap,
    entry: &str,
    rustc: &OsStr,
) -> Result<NativeProgram, String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    compile_with_cache(
        program,
        sources,
        entry,
        rustc,
        &workspace.join("target/native-artifacts"),
    )
}

pub fn compile_application(
    program: &Program,
    sources: &SourceMap,
    entry: &str,
    rustc: &OsStr,
    profile: dever_runtime::config::RuntimeProfile,
) -> Result<NativeProgram, String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let entry = program.entry(entry)?;
    compile_entry(
        program,
        sources,
        entry,
        rustc,
        &workspace.join("target/native-artifacts"),
        Some(profile),
        &OPTIMIZATION_ARGS,
    )
}

pub fn compile_project(
    program: &Program,
    sources: &SourceMap,
    rustc: &OsStr,
    profile: dever_runtime::config::RuntimeProfile,
) -> Result<NativeProgram, String> {
    compile_project_with_resources(program, sources, rustc, profile, &[])
}

pub fn compile_project_with_resources(
    program: &Program,
    sources: &SourceMap,
    rustc: &OsStr,
    profile: dever_runtime::config::RuntimeProfile,
    resources: &[crate::native::EmbeddedResource],
) -> Result<NativeProgram, String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let generated = super::emit_project_with_resources(program, sources, profile, resources)?;
    let api = !program.api_routes.is_empty() || !program.api_rest.is_empty();
    compile_generated(
        GeneratedProgram {
            source: generated,
            resources,
        },
        rustc,
        &workspace.join("target/native-artifacts"),
        Some(profile),
        api,
        &OPTIMIZATION_ARGS,
    )
}

pub fn compile_test_suite(
    program: &Program,
    sources: &SourceMap,
    rustc: &OsStr,
) -> Result<NativeProgram, String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let profile = program
        .tests()
        .iter()
        .any(crate::hir::TestCase::uses_database)
        .then_some(dever_runtime::config::RuntimeProfile {
            sqlite: true,
            postgres: false,
        });
    let generated = super::emit_test_suite(program, sources, profile)?;
    compile_generated(
        GeneratedProgram {
            source: generated,
            resources: &[],
        },
        rustc,
        &workspace.join("target/native-artifacts"),
        profile,
        false,
        &TEST_COMPILATION_ARGS,
    )
}

/// The explicit cache directory lets embedding callers and tests own their artifacts.
pub fn compile_with_cache(
    program: &Program,
    sources: &SourceMap,
    entry: &str,
    rustc: &OsStr,
    cache_directory: &Path,
) -> Result<NativeProgram, String> {
    let entry = program.entry(entry)?;
    compile_entry(
        program,
        sources,
        entry,
        rustc,
        cache_directory,
        None,
        &OPTIMIZATION_ARGS,
    )
}

fn compile_entry(
    program: &Program,
    sources: &SourceMap,
    entry: usize,
    rustc: &OsStr,
    cache_directory: &Path,
    profile: Option<dever_runtime::config::RuntimeProfile>,
    compilation_args: &[&str],
) -> Result<NativeProgram, String> {
    let generated = super::emit_entry(program, sources, entry, profile)?;
    let api = super::api_enabled(program, entry)?;
    compile_generated(
        GeneratedProgram {
            source: generated,
            resources: &[],
        },
        rustc,
        cache_directory,
        profile,
        api,
        compilation_args,
    )
}

struct GeneratedProgram<'a> {
    source: String,
    resources: &'a [crate::native::EmbeddedResource],
}

fn compile_generated(
    generated: GeneratedProgram<'_>,
    rustc: &OsStr,
    cache_directory: &Path,
    profile: Option<dever_runtime::config::RuntimeProfile>,
    api: bool,
    compilation_args: &[&str],
) -> Result<NativeProgram, String> {
    // Stale cleanup is best effort and must never replace the current compile result.
    let _ = clean_stale_build_directories(SystemTime::now());
    let version = compiler_version(rustc)?;
    // Emitted code has already pruned unreachable functions and includes direct
    // system intrinsics as well as official source wrappers.
    let crypto = generated.source.contains("dever_runtime::crypto::");
    let wire = generated.source.contains("dever_runtime::wire::")
        || generated.source.contains("initialize_ports");
    let external = generated.source.contains("dever_runtime::external::");
    let runtime = runtime_library(rustc, profile, api, crypto, wire, external)?;
    let cache = cache::Cache::new(
        cache_directory,
        &generated.source,
        rustc,
        &version,
        &runtime,
        compilation_args,
    )
    .map_err(|error| format!("cannot identify native build inputs: {error}"))?;
    compile_cached(&cache, cache_directory, |artifact| {
        compile_executable(artifact, &generated, &runtime, rustc, compilation_args)
    })
}

fn compile_executable(
    artifact: &NativeProgram,
    generated: &GeneratedProgram<'_>,
    runtime: &cache::RuntimeInputs,
    rustc: &OsStr,
    compilation_args: &[&str],
) -> Result<(), String> {
    let runtime_directory = artifact.directory.0.join("runtime");
    runtime
        .write(&runtime_directory)
        .map_err(|error| format!("cannot stage native runtime: {error}"))?;
    let source = artifact.directory.0.join("program.rs");
    // Raw resources are separate compiler inputs. Expanding an interpreter
    // into decimal Rust literals multiplies source size and allocator use.
    for (index, resource) in super::resource_inputs(generated.resources)
        .unique
        .iter()
        .enumerate()
    {
        let path = artifact
            .directory
            .0
            .join(super::resource_source_filename(index));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .map_err(|error| format!("cannot stage external resource: {error}"))?;
        file.write_all(&resource.bytes)
            .map_err(|error| format!("cannot stage external resource: {error}"))?;
    }
    fs::write(&source, &generated.source)
        .map_err(|error| format!("cannot write generated program: {error}"))?;
    let result = Command::new(rustc)
        .arg("--edition=2024")
        .arg("--crate-name=dever_program")
        .args(compilation_args)
        .arg("--extern")
        .arg(format!(
            "dever_runtime={}",
            runtime_directory.join("libdever_runtime.rlib").display()
        ))
        .arg("-L")
        .arg(format!(
            "dependency={}",
            runtime_directory.join("deps").display()
        ))
        .arg(&source)
        .arg("-o")
        .arg(&artifact.executable)
        .output()
        .map_err(|error| {
            format!(
                "cannot invoke rustc '{}': {error}; set RUSTC or add rustc to PATH",
                rustc.to_string_lossy()
            )
        })?;
    if !result.status.success() {
        return Err(format!(
            "native compiler failed ({}):\n{}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(())
}

fn compiler_version(rustc: &OsStr) -> Result<Vec<u8>, String> {
    let result = Command::new(rustc)
        .arg("--version")
        .arg("--verbose")
        .output()
        .map_err(|error| {
            format!(
                "cannot invoke rustc '{}': {error}; set RUSTC or add rustc to PATH",
                rustc.to_string_lossy()
            )
        })?;
    if !result.status.success() {
        return Err(format!(
            "cannot query rustc version ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(result.stdout)
}

fn runtime_library(
    rustc: &OsStr,
    profile: Option<dever_runtime::config::RuntimeProfile>,
    api: bool,
    crypto: bool,
    wire: bool,
    external: bool,
) -> Result<cache::RuntimeInputs, String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime_root = workspace.join("target/native-runtime");
    let target = runtime_root.join("cargo");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| {
        Path::new(rustc)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| {
                parent
                    .join(format!("cargo{}", std::env::consts::EXE_SUFFIX))
                    .into_os_string()
            })
            .unwrap_or_else(|| "cargo".into())
    });
    let mut command = Command::new(cargo);
    command.args([
        "build",
        "--offline",
        "--locked",
        "--release",
        "--message-format=json-render-diagnostics",
        "-p",
        "dever-runtime",
    ]);
    let mut features = Vec::new();
    if let Some(profile) = profile {
        features.push(match (profile.sqlite, profile.postgres) {
            (true, true) => "sqlite,postgres",
            (true, false) => "sqlite",
            (false, true) => "postgres",
            (false, false) => "database",
        });
    }
    if api {
        features.push("api");
    }
    if crypto {
        features.push("crypto");
    }
    if wire {
        features.push("wire");
    }
    if external {
        features.push("external");
    }
    if !features.is_empty() {
        command.args(["--features", &features.join(",")]);
    }
    let result = command
        .arg("--manifest-path")
        .arg(workspace.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&target)
        .env("RUSTC", rustc)
        .output()
        .map_err(|error| format!("cannot prepare native runtime: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "native runtime compilation failed ({}):\n{}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    let artifacts = cargo_runtime_artifacts(
        &result.stdout,
        &target,
        &workspace.join("crates/dever-runtime/Cargo.toml"),
    )?;
    cache::RuntimeInputs::publish(
        &runtime_root.join("inputs"),
        &artifacts.runtime,
        &artifacts.dependencies,
    )
    .map_err(|error| format!("cannot publish native runtime inputs: {error}"))
}

struct CargoRuntimeArtifacts {
    runtime: PathBuf,
    dependencies: Vec<PathBuf>,
}

fn cargo_runtime_artifacts(
    bytes: &[u8],
    target: &Path,
    runtime_manifest: &Path,
) -> Result<CargoRuntimeArtifacts, String> {
    let dependency_root = fs::canonicalize(target.join("release/deps"))
        .map_err(|error| format!("cannot locate Cargo runtime dependencies: {error}"))?;
    let runtime_manifest = fs::canonicalize(runtime_manifest)
        .map_err(|error| format!("cannot locate dever-runtime manifest: {error}"))?;
    let mut runtime_candidates = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let message: serde_json::Value = serde_json::from_slice(line)
            .map_err(|error| format!("invalid Cargo artifact message: {error}"))?;
        if message.get("reason").and_then(serde_json::Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let target_name = message
            .pointer("/target/name")
            .and_then(serde_json::Value::as_str);
        let is_runtime = target_name == Some("dever_runtime")
            && message
                .get("manifest_path")
                .and_then(serde_json::Value::as_str)
                .and_then(|path| fs::canonicalize(path).ok())
                .is_some_and(|path| path == runtime_manifest);
        let Some(filenames) = message
            .get("filenames")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for filename in filenames.iter().filter_map(serde_json::Value::as_str) {
            let path = fs::canonicalize(filename).map_err(|error| {
                format!("cannot inspect Cargo runtime artifact '{filename}': {error}")
            })?;
            let extension = path.extension().and_then(OsStr::to_str);
            if is_runtime
                && extension == Some("rmeta")
                && path.parent() == Some(dependency_root.as_path())
            {
                let hashed_runtime = path.with_extension("rlib");
                if hashed_runtime.is_file() {
                    runtime_candidates.insert(fs::canonicalize(&hashed_runtime).map_err(
                        |error| {
                            format!(
                                "cannot inspect Cargo runtime artifact '{}': {error}",
                                hashed_runtime.display(),
                            )
                        },
                    )?);
                }
            }
            if !matches!(extension, Some("rlib" | "so" | "dylib" | "dll")) {
                continue;
            }
            if is_runtime && extension == Some("rlib") {
                runtime_candidates.insert(path.clone());
            }
            if path.parent() == Some(dependency_root.as_path()) {
                dependencies.insert(path);
            }
        }
    }
    let hashed_runtime = runtime_candidates
        .iter()
        .filter(|path| path.parent() == Some(dependency_root.as_path()))
        .cloned()
        .collect::<Vec<_>>();
    let runtime = match hashed_runtime.as_slice() {
        [runtime] => runtime.clone(),
        [] => match runtime_candidates
            .into_iter()
            .collect::<Vec<_>>()
            .as_slice()
        {
            [runtime] => runtime.clone(),
            [] => return Err("Cargo did not report the dever-runtime library artifact".into()),
            _ => return Err("Cargo reported multiple dever-runtime library artifacts".into()),
        },
        _ => return Err("Cargo reported multiple hashed dever-runtime library artifacts".into()),
    };
    dependencies.remove(&runtime);
    Ok(CargoRuntimeArtifacts {
        runtime,
        dependencies: dependencies.into_iter().collect(),
    })
}
