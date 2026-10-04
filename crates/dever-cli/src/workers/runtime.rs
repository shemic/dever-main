//! One runtime descriptor and file-closure contract for authoring and Worker preparation.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use super::{MAX_TREE_BYTES, MAX_TREE_FILES, go_build, valid_path};
use crate::libs::Ecosystem;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    format: String,
    ecosystem: String,
    pub(crate) target: String,
    pub(crate) executable: Option<String>,
    #[serde(default)]
    pub(super) arguments: Vec<String>,
    pub(super) build: Option<go_build::GoBuildManifest>,
    #[serde(default)]
    pub(crate) python_wheel_tags: Vec<String>,
    #[serde(default)]
    pub(crate) python_extension_suffixes: Vec<String>,
    pub(crate) python_markers: Option<pep508_rs::MarkerEnvironment>,
}

impl Manifest {
    pub(crate) fn executable(&self, path: &str) -> bool {
        self.executable.as_deref() == Some(path)
            || self
                .build
                .as_ref()
                .is_some_and(|build| build.executable(path))
    }
}

pub(crate) fn validate(
    files: &[(String, Vec<u8>)],
    ecosystem: &Ecosystem,
    target: &str,
) -> Result<Manifest, String> {
    if files.len() > MAX_TREE_FILES {
        return Err("managed runtime pack has too many files".into());
    }
    let mut paths = BTreeSet::new();
    let mut total = 0usize;
    for (path, bytes) in files {
        valid_path(path)?;
        if !paths.insert(path.as_str()) {
            return Err("managed runtime pack repeats a file".into());
        }
        total = total
            .checked_add(bytes.len())
            .ok_or("managed runtime pack size overflow")?;
        if total > MAX_TREE_BYTES {
            return Err("managed runtime pack exceeds 256 MiB".into());
        }
    }
    for path in &paths {
        for parent in Path::new(path).ancestors().skip(1) {
            if parent.to_str().is_some_and(|name| paths.contains(name)) {
                return Err("managed runtime pack has a file/directory conflict".into());
            }
        }
    }
    let metadata = files
        .iter()
        .find(|(path, _)| path == "dever-runtime.json")
        .ok_or("managed runtime pack has no dever-runtime.json")?;
    let manifest: Manifest = serde_json::from_slice(&metadata.1)
        .map_err(|error| format!("invalid managed runtime manifest: {error}"))?;
    if manifest.format != "dever-worker-runtime-v1"
        || manifest.ecosystem != ecosystem.as_str()
        || manifest.target != target
    {
        return Err("managed runtime pack identity differs from its lock and target".into());
    }
    if ecosystem != &Ecosystem::Pip
        && (!manifest.python_wheel_tags.is_empty()
            || !manifest.python_extension_suffixes.is_empty()
            || manifest.python_markers.is_some())
    {
        return Err("non-Python runtime must not declare Python ABI metadata".into());
    }
    if ecosystem == &Ecosystem::Go {
        if manifest.executable.is_some() || !manifest.arguments.is_empty() {
            return Err("managed Go build pack must not declare a runtime interpreter".into());
        }
        manifest
            .build
            .as_ref()
            .ok_or("managed Go Worker requires a locked compile/link build pack")?
            .validate_files(files, target)?;
    } else {
        if manifest.build.is_some() {
            return Err("managed interpreter pack must not declare Go build tools".into());
        }
        let executable = manifest
            .executable
            .as_deref()
            .ok_or("managed runtime executable is missing")?;
        valid_path(executable)?;
        if !executable.starts_with("bin/")
            || manifest.arguments.len() > 64
            || manifest
                .arguments
                .iter()
                .any(|argument| !argument.starts_with('-') || argument.contains('\0'))
        {
            return Err("managed runtime executable or fixed flags are invalid".into());
        }
        if ecosystem == &Ecosystem::Pip && manifest.arguments != ["-I", "-S", "-B"] {
            return Err(
                "managed Python runtime requires the fixed -I -S -B isolation flags".into(),
            );
        }
        if ecosystem == &Ecosystem::Pip {
            if manifest.python_markers.is_none() {
                return Err("managed Python runtime lacks its marker environment".into());
            }
            super::python_wheel::validate_target(
                target,
                &manifest.python_wheel_tags,
                manifest
                    .python_markers
                    .as_ref()
                    .expect("validated Python markers"),
            )?;
            let mut suffixes = BTreeSet::new();
            if manifest.python_extension_suffixes.is_empty()
                || manifest.python_extension_suffixes.len() > 16
                || manifest.python_extension_suffixes.iter().any(|suffix| {
                    !suffix.starts_with('.')
                        || !suffix.ends_with(".so")
                        || !suffix
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                        || !suffixes.insert(suffix)
                })
            {
                return Err(
                    "managed Python extension suffixes are missing, invalid or repeated".into(),
                );
            }
        }
        if !files
            .iter()
            .any(|(path, bytes)| path == executable && !bytes.is_empty())
        {
            return Err("managed runtime executable is missing from its pack".into());
        }
        for (path, bytes) in files {
            if path == executable || bytes.starts_with(b"\x7fELF") {
                validate_binary(path, bytes, target)?;
            }
        }
    }
    Ok(manifest)
}

pub(crate) fn validate_binary(path: &str, bytes: &[u8], target: &str) -> Result<(), String> {
    use goblin::elf::{Elf, header};
    let (machine, interpreter) = match target.parse::<crate::toolchain::BuildTarget>()? {
        crate::toolchain::BuildTarget::LinuxX86_64 => {
            (header::EM_X86_64, "/lib64/ld-linux-x86-64.so.2")
        }
        crate::toolchain::BuildTarget::LinuxAarch64 => {
            (header::EM_AARCH64, "/lib/ld-linux-aarch64.so.1")
        }
    };
    let elf =
        Elf::parse(bytes).map_err(|error| format!("invalid runtime ELF '{path}': {error}"))?;
    if !elf.is_64
        || !elf.little_endian
        || elf.header.e_machine != machine
        || !matches!(elf.header.e_type, header::ET_DYN | header::ET_EXEC)
        || elf.interpreter.is_some_and(|actual| actual != interpreter)
    {
        return Err(format!("runtime ELF '{path}' differs from target {target}"));
    }
    Ok(())
}
