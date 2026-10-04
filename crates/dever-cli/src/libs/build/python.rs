//! PEP 517 source candidates enter the normal dependency solver as real wheels.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::{Product, PythonSystem, Receipt, Session, pack, process::Stage};
use crate::libs::{
    Ecosystem, LibSpec, LockFile, LockedArtifact, RegistryResolver, RegistryRuntime, archive_files,
    sha256,
};
use crate::workers::{python_wheel::Wheel, runtime};
use serde_json::{Value, json};

struct Environment {
    files: BTreeMap<String, (Vec<u8>, bool)>,
    assets: Vec<(String, Vec<u8>)>,
    executable: String,
    version: String,
    extension_suffixes: Vec<String>,
    descriptor: pack::Descriptor,
}

struct Active<'a> {
    session: &'a Session<'a>,
    key: String,
}
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.session.active.borrow_mut().remove(&self.key);
    }
}

impl Session<'_> {
    pub(crate) fn python(
        &self,
        resolver: &RegistryResolver<'_>,
        spec: &LibSpec,
        runtime: &RegistryRuntime,
        filename: &str,
        source: &[u8],
    ) -> Result<Product, String> {
        let key = format!(
            "{}@{}:{}:{}",
            spec.distribution_name(),
            spec.version,
            sha256(source),
            runtime.pack.sha256
        );
        if let Some(product) = self.products.borrow().get(&key) {
            return Ok(product.clone());
        }
        if self.active.borrow().len() >= 16 || !self.active.borrow_mut().insert(key.clone()) {
            return Err(format!(
                "cyclic or overly deep Python build requirement for {}",
                spec.key()
            ));
        }
        let _active = Active {
            session: self,
            key: key.clone(),
        };
        let sources = source_files(filename, source)?;
        let environment = Environment::new(self, runtime)?;
        let empty = LockFile::new(vec![])?;
        let inspected =
            environment.run(resolver, &empty, &sources, json!({"phase":"inspect"}), None)?;
        let system: PythonSystem = serde_json::from_value(inspected.response)
            .map_err(|error| format!("invalid Python build configuration: {error}"))?;
        let mut requirements = system.requires.clone();
        let mut dependencies = resolver.build_requirements(&requirements, runtime)?;
        let mut stable = None;
        let mut discovered = BTreeSet::new();
        for _ in 0..8 {
            let result = environment.run(
                resolver,
                &dependencies,
                &sources,
                json!({"phase":"requires","system":system}),
                None,
            )?;
            let dynamic: Vec<String> = serde_json::from_value(
                result
                    .response
                    .get("requires")
                    .cloned()
                    .ok_or("build backend omitted requirements")?,
            )
            .map_err(|error| format!("invalid dynamic Python build requirements: {error}"))?;
            discovered.extend(dynamic);
            requirements = system.requires.iter().chain(&discovered).cloned().collect();
            let next = resolver.build_requirements(&requirements, runtime)?;
            if next == dependencies {
                stable = Some(discovered.iter().cloned().collect());
                break;
            }
            dependencies = next;
        }
        let dynamic = stable.ok_or("Python dynamic build requirements did not converge")?;
        let metadata = environment.run(
            resolver,
            &dependencies,
            &sources,
            json!({"phase":"metadata","system":system}),
            None,
        )?;
        let result = environment.run(
            resolver,
            &dependencies,
            &sources,
            json!({"phase":"wheel","system":system,"metadata":metadata.response.get("metadata")}),
            Some(&metadata.outputs),
        )?;
        let filename = result
            .response
            .get("filename")
            .and_then(Value::as_str)
            .ok_or("build backend omitted wheel filename")?;
        crate::workers::valid_path(filename)?;
        if filename.contains('/') || !filename.ends_with(".whl") {
            return Err("build backend returned invalid wheel filename".into());
        }
        let bytes = result
            .outputs
            .get(&format!("wheels/{filename}"))
            .ok_or("build backend did not produce its declared wheel")?
            .clone();
        Wheel::parse(spec, Some(filename), &runtime.python_wheel_tags, &bytes)?;
        if let Some(prefix) = metadata.response.get("metadata").and_then(Value::as_str) {
            compare_metadata(prefix, &metadata.outputs, &bytes)?;
        }
        let output = artifact(
            runtime,
            format!(
                "lib/pip/{}/{}/archive.whl",
                sha256(spec.distribution_name().as_bytes()),
                spec.version
            ),
            &bytes,
        );
        let source_artifact = artifact(
            runtime,
            format!("build/source/{}/source", sha256(source)),
            source,
        );
        for (artifact, bytes) in [(&output, bytes.as_slice()), (&source_artifact, source)] {
            if resolver.store.publish(bytes, &runtime.target)? != artifact.sha256 {
                return Err("build artifact cache identity mismatch".into());
            }
        }
        let base: LibSpec = format!("pip:{}@{}", spec.distribution_name(), spec.version).parse()?;
        let receipt = Receipt {
            ecosystem: Ecosystem::Pip,
            roots: vec![base],
            sources: vec![source_artifact],
            graph: None,
            runtime: runtime.pack.clone(),
            auxiliary_runtime: None,
            native_entries: BTreeSet::new(),
            native_rejections: BTreeMap::new(),
            python_markers: runtime.python_markers.clone(),
            tools: environment.descriptor,
            frontend: super::frontend(&Ecosystem::Pip)?,
            python: Some(system),
            dynamic_requires: dynamic,
            dependencies: Box::new(dependencies),
            config_settings: BTreeMap::new(),
            output,
        };
        let product = Product {
            bytes,
            filename: filename.into(),
            receipt,
        };
        self.remember(key, product.clone())?;
        Ok(product)
    }
}

fn artifact(runtime: &RegistryRuntime, path: String, bytes: &[u8]) -> LockedArtifact {
    LockedArtifact {
        target: runtime.target.clone(),
        path,
        bytes: bytes.len() as u64,
        sha256: sha256(bytes),
    }
}

struct Source {
    files: Vec<(String, Vec<u8>)>,
    executables: BTreeSet<String>,
}

fn source_files(filename: &str, bytes: &[u8]) -> Result<Source, String> {
    let extension = if filename.ends_with(".tar.gz") {
        "tgz"
    } else if filename.ends_with(".zip") {
        "zip"
    } else {
        return Err("unsupported Python source archive format".into());
    };
    let files = archive_files(extension, bytes)?;
    let executables = crate::libs::registry::archive_executables(extension, bytes)?;
    let root = files
        .first()
        .and_then(|(path, _)| path.split_once('/'))
        .map(|(root, _)| format!("{root}/"))
        .ok_or("Python source archive needs one root directory")?;
    let files = files
        .into_iter()
        .map(|(path, bytes)| {
            let path = path
                .strip_prefix(&root)
                .ok_or("Python source archive contains multiple roots")?
                .to_owned();
            crate::workers::valid_path(&path)?;
            Ok((path, bytes))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let executables = executables
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .map(str::to_owned)
                .ok_or("source executable escapes archive root".into())
        })
        .collect::<Result<_, String>>()?;
    Ok(Source { files, executables })
}

struct ResultFiles {
    response: Value,
    outputs: BTreeMap<String, Vec<u8>>,
}

impl Environment {
    fn new(session: &Session<'_>, requested: &RegistryRuntime) -> Result<Self, String> {
        super::require_execution_target(&requested.target, "Python source build")?;
        let (runtime, bytes) = session.inputs.runtime(Ecosystem::Pip)?;
        if runtime.pack != requested.pack || sha256(&bytes) != requested.pack.sha256 {
            return Err("build Python runtime differs from resolver identity".into());
        }
        let runtime_files = archive_files("tgz", &bytes)?;
        let manifest = runtime::validate(&runtime_files, &Ecosystem::Pip, &requested.target)?;
        if manifest.python_wheel_tags != requested.python_wheel_tags
            || manifest.python_markers != requested.python_markers
        {
            return Err("build runtime policy differs from signed descriptor".into());
        }
        let executable = manifest
            .executable
            .clone()
            .ok_or("build Python executable missing")?;
        let executable = format!("runtime/{executable}");
        let mut files = runtime_files
            .into_iter()
            .map(|(path, bytes)| {
                let executable = manifest.executable(&path);
                (format!("runtime/{path}"), (bytes, executable))
            })
            .collect::<BTreeMap<_, _>>();
        let (descriptor, bytes) = session.inputs.tools(Ecosystem::Pip)?;
        descriptor.validate(requested, None)?;
        let (tools, tool_files) = pack::unpack(&bytes, &descriptor)?;
        let assets = session.inputs.assets()?;
        dever_sandbox::validate_assets(&assets)?;
        pack::validate_tools(&tools, &tool_files, &assets)?;
        for (path, bytes) in tool_files {
            let executable = tools.executables.contains(&path);
            if files.insert(path, (bytes, executable)).is_some() {
                return Err("build tools overlap runtime inputs".into());
            }
        }
        let version = requested
            .python_markers
            .as_ref()
            .ok_or("build Python markers absent")?
            .python_version()
            .to_string();
        Ok(Self {
            files,
            assets,
            executable,
            version,
            extension_suffixes: manifest.python_extension_suffixes,
            descriptor,
        })
    }

    fn run(
        &self,
        resolver: &RegistryResolver<'_>,
        dependencies: &LockFile,
        source: &Source,
        mut request: Value,
        prepared: Option<&BTreeMap<String, Vec<u8>>>,
    ) -> Result<ResultFiles, String> {
        let stage = Stage::new()?;
        let input = stage.0.join("input");
        for (path, (bytes, executable)) in &self.files {
            stage.write(&format!("input/{path}"), bytes, *executable)?;
        }
        for (path, bytes) in &self.assets {
            stage.write(
                &format!("sandbox/{path}"),
                bytes,
                dever_sandbox::asset_executable(path),
            )?;
        }
        for (path, bytes) in &source.files {
            stage.write(
                &format!("input/source/{path}"),
                bytes,
                source.executables.contains(path),
            )?;
        }
        if let Some(prepared) = prepared {
            for (path, bytes) in prepared {
                stage.write(&format!("input/prepared/{path}"), bytes, false)?;
            }
        }
        let mut installed = BTreeMap::<String, Vec<u8>>::new();
        let runtime = resolver
            .runtimes
            .get(&Ecosystem::Pip)
            .ok_or("build dependency runtime is missing")?;
        let mut distributions = BTreeSet::new();
        for lib in &dependencies.libs {
            super::validate_python_policy(
                dependencies,
                lib,
                runtime
                    .python_markers
                    .as_ref()
                    .ok_or("Python build markers absent")?,
            )?;
            if lib.runtime != runtime.pack || lib.artifacts.len() != 1 {
                return Err(
                    "build dependency does not use the exact Python runtime and wheel".into(),
                );
            }
            let artifact = &lib.artifacts[0];
            let bytes =
                resolver
                    .store
                    .verify_exact(&artifact.sha256, artifact.bytes, &artifact.target)?;
            let wheel = Wheel::parse(&lib.spec, None, &runtime.python_wheel_tags, &bytes)?;
            wheel.validate_dependencies(
                lib,
                runtime
                    .python_markers
                    .as_ref()
                    .ok_or("Python build markers absent")?,
            )?;
            if !distributions.insert((
                lib.spec.distribution_name().to_owned(),
                lib.spec.version.clone(),
            )) {
                continue;
            }
            for file in wheel.install(&self.executable, &self.version)? {
                if self.files.contains_key(&file.path)
                    || installed
                        .insert(file.path.clone(), file.bytes.clone())
                        .is_some()
                {
                    return Err("build dependency installation has conflicting file owners".into());
                }
                stage.write(
                    &format!("input/{}", file.path),
                    &file.bytes,
                    file.executable,
                )?;
            }
        }
        let mut supplied = self
            .files
            .iter()
            .map(|(path, (bytes, _))| (path.clone(), bytes.as_slice()))
            .collect::<BTreeMap<_, _>>();
        for (path, bytes) in &self.assets {
            supplied.insert(format!("sandbox/{path}"), bytes);
        }
        for (path, bytes) in &installed {
            supplied.insert(path.clone(), bytes);
        }
        crate::workers::python_wheel::validate_native(
            &runtime.target,
            &self.extension_suffixes,
            &self.executable,
            &supplied,
            &installed.keys().cloned().collect(),
        )?;
        request["python_version"] = json!(self.version);
        request["config_settings"] = json!({});
        stage.write(
            "input/request.json",
            &serde_json::to_vec(&request).map_err(|error| error.to_string())?,
            false,
        )?;
        stage.write("input/frontend.py", include_bytes!("python.py"), false)?;
        let work = stage.0.join("work");
        fs::create_dir(&work).map_err(|error| error.to_string())?;
        let grants = [dever_sandbox::Grant {
            source: work.clone(),
            destination: "/data/build".into(),
            writable: true,
        }];
        let arguments = [
            OsString::from("-I"),
            OsString::from("-B"),
            input.join("frontend.py").into_os_string(),
        ];
        let executable = input.join(&self.executable);
        let assets = stage.0.join("sandbox");
        let launch = dever_sandbox::Launch {
            assets: &assets,
            worker: &input,
            executable: &executable,
            arguments: &arguments,
            working_directory: &input,
            capabilities: dever_sandbox::Capabilities {
                network: false,
                process: true,
            },
            grants: &grants,
        };
        stage.run(
            launch.command_for_build(&input.join("rootfs"))?,
            "isolated Python build hook",
            &work,
        )?;
        if let Some(session) = resolver.build {
            session.inputs.diagnostic(&stage.diagnostics()?);
        }
        let outputs = output_files(&work.join("output"))?;
        let response = serde_json::from_slice(
            outputs
                .get("result.json")
                .ok_or("build hook produced no result")?,
        )
        .map_err(|error| format!("invalid build hook response: {error}"))?;
        Ok(ResultFiles { response, outputs })
    }
}

pub(super) fn output_files(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    let mut remaining = crate::workers::MAX_TREE_BYTES;
    let mut entries = 0usize;
    #[cfg(unix)]
    let mut links = BTreeMap::<(u64, u64), (u64, u64)>::new();
    while let Some(directory) = pending.pop() {
        if fs::symlink_metadata(&directory)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("build output directory is a symbolic link".into());
        }
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            entries += 1;
            if entries > crate::workers::MAX_TREE_FILES {
                return Err("build output exceeds file budget".into());
            }
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !kind.is_file() {
                return Err("build output contains a link or special file".into());
            }
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
            }
            let file = options
                .open(entry.path())
                .map_err(|error| error.to_string())?;
            let metadata = file.metadata().map_err(|error| error.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let aliases = links
                    .entry((metadata.dev(), metadata.ino()))
                    .or_insert((metadata.nlink(), 0));
                if aliases.0 != metadata.nlink() {
                    return Err("build output link count changed during collection".into());
                }
                aliases.1 += 1;
            }
            if !metadata.is_file() || metadata.len() > remaining as u64 {
                return Err("build output exceeds byte budget".into());
            }
            let mut bytes = Vec::new();
            file.take(remaining as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            remaining = remaining
                .checked_sub(bytes.len())
                .ok_or("build output exceeds byte budget")?;
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_str()
                .ok_or("build output path is not UTF-8")?
                .to_owned();
            crate::workers::valid_path(&name)?;
            files.insert(name, bytes);
        }
    }
    // node-gyp's COPY rule may hard-link two output names. Namespace teardown
    // precedes this read: accept aliases only when every link is accounted for
    // inside the output root, then publish independent immutable byte entries.
    #[cfg(unix)]
    if links
        .values()
        .any(|(expected, observed)| expected != observed)
    {
        return Err("build output has a hard-link alias outside its output root".into());
    }
    Ok(files)
}

fn compare_metadata(
    prefix: &str,
    outputs: &BTreeMap<String, Vec<u8>>,
    wheel: &[u8],
) -> Result<(), String> {
    crate::workers::valid_path(prefix)?;
    let distribution = PathBuf::from(prefix)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid prepared metadata directory")?
        .to_owned();
    let wheel = archive_files("whl", wheel)?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let prefix = format!("{prefix}/");
    let mut count = 0;
    for (path, bytes) in outputs {
        if let Some(relative) = path.strip_prefix(&prefix) {
            count += 1;
            if wheel.get(&format!("{distribution}/{relative}")) != Some(bytes) {
                return Err(
                    "built wheel metadata differs from prepare_metadata_for_build_wheel".into(),
                );
            }
        }
    }
    if count == 0 {
        return Err("metadata hook produced an empty directory".into());
    }
    Ok(())
}
