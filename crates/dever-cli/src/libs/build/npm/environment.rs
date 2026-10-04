//! One immutable Node/Python/tool environment for registry lifecycle execution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use crate::libs::build::NativeRejection;
use crate::libs::build::{Inputs, pack, process::Stage};
use crate::libs::{Ecosystem, RegistryRuntime, RuntimePack, archive_files, npm, sha256};
use crate::workers::runtime;

pub(super) struct Environment {
    files: BTreeMap<String, (Vec<u8>, bool)>,
    assets: Vec<(String, Vec<u8>)>,
    node: String,
    node_paths: BTreeSet<String>,
    python: String,
    pub python_runtime: RuntimePack,
    pub descriptor: pack::Descriptor,
}

pub(super) struct Output {
    pub files: Vec<(String, Vec<u8>)>,
    pub executables: BTreeSet<String>,
    pub failed: BTreeSet<String>,
    pub native: BTreeSet<String>,
    pub rejections: BTreeMap<String, NativeRejection>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultMetadata {
    failed: BTreeSet<String>,
    native: BTreeSet<String>,
}

impl Environment {
    pub fn new(inputs: &dyn Inputs, runtime: &RegistryRuntime) -> Result<Self, String> {
        crate::libs::build::require_execution_target(&runtime.target, "npm lifecycle build")?;
        let (node, bytes) = inputs.runtime(Ecosystem::Npm)?;
        if node.pack != runtime.pack || sha256(&bytes) != runtime.pack.sha256 {
            return Err("npm build runtime differs from resolver".into());
        }
        let node_files = archive_files("tgz", &bytes)?;
        let node_manifest = runtime::validate(&node_files, &Ecosystem::Npm, &runtime.target)?;
        let node_paths = node_files
            .iter()
            .map(|(path, _)| format!("runtime/{path}"))
            .collect();
        let node = format!(
            "runtime/{}",
            node_manifest
                .executable
                .as_deref()
                .ok_or("Node executable missing")?
        );
        let (python, bytes) = inputs.runtime(Ecosystem::Pip)?;
        if sha256(&bytes) != python.pack.sha256 {
            return Err("auxiliary Python runtime hash mismatch".into());
        }
        let python_files = archive_files("tgz", &bytes)?;
        let python_manifest = runtime::validate(&python_files, &Ecosystem::Pip, &runtime.target)?;
        let python_executable = format!(
            "runtime/{}",
            python_manifest
                .executable
                .as_deref()
                .ok_or("build Python executable missing")?
        );
        let (descriptor, bytes) = inputs.tools(Ecosystem::Npm)?;
        descriptor.validate(runtime, Some(&python))?;
        let (manifest, tools) = pack::unpack(&bytes, &descriptor)?;
        let assets = inputs.assets()?;
        dever_sandbox::validate_assets(&assets)?;
        pack::validate_tools(&manifest, &tools, &assets)?;
        let mut files = BTreeMap::new();
        for (path, bytes) in node_files {
            let executable = node_manifest.executable(&path);
            files.insert(format!("runtime/{path}"), (bytes, executable));
        }
        for (path, bytes) in python_files {
            if path == "dever-runtime.json" {
                continue;
            }
            let executable = python_manifest.executable(&path);
            let path = format!("runtime/{path}");
            if let Some((existing, _)) = files.get(&path) {
                if existing != &bytes {
                    return Err("Node and auxiliary Python runtime paths conflict".into());
                }
            } else {
                files.insert(path, (bytes, executable));
            }
        }
        for (path, bytes) in tools {
            let executable = manifest.executables.contains(&path);
            if files.insert(path, (bytes, executable)).is_some() {
                return Err("npm tools overlap runtimes".into());
            }
        }
        Ok(Self {
            files,
            assets,
            node,
            node_paths,
            python: python_executable,
            python_runtime: python.pack,
            descriptor,
        })
    }

    pub fn run(
        &self,
        inputs: &dyn Inputs,
        graph: &npm::Environment,
        files: &[(String, Vec<u8>)],
        executables: &BTreeSet<String>,
    ) -> Result<Output, String> {
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
        for (path, bytes) in files {
            stage.write(
                &format!("input/install/{path}"),
                bytes,
                executables.contains(path),
            )?;
        }
        let request = serde_json::json!({
            "instances":graph.instances, "required":super::required_nodes(graph)?,
            "roots":graph.roots.iter().map(|root|format!("node_modules/{}",root.name)).collect::<Vec<_>>(),
            "python":format!("/worker/{}", self.python),
        });
        stage.write(
            "input/request.json",
            &serde_json::to_vec(&request).map_err(|error| error.to_string())?,
            false,
        )?;
        stage.write("input/frontend.js", include_bytes!("../npm.js"), false)?;
        let work = stage.0.join("work");
        fs::create_dir(&work).map_err(|error| error.to_string())?;
        let grants = [dever_sandbox::Grant {
            source: work.clone(),
            destination: "/data/build".into(),
            writable: true,
        }];
        let arguments = [input.join("frontend.js").into_os_string()];
        let executable = input.join(&self.node);
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
            "isolated npm lifecycle",
            &work,
        )?;
        inputs.diagnostic(&stage.diagnostics()?);
        let mut outputs = super::super::python::output_files(&work.join("output"))?;
        let response: ResultMetadata = serde_json::from_slice(
            &outputs
                .remove("result.json")
                .ok_or("npm build result missing")?,
        )
        .map_err(|error| error.to_string())?;
        let mut files = Vec::new();
        let mut executables = BTreeSet::new();
        for (path, bytes) in outputs {
            let path = path
                .strip_prefix("install/")
                .ok_or("npm lifecycle output is outside its installation")?
                .to_owned();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if fs::symlink_metadata(work.join("output/install").join(&path))
                    .map_err(|error| error.to_string())?
                    .permissions()
                    .mode()
                    & 0o111
                    != 0
                {
                    executables.insert(path.clone());
                }
            }
            files.push((path, bytes));
        }
        let mut output = Output {
            files,
            executables,
            failed: response.failed,
            native: response.native,
            rejections: BTreeMap::new(),
        };
        // Release the large compiler tree before staging the much smaller
        // runtime-only native admission sandbox.
        drop(stage);
        self.admit_native(inputs, &mut output)?;
        Ok(output)
    }

    fn admit_native(&self, inputs: &dyn Inputs, output: &mut Output) -> Result<(), String> {
        let required = std::mem::take(&mut output.native);
        let candidates = output
            .files
            .iter()
            .filter(|(path, _)| path.ends_with(".node"))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return if required.is_empty() {
                Ok(())
            } else {
                Err("npm generated native entry point is missing".into())
            };
        }
        let machine = match self.descriptor.target.as_str() {
            "linux-x86_64" => goblin::elf::header::EM_X86_64,
            "linux-aarch64" => goblin::elf::header::EM_AARCH64,
            _ => return Err("Node native admission target is unsupported".into()),
        };
        let mut files = self
            .files
            .iter()
            .filter(|(path, _)| self.node_paths.contains(*path))
            .map(|(path, (bytes, _))| (path.clone(), bytes.as_slice()))
            .collect::<BTreeMap<_, _>>();
        files.extend(
            self.assets
                .iter()
                .map(|(path, bytes)| (format!("sandbox/{path}"), bytes.as_slice())),
        );
        files.extend(
            output
                .files
                .iter()
                .map(|(path, bytes)| (path.clone(), bytes.as_slice())),
        );
        let stage = Stage::new()?;
        for (path, bytes) in &files {
            let relative = if path.starts_with("sandbox/") {
                path.clone()
            } else {
                format!("input/{path}")
            };
            let executable = path == &self.node
                || path
                    .strip_prefix("sandbox/")
                    .is_some_and(dever_sandbox::asset_executable);
            stage.write(&relative, bytes, executable)?;
        }
        stage.write("input/probe.cjs", include_bytes!("probe.cjs"), false)?;
        let input = stage.0.join("input");
        let executable = input.join(&self.node);
        let assets = stage.0.join("sandbox");
        for (path, bytes) in candidates {
            let rejection = match goblin::elf::Elf::parse(bytes) {
                Err(_) => Some(NativeRejection::Format),
                Ok(elf) if !elf.is_64 || !elf.little_endian || elf.header.e_machine != machine => {
                    Some(NativeRejection::Target)
                }
                Ok(_) => {
                    if let Err(error) = crate::workers::python_wheel::validate_native(
                        &self.descriptor.target,
                        &[],
                        &self.node,
                        &files,
                        &BTreeSet::from([path.clone()]),
                    ) {
                        inputs.diagnostic(
                            format!("npm addon '{path}' has no packaged loader closure: {error}\n")
                                .as_bytes(),
                        );
                        Some(NativeRejection::Closure)
                    } else {
                        let arguments = [
                            input.join("probe.cjs").into_os_string(),
                            input.join(path).into_os_string(),
                        ];
                        let launch = dever_sandbox::Launch {
                            assets: &assets,
                            worker: &input,
                            executable: &executable,
                            arguments: &arguments,
                            working_directory: &input,
                            capabilities: dever_sandbox::Capabilities::default(),
                            grants: &[],
                        };
                        let admitted = stage.probe(launch.command()?)?;
                        inputs.diagnostic(&stage.diagnostics()?);
                        if admitted {
                            None
                        } else {
                            Some(NativeRejection::Unloadable)
                        }
                    }
                }
            };
            if let Some(reason) = rejection {
                output.rejections.insert(path.clone(), reason);
            } else {
                output.native.insert(path.clone());
            }
        }
        if !required.is_subset(&output.native) {
            return Err("source-built npm addon did not pass runtime-only native admission".into());
        }
        Ok(())
    }
}
