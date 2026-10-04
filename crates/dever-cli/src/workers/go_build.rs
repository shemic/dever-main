//! Compile a checked Go Adapter with the exact tools and archives in its locked pack.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use dever_core::hir::ExternalWorkerContract;
use dever_core::native::EmbeddedResource;
use serde::Deserialize;
use serde_json::json;

use super::{Tree, dependency_closure, digest, entry_name, locked_archive, valid_path};
use crate::libs::build::process::Stage;
use crate::libs::{self, LockFile};

const SDK: &[u8] = include_bytes!("../../../../sdk/go/component.go");

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GoBuildManifest {
    host: crate::toolchain::BuildTarget,
    compiler: String,
    linker: String,
    analyzer: String,
    stdlib_importcfg: String,
    goos: String,
    goarch: String,
    go_version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedPackage {
    name: String,
    files: Vec<String>,
    imports: Vec<String>,
    embed: BTreeMap<String, Vec<String>>,
}

struct Builder {
    stage: Stage,
    input: PathBuf,
    assets: PathBuf,
    grants: Vec<dever_sandbox::Grant>,
    tools: GoBuildManifest,
    entry: String,
    stdlib: BTreeMap<String, PathBuf>,
    modules: BTreeMap<String, PathBuf>,
    archives: BTreeMap<String, PathBuf>,
    visiting: BTreeSet<String>,
}

pub(super) struct Inputs<'a> {
    pub files: &'a [(String, Vec<u8>)],
    pub manifest: &'a GoBuildManifest,
    pub sandbox: &'a [EmbeddedResource],
}

pub(super) fn compile_worker(
    contract: &ExternalWorkerContract,
    lock: &LockFile,
    supplied: &BTreeMap<&str, &EmbeddedResource>,
    inputs: Inputs<'_>,
    tree: &mut Tree,
    target: &str,
) -> Result<String, String> {
    let Inputs {
        files: pack_files,
        manifest: build,
        sandbox: build_assets,
    } = inputs;
    let references = build.validate_files(pack_files, target)?;
    if build.host != crate::toolchain::BuildTarget::host()? {
        return Err("Go build tools require a different build host".into());
    }
    let stage = Stage::new()?;
    let input = stage.0.join("input");
    let assets = stage.0.join("sandbox");
    for resource in build_assets
        .iter()
        .filter(|resource| resource.path.starts_with("sandbox/"))
    {
        if digest(&resource.bytes) != resource.sha256 {
            return Err("Go build sandbox asset has a digest mismatch".into());
        }
        stage.write(&resource.path, &resource.bytes, resource.executable)?;
    }
    let build_directory = stage.0.join("build");
    fs::create_dir(&build_directory)
        .map_err(|error| format!("cannot create Go build output: {error}"))?;
    let grants = vec![dever_sandbox::Grant {
        source: build_directory,
        destination: "/data/build".into(),
        writable: true,
    }];
    for (path, bytes) in pack_files {
        stage.write(
            &format!("input/{path}"),
            bytes,
            path == &build.compiler || path == &build.linker || path == &build.analyzer,
        )?;
    }
    let stdlib = references
        .into_iter()
        .map(|(name, path)| (name, input.join(path)))
        .collect();
    let source_prefix = format!("{}/source/", tree.prefix);
    let mut entry_present = false;
    for (path, resource) in &tree.files {
        if let Some(relative) = path.strip_prefix(&source_prefix) {
            if relative == entry_name(&contract.entry)? {
                entry_present = true;
            }
            stage.write(&format!("input/source/{relative}"), &resource.bytes, false)?;
        }
    }
    if !entry_present {
        return Err("managed Go Worker entry source is missing".into());
    }
    if tree
        .files
        .contains_key(&format!("{}/source/dever_worker_runner.go", tree.prefix))
    {
        return Err("Adapter source conflicts with the generated Go Worker runner".into());
    }
    stage.write("input/sdk/component.go", SDK, false)?;
    let runner = runner_source(contract)?;
    stage.write(
        "input/source/dever_worker_runner.go",
        runner.as_bytes(),
        false,
    )?;
    let mut modules = BTreeMap::new();
    let authenticated =
        libs::sumdb::verify_chain(&lock.go_sumdb, &libs::sumdb::Verifier::official())?;
    for lib in dependency_closure(contract, lock)? {
        let root = format!("{}@v{}/", lib.spec.name, lib.spec.version);
        let directory = format!("modules/{}", digest(lib.spec.key().as_bytes()));
        let archive = locked_archive(lib, "zip", supplied, target)?;
        authenticated.verify_zip(&lib.spec.name, &lib.spec.version, archive)?;
        let files = libs::archive_files("zip", archive)?;
        if files.is_empty() {
            return Err(format!("locked Go module {} is empty", lib.spec.key()));
        }
        for (path, bytes) in files {
            let relative = path.strip_prefix(&root).ok_or_else(|| {
                format!(
                    "locked Go module {} has a mismatched archive root",
                    lib.spec.key()
                )
            })?;
            stage.write(&format!("input/{directory}/{relative}"), &bytes, false)?;
        }
        if modules
            .insert(lib.spec.name.clone(), input.join(directory))
            .is_some()
        {
            return Err("locked Go module graph selects multiple versions of one module".into());
        }
    }
    let mut builder = Builder {
        stage,
        input,
        assets,
        grants,
        tools: build.clone(),
        entry: entry_name(&contract.entry)?.into(),
        stdlib,
        modules,
        archives: BTreeMap::new(),
        visiting: BTreeSet::new(),
    };
    builder.verify_tools()?;
    builder.compile("dever-component")?;
    builder.compile("main")?;
    let binary_name = format!("worker{}", std::env::consts::EXE_SUFFIX);
    builder.link(&binary_name)?;
    let path = builder.stage.0.join("build").join(&binary_name);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("cannot open linked Go Worker: {error}"))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
        return Err("linked Go Worker is not a bounded regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err("linked Go Worker must not have hard links".into());
        }
    }
    let mut binary = Vec::new();
    file.take(128 * 1024 * 1024 + 1)
        .read_to_end(&mut binary)
        .map_err(|error| format!("cannot read linked Go Worker: {error}"))?;
    if binary.len() > 128 * 1024 * 1024 {
        return Err("linked Go Worker exceeds the byte limit".into());
    }
    if binary.is_empty() {
        return Err("linked Go Worker is empty".into());
    }
    super::runtime::validate_binary("Go Worker", &binary, target)?;
    tree.add(&binary_name, binary, true)?;
    Ok(binary_name)
}

impl GoBuildManifest {
    pub(super) fn executable(&self, path: &str) -> bool {
        [&self.compiler, &self.linker, &self.analyzer]
            .iter()
            .any(|tool| tool.as_str() == path)
    }

    pub(super) fn validate_files(
        &self,
        files: &[(String, Vec<u8>)],
        target: &str,
    ) -> Result<BTreeMap<String, String>, String> {
        self.validate(target)?;
        let mut pack = BTreeMap::new();
        for (path, bytes) in files {
            if pack.insert(path.as_str(), bytes.as_slice()).is_some() {
                return Err("Go build pack repeats a file".into());
            }
        }
        for tool in [&self.compiler, &self.linker, &self.analyzer] {
            if !tool.starts_with("bin/")
                || !pack
                    .get(tool.as_str())
                    .is_some_and(|bytes| !bytes.is_empty())
            {
                return Err(format!("locked Go build tool '{tool}' is missing"));
            }
            super::runtime::validate_binary(tool, pack[tool.as_str()], self.host.platform())?;
        }
        if self.stdlib_importcfg != "stdlib/importcfg" {
            return Err("locked Go standard library importcfg path is invalid".into());
        }
        let importcfg = pack
            .get(self.stdlib_importcfg.as_str())
            .ok_or("locked Go standard library importcfg is missing")?;
        let importcfg = std::str::from_utf8(importcfg)
            .map_err(|_| "locked Go standard library importcfg is not UTF-8")?;
        let mut stdlib = BTreeMap::new();
        for line in importcfg.lines() {
            let (name, path) = line
                .strip_prefix("packagefile ")
                .and_then(|line| line.split_once('='))
                .ok_or("invalid locked Go standard library importcfg")?;
            valid_import(name)?;
            if path != format!("stdlib/{name}.a")
                || !pack.get(path).is_some_and(|bytes| !bytes.is_empty())
                || stdlib.insert(name.into(), path.into()).is_some()
            {
                return Err(
                    "locked Go standard library importcfg has a missing or repeated archive".into(),
                );
            }
            let archive = pack[path];
            let version = self
                .go_version
                .split_whitespace()
                .nth(2)
                .ok_or("invalid Go version")?;
            let object_identity = format!("go object {} {} {version} ", self.goos, self.goarch);
            if !archive.starts_with(b"!<arch>\n__.PKGDEF")
                || !archive
                    .get(68..)
                    .is_some_and(|bytes| bytes.starts_with(object_identity.as_bytes()))
            {
                return Err(format!(
                    "locked Go standard library archive '{name}' differs from its target or compiler version"
                ));
            }
        }
        if !["runtime", "context", "encoding/json"]
            .iter()
            .all(|name| stdlib.contains_key(*name))
        {
            return Err("locked Go standard library importcfg is incomplete".into());
        }
        Ok(stdlib)
    }

    fn validate(&self, target: &str) -> Result<(), String> {
        let expected = match target {
            "linux-x86_64" => ("linux", "amd64"),
            "linux-aarch64" => ("linux", "arm64"),
            _ => return Err("managed Go build target is unsupported".into()),
        };
        if (self.goos.as_str(), self.goarch.as_str()) != expected
            || !self.go_version.starts_with("go version go")
            || !self.go_version.ends_with(match self.host {
                crate::toolchain::BuildTarget::LinuxX86_64 => " linux/amd64",
                crate::toolchain::BuildTarget::LinuxAarch64 => " linux/arm64",
            })
        {
            return Err("managed Go build tools do not match the selected target".into());
        }
        for path in [
            &self.compiler,
            &self.linker,
            &self.analyzer,
            &self.stdlib_importcfg,
        ] {
            valid_path(path)?;
        }
        Ok(())
    }
}

impl Builder {
    fn verify_tools(&self) -> Result<(), String> {
        let version = self
            .tools
            .go_version
            .split_whitespace()
            .nth(2)
            .ok_or("locked Go build version is invalid")?;
        for (kind, path) in [
            ("compile", &self.tools.compiler),
            ("link", &self.tools.linker),
        ] {
            let output = self.run(&self.input.join(path), &["-V"])?;
            if output != format!("{kind} version {version}\n").as_bytes() {
                return Err(format!(
                    "locked Go {kind} tool version differs from its pack"
                ));
            }
        }
        Ok(())
    }

    fn package_dir(&self, name: &str) -> Result<PathBuf, String> {
        if name == "main" {
            return Ok(self.input.join("source"));
        }
        if name == "dever-component" {
            return Ok(self.input.join("sdk"));
        }
        if let Some(relative) = name.strip_prefix("dever-worker/") {
            valid_path(relative)?;
            return Ok(self.input.join("source").join(relative));
        }
        let (module, root) = self
            .modules
            .iter()
            .filter(|(module, _)| {
                name == module.as_str() || name.starts_with(&format!("{module}/"))
            })
            .max_by_key(|(module, _)| module.len())
            .ok_or_else(|| format!("Go import '{name}' is not in the locked module graph"))?;
        let suffix = name
            .strip_prefix(module)
            .expect("selected module prefix")
            .trim_start_matches('/');
        if !suffix.is_empty() {
            valid_path(suffix)?;
        }
        Ok(root.join(suffix))
    }

    fn compile(&mut self, name: &str) -> Result<(), String> {
        if self.stdlib.contains_key(name) || name == "unsafe" || self.archives.contains_key(name) {
            return Ok(());
        }
        valid_import(name)?;
        if !self.visiting.insert(name.to_owned()) {
            return Err(format!("Go import cycle includes '{name}'"));
        }
        let directory = self.package_dir(name)?;
        let analyzer = self.input.join(&self.tools.analyzer);
        let output = self.run(
            &analyzer,
            &[
                self.tools.goos.as_str(),
                self.tools.goarch.as_str(),
                directory.to_str().ok_or("Go package path is not UTF-8")?,
            ],
        )?;
        let selected: SelectedPackage = serde_json::from_slice(&output)
            .map_err(|error| format!("invalid locked Go analyzer output: {error}"))?;
        if selected.files.is_empty() || (name == "main") != (selected.name == "main") {
            return Err(format!(
                "Go package '{name}' has an invalid package declaration"
            ));
        }
        if name == "main" && !selected.files.contains(&self.entry) {
            // The generated runner may be selected while the checked entry is excluded by tags.
            return Err("checked Go Worker entry is excluded by target build constraints".into());
        }
        for import in &selected.imports {
            self.compile(import)?;
        }
        let cfg = self.importcfg()?;
        let id = digest(name.as_bytes());
        let cfg_path = self.stage.write(
            &format!("input/config/{id}.importcfg"),
            cfg.as_bytes(),
            false,
        )?;
        let archive = self.stage.0.join(format!("build/{id}.a"));
        let mut args = vec![
            "-pack".to_owned(),
            "-complete".into(),
            "-nolocalimports".into(),
            // Staging is invocation-owned; recorded source paths must not make
            // identical locked Workers differ across run/build or projects.
            "-trimpath".into(),
            self.input.to_string_lossy().into_owned(),
            "-p".into(),
            name.into(),
            "-importcfg".into(),
            cfg_path.to_string_lossy().into_owned(),
            "-o".into(),
            archive.to_string_lossy().into_owned(),
        ];
        if !selected.embed.is_empty() {
            let mut files = BTreeMap::new();
            for paths in selected.embed.values() {
                for path in paths {
                    valid_path(path)?;
                    let file = directory.join(path);
                    if !file.is_file() {
                        return Err("Go embed file is absent from locked source".into());
                    }
                    files.insert(path.clone(), self.visible_path(&file)?);
                }
            }
            let embed = json!({"Patterns":selected.embed,"Files":files});
            let path = self.stage.write(
                &format!("input/config/{id}.embedcfg"),
                &serde_json::to_vec(&embed).map_err(|error| error.to_string())?,
                false,
            )?;
            args.extend(["-embedcfg".into(), path.to_string_lossy().into_owned()]);
        }
        for file in selected.files {
            valid_path(&file)?;
            args.push(directory.join(file).to_string_lossy().into_owned());
        }
        let compiler = self.input.join(&self.tools.compiler);
        let arguments = args.iter().map(String::as_str).collect::<Vec<_>>();
        self.run(&compiler, &arguments)?;
        self.archives.insert(name.into(), archive);
        self.visiting.remove(name);
        Ok(())
    }

    fn importcfg(&self) -> Result<String, String> {
        let mut cfg = String::new();
        for (name, path) in self.stdlib.iter().chain(&self.archives) {
            cfg.push_str(&format!(
                "packagefile {name}={}\n",
                self.visible_path(path)?
            ));
        }
        Ok(cfg)
    }

    fn link(&self, output: &str) -> Result<(), String> {
        let cfg = self.stage.write(
            "input/config/link.importcfg",
            self.importcfg()?.as_bytes(),
            false,
        )?;
        let main = self
            .archives
            .get("main")
            .ok_or("Go Worker main archive is missing")?;
        let linker = self.input.join(&self.tools.linker);
        let output_path = self.stage.0.join("build").join(output);
        let temporary_directory = self.stage.0.join("build");
        self.run(
            &linker,
            &[
                "-linkmode=internal",
                "-buildmode=exe",
                "-tmpdir",
                temporary_directory
                    .to_str()
                    .ok_or("Go build path is not UTF-8")?,
                "-importcfg",
                cfg.to_str().ok_or("Go importcfg path is not UTF-8")?,
                "-o",
                output_path.to_str().ok_or("Go output path is not UTF-8")?,
                main.to_str().ok_or("Go archive path is not UTF-8")?,
            ],
        )?;
        Ok(())
    }

    fn run(&self, executable: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
        let arguments = args.iter().map(OsString::from).collect::<Vec<_>>();
        self.stage.run(
            self.sandbox(executable, &arguments).command()?,
            "locked Go build tool",
            &self.stage.0.join("build"),
        )
    }

    fn sandbox<'a>(
        &'a self,
        executable: &'a Path,
        arguments: &'a [OsString],
    ) -> dever_sandbox::Launch<'a> {
        dever_sandbox::Launch {
            assets: &self.assets,
            worker: &self.input,
            executable,
            arguments,
            working_directory: &self.input,
            capabilities: dever_sandbox::Capabilities::default(),
            grants: &self.grants,
        }
    }

    fn visible_path(&self, path: &Path) -> Result<String, String> {
        self.sandbox(&self.input, &[])
            .argument_path(path)?
            .into_os_string()
            .into_string()
            .map_err(|_| "Go sandbox path is not UTF-8".into())
    }
}

fn valid_import(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.split('/').any(str::is_empty)
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'~')
        })
    {
        return Err(format!("invalid locked Go import '{name}'"));
    }
    Ok(())
}

fn runner_source(contract: &ExternalWorkerContract) -> Result<String, String> {
    let mut handlers = String::new();
    let mut names = BTreeSet::new();
    for operation in &contract.operations {
        let mut function = String::new();
        for part in operation.split(['.', '_', '-']) {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
                return Err(format!(
                    "Go operation '{operation}' cannot bind to an exported function"
                ));
            }
            let mut letters = part.chars();
            function.push(letters.next().expect("nonempty part").to_ascii_uppercase());
            function.extend(letters);
        }
        if !names.insert(function.clone())
            || !function.starts_with(|character: char| character.is_ascii_uppercase())
        {
            return Err("Go operation names map to duplicate or invalid exported functions".into());
        }
        handlers.push_str(&format!("\t\t{operation:?}: {function},\n"));
    }
    Ok(format!(
        "package main\n\nimport (\n\t\"context\"\n\t\"fmt\"\n\t\"os\"\n\tcomponent \"dever-component\"\n)\n\nfunc main() {{\n\tmanifest, err := os.ReadFile(\"contract.json\")\n\tif err == nil {{\n\t\terr = component.ServeManifest(manifest, map[string]func(context.Context, any, any) (any, error){{\n{handlers}\t\t}}, os.Stdin, os.Stdout)\n\t}}\n\tif err != nil {{\n\t\tfmt.Fprintln(os.Stderr, err)\n\t\tos.Exit(1)\n\t}}\n}}\n"
    ))
}
