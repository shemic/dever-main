//! Native compiler loader closure, shared by authoring and signed installation.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use goblin::elf::{Elf, dynamic, header};

use super::{Artifact, runtime_pack};

const MAX_FILES: usize = 64;
const MAX_BYTES: u64 = 512 * 1024 * 1024;

pub(super) fn library_name(name: &str) -> Result<(), String> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', '\0', ':']) {
        return Err("compiler dependency must use a plain library name".into());
    }
    Ok(())
}

fn os_library(name: &str) -> bool {
    // These are the declared GNU OS ABI, not host-discovered dependencies.
    matches!(
        name,
        "libc.so.6"
            | "libm.so.6"
            | "libdl.so.2"
            | "libpthread.so.0"
            | "librt.so.1"
            | "libresolv.so.2"
            | "libutil.so.1"
    ) || name == loader_name()
}

fn loader_name() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "ld-linux-aarch64.so.1"
    } else {
        "ld-linux-x86-64.so.2"
    }
}

pub(super) fn validate(root: &Path, artifacts: &[Artifact]) -> Result<(), String> {
    validate_entries(root, artifacts, &["dever-core"])
}

/// Both release components keep their complete non-OS loader closure in lib/.
/// Their entry sets differ; the loader and file admission rules do not.
pub(super) fn validate_entries(
    root: &Path,
    artifacts: &[Artifact],
    entries: &[&str],
) -> Result<(), String> {
    runtime_pack::host_target()?;
    dever_sandbox::require_no_loader_preload()?;
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for artifact in artifacts {
        if !entries.contains(&artifact.path.as_str()) && !artifact.path.starts_with("lib/") {
            continue;
        }
        if let Some(name) = artifact.path.strip_prefix("lib/") {
            library_name(name)?;
            if os_library(name) {
                return Err("compiler pack must not override the GNU OS ABI libraries".into());
            }
        }
        total = total
            .checked_add(artifact.bytes)
            .ok_or("compiler closure size overflow")?;
        if total > MAX_BYTES || files.len() >= MAX_FILES {
            return Err("compiler loader closure exceeds its file or byte budget".into());
        }
        if files.insert(artifact.path.as_str(), artifact).is_some() {
            return Err("compiler loader closure has duplicate files".into());
        }
    }
    validate_library_directory(root, &files)?;
    let mut remaining = entries
        .iter()
        .map(|entry| (*entry).to_owned())
        .collect::<Vec<_>>();
    let mut reached = BTreeSet::new();
    while let Some(path) = remaining.pop() {
        if !reached.insert(path.clone()) {
            continue;
        }
        let artifact = files.get(path.as_str()).ok_or_else(|| {
            format!("compiler dependency '{path}' is not supplied by the signed release")
        })?;
        let input = runtime_pack::checked_path(root, &path, true)?;
        let mut bytes = Vec::new();
        File::open(input)
            .and_then(|file| file.take(artifact.bytes + 1).read_to_end(&mut bytes))
            .map_err(|error| format!("cannot read compiler loader input: {error}"))?;
        if bytes.len() as u64 != artifact.bytes {
            return Err("compiler loader input size differs from its signed declaration".into());
        }
        remaining.extend(dependencies(
            &path,
            &bytes,
            entries.contains(&path.as_str()),
        )?);
    }
    if reached.len() != files.len() {
        return Err("compiler closure contains libraries unused by the core".into());
    }
    Ok(())
}

fn dependencies(path: &str, bytes: &[u8], executable: bool) -> Result<Vec<String>, String> {
    let elf = Elf::parse(bytes).map_err(|error| format!("invalid compiler ELF input: {error}"))?;
    let machine = if cfg!(target_arch = "aarch64") {
        header::EM_AARCH64
    } else {
        header::EM_X86_64
    };
    if !elf.is_64
        || !elf.little_endian
        || elf.header.e_machine != machine
        || !matches!(elf.header.e_type, header::ET_DYN | header::ET_EXEC)
    {
        return Err("compiler ELF input has an incompatible target or object type".into());
    }
    // GNU auditing/config and ELF filter tags can load libraries outside
    // DT_NEEDED; they are not part of this release's loader contract.
    if elf.dynamic.as_ref().is_some_and(|table| {
        table.dyns.iter().any(|entry| {
            matches!(
                entry.d_tag,
                dynamic::DT_CONFIG
                    | dynamic::DT_AUDIT
                    | dynamic::DT_DEPAUDIT
                    | 0x7fff_fffd
                    | 0x7fff_ffff
            )
        })
    }) {
        return Err("compiler ELF input contains unsupported dynamic loader hooks".into());
    }
    if executable {
        let interpreter = if cfg!(target_arch = "aarch64") {
            "/lib/ld-linux-aarch64.so.1"
        } else {
            "/lib64/ld-linux-x86-64.so.2"
        };
        if elf.interpreter != Some(interpreter)
            || elf.rpaths != ["$ORIGIN/lib"]
            || !elf.runpaths.is_empty()
        {
            return Err("compiler requires the GNU loader and inherited RPATH=$ORIGIN/lib".into());
        }
    } else {
        if elf.header.e_type != header::ET_DYN || elf.soname != path.strip_prefix("lib/") {
            return Err("compiler library SONAME must match its signed filename".into());
        }
        // Upstream LLVM uses $ORIGIN/../lib, which resolves to this same
        // private lib directory. No search entry may reach the host tree.
        if elf
            .rpaths
            .iter()
            .chain(&elf.runpaths)
            .any(|path| !matches!(*path, "$ORIGIN" | "$ORIGIN/../lib"))
        {
            return Err("compiler library search path leaves its private library directory".into());
        }
    }
    let mut dependencies = Vec::new();
    for name in elf.libraries {
        library_name(name)?;
        if !os_library(name) {
            dependencies.push(format!("lib/{name}"));
        }
    }
    Ok(dependencies)
}

fn validate_library_directory(
    root: &Path,
    files: &BTreeMap<&str, &Artifact>,
) -> Result<(), String> {
    let mut expected = files
        .keys()
        .filter_map(|path| path.strip_prefix("lib/"))
        .collect::<BTreeSet<_>>();
    match std::fs::symlink_metadata(root.join("lib")) {
        Err(error) if expected.is_empty() && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(format!("cannot inspect compiler lib directory: {error}")),
        Ok(_) => {}
    }
    let directory = runtime_pack::checked_path(root, "lib", false)?;
    for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
            || !name.to_str().is_some_and(|name| expected.remove(name))
        {
            return Err(
                "compiler lib directory contains an unsigned file, directory or link".into(),
            );
        }
    }
    if !expected.is_empty() {
        return Err("compiler lib directory is missing a signed library".into());
    }
    Ok(())
}
