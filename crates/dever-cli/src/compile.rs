//! The default compiler pipeline: checked HIR -> LLVM object -> verified LLD
//! inputs. Artifact lifetime, cache integrity and output publication stay in core.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use dever_backend_bridge::{BinaryResource, LinkKind, Target, emit_object_with_resources, link};
use dever_core::hir::Program;
use dever_core::llvm::ResourceModule;
use dever_core::native::{EmbeddedResource, NativeProgram};
use dever_core::source::SourceMap;
use dever_runtime::config::{CompilationBindings, RuntimeProfile};
use sha2::{Digest, Sha256};

use dever_cli::toolchain::compilation::{CompileKind, CompileRequest};
use dever_cli::toolchain::runtime_pack::RuntimePack;
use dever_cli::toolchain::{BuildTarget, Layout, Version};

pub fn application(
    program: &Program,
    sources: &SourceMap,
    bindings: &CompilationBindings,
    resources: &[EmbeddedResource],
    target: BuildTarget,
) -> Result<NativeProgram, String> {
    if let Some((layout, version)) = managed_compiler()? {
        let request = CompileRequest::new(
            version,
            CompileKind::Application,
            sources,
            Some(bindings.clone()),
            resources,
            target,
        )?;
        return NativeProgram::from_verified_bytes(&dever_cli::toolchain::compile(
            &layout, &request,
        )?);
    }
    let profile = bindings.profile();
    compile(profile, target, || {
        dever_core::llvm::emit_executable_with_resources(program, sources, resources)
    })
}

pub fn test_suite(program: &Program, sources: &SourceMap) -> Result<NativeProgram, String> {
    let target = BuildTarget::host()?;
    if let Some((layout, version)) = managed_compiler()? {
        let request =
            CompileRequest::new(version, CompileKind::TestSuite, sources, None, &[], target)?;
        return NativeProgram::from_verified_bytes(&dever_cli::toolchain::compile(
            &layout, &request,
        )?);
    }
    let profile = test_profile(program);
    compile(profile, target, || {
        dever_core::llvm::emit_test_executable(program, sources).map(|ir| ResourceModule {
            ir,
            resources: Vec::new(),
        })
    })
}

pub(super) fn compiler_directory() -> Result<PathBuf, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("cannot locate compiler: {error}"))?;
    executable
        .parent()
        .map(Path::to_owned)
        .ok_or_else(|| "compiler has no parent directory".into())
}

pub(super) fn managed_compiler() -> Result<Option<(Layout, String)>, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("cannot locate compiler: {error}"))?;
    let directory = executable
        .parent()
        .ok_or("compiler has no parent directory")?;
    let managed = executable.file_stem().and_then(|name| name.to_str()) == Some("dever-core")
        || directory
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "versions");
    if !managed {
        return Ok(None);
    }
    let versions = directory
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "versions"))
        .ok_or(
            "managed compiler must be installed under <toolchain>/versions/<version>/dever-core",
        )?;
    if executable.file_stem().and_then(|name| name.to_str()) != Some("dever-core") {
        return Err("managed compiler must use the installed dever-core executable".into());
    }
    let version = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid managed compiler version directory")?;
    Version::parse(version)?;
    if version != env!("CARGO_PKG_VERSION") {
        return Err("managed compiler version does not match its installation directory".into());
    }
    let root = versions
        .parent()
        .ok_or("managed compiler has no toolchain root")?;
    Ok(Some((Layout::new(root), version.to_owned())))
}

fn test_profile(program: &Program) -> RuntimeProfile {
    RuntimeProfile {
        sqlite: program.tests().iter().any(|test| test.uses_database()),
        postgres: false,
    }
}

pub(super) fn worker_compile(
    program: &Program,
    sources: &SourceMap,
    request: &CompileRequest,
    build: &Path,
    output: &Path,
) -> Result<(), String> {
    let resources = request.embedded_resources();
    let (profile, ir) = match request.kind {
        CompileKind::Application => {
            let profile = program.validate_database_bindings(
                request
                    .bindings
                    .as_ref()
                    .ok_or("application bindings are missing")?,
            )?;
            let ir =
                dever_core::llvm::emit_executable_with_resources(program, sources, &resources)?;
            (profile, ir)
        }
        CompileKind::TestSuite => (
            test_profile(program),
            ResourceModule {
                ir: dever_core::llvm::emit_test_executable(program, sources)?,
                resources: Vec::new(),
            },
        ),
    };
    let (layout, version) =
        managed_compiler()?.ok_or("compile worker requires a managed compiler")?;
    let resources =
        dever_cli::toolchain::SignedResources::load(&layout, &Version::parse(&version)?)?;
    let root = resources.root(&format!(
        "runtime/{}/manifest.json",
        request.target.platform()
    ))?;
    let pack = RuntimePack::load(&root, profile, request.target)?;
    link_ir(&ir, &pack, build, output)
}

fn compile<'a>(
    profile: RuntimeProfile,
    target: BuildTarget,
    emit: impl FnOnce() -> Result<ResourceModule<'a>, String>,
) -> Result<NativeProgram, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("cannot locate compiler: {error}"))?;
    let directory = compiler_directory()?;
    let pack = RuntimePack::load(&directory, profile, target)?;
    let ir = emit()?;
    let compiler = dever_cli::toolchain::sha256_file(&executable)?;
    let mut identity = Sha256::new();
    for part in [
        "dever-llvm-executable-v1",
        &compiler,
        &pack.identity,
        target.platform(),
        &ir.ir,
    ] {
        identity.update((part.len() as u64).to_le_bytes());
        identity.update(part.as_bytes());
    }
    dever_core::native::compile_artifact(
        &directory.join("cache/native"),
        &format!("{:x}", identity.finalize()),
        |build, output| link_ir(&ir, &pack, build, output),
    )
}

fn link_ir(
    module: &ResourceModule<'_>,
    pack: &RuntimePack,
    build: &Path,
    output: &Path,
) -> Result<(), String> {
    let target = Target::ALL
        .into_iter()
        .find(|target| target.triple() == pack.target.triple())
        .ok_or("native runtime target is not supported by the compiler bridge")?;
    let resources = module
        .resources
        .iter()
        .map(|resource| BinaryResource {
            symbol: &resource.symbol,
            bytes: resource.bytes,
        })
        .collect::<Vec<_>>();
    let object = emit_object_with_resources(&module.ir, target, &resources)?;
    let path = build.join("application.o");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .and_then(|mut file| file.write_all(&object))
        .map_err(|error| format!("cannot write application object: {error}"))?;
    let snapshot = pack.snapshot(build)?;
    let inputs = snapshot
        .start
        .into_iter()
        .chain(std::iter::once(path))
        .chain(snapshot.libraries)
        .chain(snapshot.end)
        .collect::<Vec<_>>();
    link(target, LinkKind::StaticExecutable, &inputs, output, None)
}
