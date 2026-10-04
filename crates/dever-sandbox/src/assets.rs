//! Validate the bootstrap dependency closure before executing any asset.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use goblin::elf::{Elf, dynamic, header, program_header};
use sha2::{Digest, Sha256};

const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;
const MAX_ASSET_FILES: usize = 128;

/// The first executable must be independent of every host loader and library.
pub fn validate_bwrap(bytes: &[u8]) -> Result<String, String> {
    validate_bwrap_for_target(bytes, host_target())
}

fn host_target() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "linux-aarch64"
    } else {
        "linux-x86_64"
    }
}

fn target_abi(target: &str) -> Result<(u16, &'static str, &'static str), String> {
    match target {
        "linux-x86_64" => Ok((
            header::EM_X86_64,
            "ld-linux-x86-64.so.2",
            "/lib64/ld-linux-x86-64.so.2",
        )),
        "linux-aarch64" => Ok((
            header::EM_AARCH64,
            "ld-linux-aarch64.so.1",
            "/lib/ld-linux-aarch64.so.1",
        )),
        _ => Err(format!("unsupported sandbox target '{target}'")),
    }
}

pub fn validate_bwrap_for_target(bytes: &[u8], target: &str) -> Result<String, String> {
    let elf = Elf::parse(bytes).map_err(|error| format!("invalid sandbox bwrap ELF: {error}"))?;
    let (machine, _, _) = target_abi(target)?;
    if bytes.len() > MAX_ASSET_BYTES
        || !elf.is_64
        || !elf.little_endian
        || elf.header.e_machine != machine
        || !matches!(elf.header.e_type, header::ET_DYN | header::ET_EXEC)
        || elf
            .program_headers
            .iter()
            .any(|segment| segment.p_type == program_header::PT_INTERP)
        || elf.dynamic.as_ref().is_some_and(|table| {
            table
                .dyns
                .iter()
                .any(|entry| entry.d_tag == dynamic::DT_NEEDED)
        })
        || !elf.rpaths.is_empty()
        || !elf.runpaths.is_empty()
    {
        return Err("sandbox bwrap must be a static target ELF without PT_INTERP, DT_NEEDED or library search paths".into());
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Used both before signing a pack and when launching its verified extraction.
/// Content authentication belongs to the release/bundle owner; this validates
/// that the loader can resolve every dependency without a host-library search.
pub fn validate_assets(files: &[(String, Vec<u8>)]) -> Result<(), String> {
    validate_assets_for_target(files, host_target())
}

pub fn validate_assets_for_target(files: &[(String, Vec<u8>)], target: &str) -> Result<(), String> {
    let (machine, loader_name, interpreter) = target_abi(target)?;
    let total = files
        .iter()
        .try_fold(0usize, |total, (_, bytes)| total.checked_add(bytes.len()))
        .ok_or("sandbox asset size overflow")?;
    if files.len() > MAX_ASSET_FILES || total > MAX_ASSET_BYTES {
        return Err("sandbox assets exceed their file or byte budget".into());
    }
    let mut supplied = BTreeMap::new();
    for (name, bytes) in files {
        let valid = matches!(name.as_str(), "bin/bwrap" | "bin/guard")
            || name.strip_prefix("lib/").is_some_and(|name| {
                !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
            });
        if !valid || supplied.insert(name.as_str(), bytes.as_slice()).is_some() {
            return Err("sandbox asset has an invalid or repeated path".into());
        }
    }
    let loader = format!("lib/{loader_name}");
    for required in ["bin/bwrap", "bin/guard", &loader] {
        if !supplied.contains_key(required) {
            return Err(format!("sandbox asset '{required}' is missing"));
        }
    }
    validate_bwrap_for_target(supplied["bin/bwrap"], target)?;
    let mut reached = BTreeSet::new();
    // The shared OS ABI set also serves packaged language runtimes. Validate
    // every supplied library even when the two bootstrap helpers do not use it.
    let mut remaining = supplied
        .keys()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    while let Some(name) = remaining.pop() {
        if !reached.insert(name.clone()) {
            continue;
        }
        let bytes = supplied
            .get(name.as_str())
            .ok_or_else(|| format!("sandbox dependency '{name}' is missing"))?;
        let elf =
            Elf::parse(bytes).map_err(|error| format!("invalid sandbox ELF asset: {error}"))?;
        if !elf.is_64
            || !elf.little_endian
            || elf.header.e_machine != machine
            || !matches!(elf.header.e_type, header::ET_DYN | header::ET_EXEC)
        {
            return Err("sandbox ELF asset has an incompatible target or object type".into());
        }
        // Reject paths that can outrank the explicit bootstrap library path.
        if !elf.rpaths.is_empty() || !elf.runpaths.is_empty() {
            return Err("sandbox bootstrap assets must not declare RPATH or RUNPATH".into());
        }
        if let Some(actual) = elf.interpreter
            && actual != interpreter
        {
            return Err("sandbox ELF interpreter differs from the packaged loader".into());
        }
        for library in elf.libraries {
            if library.is_empty() || library.contains(['/', '\\']) || matches!(library, "." | "..")
            {
                return Err("sandbox dependency is not a plain library name".into());
            }
            remaining.push(format!("lib/{library}"));
        }
    }
    Ok(())
}

pub(super) fn verify_directory(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    let mut remaining = MAX_ASSET_BYTES;
    for directory in ["bin", "lib"] {
        let directory_path = super::real_path(&root.join(directory))?;
        for entry in fs::read_dir(directory_path).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = super::real_path(&entry.path())?;
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
                || files.len() >= MAX_ASSET_FILES
            {
                return Err("sandbox assets contain a non-regular file or too many files".into());
            }
            let mut bytes = Vec::new();
            fs::File::open(path)
                .and_then(|file| file.take(remaining as u64 + 1).read_to_end(&mut bytes))
                .map_err(|error| format!("cannot read sandbox asset: {error}"))?;
            remaining = remaining
                .checked_sub(bytes.len())
                .ok_or("sandbox assets exceed their byte budget")?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "sandbox asset name is not UTF-8")?;
            files.push((format!("{directory}/{name}"), bytes));
        }
    }
    validate_assets(&files)?;
    let bytes = &files
        .iter()
        .find(|(name, _)| name == "bin/bwrap")
        .ok_or("sandbox bwrap is missing")?
        .1;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
