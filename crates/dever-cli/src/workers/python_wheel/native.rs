//! Validate extension ABI and the dynamic loader's closed, packaged search tree.

use super::super::elf::{Closure, object, origin_directories};
use goblin::elf::header;
use std::collections::{BTreeMap, BTreeSet};

/// `files` contains Worker-relative installation files and separately verified
/// OS ABI files at sandbox/lib/*. No host directory is searched or executed.
pub fn validate_native(
    target: &str,
    suffixes: &[String],
    interpreter: &str,
    files: &BTreeMap<String, &[u8]>,
    wheel_files: &BTreeSet<String>,
) -> Result<(), String> {
    let native = files
        .iter()
        .filter(|(path, _)| wheel_files.contains(*path))
        .filter(|(path, bytes)| {
            bytes.starts_with(b"\x7fELF")
                || path.ends_with(".so")
                || path.contains(".so.")
                || path.ends_with(".pyd")
                || path.ends_with(".dll")
                || path.ends_with(".dylib")
        })
        .collect::<Vec<_>>();
    if native.is_empty() {
        return Ok(());
    }
    let machine = match target {
        "linux-x86_64" => header::EM_X86_64,
        "linux-aarch64" => header::EM_AARCH64,
        _ => return Err("native Python wheel target is unsupported".into()),
    };
    let closure = Closure {
        files,
        machine,
        directories: vec!["sandbox/lib".into()],
    };
    let interpreter_elf = object(
        interpreter,
        files
            .get(interpreter)
            .ok_or("native wheel requires its packaged Python executable")?,
        machine,
    )?;
    let inherited = if interpreter_elf.runpaths.is_empty() {
        origin_directories(interpreter, &interpreter_elf.rpaths)?
    } else {
        vec![]
    };
    let (globals, _) = closure.follow(interpreter, vec![], &BTreeMap::new())?;
    let mut entries = Vec::new();
    for (path, bytes) in native {
        if path.ends_with(".pyd") || path.ends_with(".dll") || path.ends_with(".dylib") {
            return Err("native Python wheel contains a binary for a different OS".into());
        }
        let elf = object(path, bytes, machine)?;
        if elf.header.e_type != header::ET_DYN || elf.interpreter.is_some() {
            return Err("native wheel content must be a shared library, not an executable".into());
        }
        let module = elf.dynsyms.iter().any(|symbol| {
            symbol.st_shndx != 0
                && elf
                    .dynstrtab
                    .get_at(symbol.st_name)
                    .is_some_and(|name| name.starts_with("PyInit_"))
        });
        let module = module || path.contains(".cpython-") || path.ends_with(".abi3.so");
        if module {
            let compatible = suffixes.iter().any(|suffix| {
                path.ends_with(suffix)
                    && (suffix != ".so"
                        || !path.contains(".cpython-") && !path.ends_with(".abi3.so"))
            });
            if !compatible {
                return Err(
                    "native Python extension suffix differs from the signed interpreter ABI".into(),
                );
            }
        }
        entries.push((path, module));
    }
    entries.sort_by_key(|(_, module)| !*module);
    let mut reached = BTreeSet::new();
    for (path, module) in entries {
        // Vendored helpers inherit the importing extension's loader context;
        // reaching one does not imply an independent direct dlopen entry.
        if !module && reached.contains(path) {
            continue;
        }
        let (_, closure_paths) = closure.follow(path, inherited.clone(), &globals)?;
        reached.extend(closure_paths);
    }
    Ok(())
}

/// Compiler processes use the signed sysroot's standard library locations.
/// Reuse the ELF version/SONAME closure checks used by installed extensions.
pub(crate) fn validate_build_tools(
    target: &str,
    executables: &[String],
    files: &BTreeMap<String, &[u8]>,
) -> Result<(), String> {
    let (machine, multiarch, interpreter) = match target {
        "linux-x86_64" => (
            header::EM_X86_64,
            "x86_64-linux-gnu",
            "/lib64/ld-linux-x86-64.so.2",
        ),
        "linux-aarch64" => (
            header::EM_AARCH64,
            "aarch64-linux-gnu",
            "/lib/ld-linux-aarch64.so.1",
        ),
        _ => return Err("build tools target is unsupported".into()),
    };
    let closure = Closure {
        files,
        machine,
        directories: vec![
            format!("rootfs/usr/lib/{multiarch}"),
            "sandbox/lib".into(),
            "rootfs/usr/lib".into(),
        ],
    };
    for path in executables {
        let bytes = files.get(path).ok_or("build executable is missing")?;
        if bytes.starts_with(b"#!") {
            let first = bytes
                .split(|byte| *byte == b'\n')
                .next()
                .unwrap_or_default();
            if !matches!(
                first,
                b"#!/bin/sh"
                    | b"#!/usr/bin/sh"
                    | b"#!/usr/bin/env sh"
                    | b"#!/usr/bin/env node"
                    | b"#!/usr/bin/env python3"
            ) {
                return Err("build tool script must name the signed shell directly".into());
            }
            continue;
        }
        let elf = object(path, bytes, machine)?;
        if elf.interpreter.is_some_and(|value| value != interpreter) {
            return Err("build tool interpreter differs from signed sandbox loader".into());
        }
        closure.follow(path, vec![], &BTreeMap::new())?;
    }
    Ok(())
}
