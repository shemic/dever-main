//! Offline native command trees, including their explicitly supplied libraries.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use dever_core::hir::ExternalWorkerContract;
use dever_core::native::EmbeddedResource;
use goblin::elf::{dynamic, header};
use serde_json::json;

use super::{MAX_TREE_BYTES, MAX_TREE_FILES, Tree, digest, elf, resource, runtime, valid_path};
use crate::toolchain::BuildTarget;

pub(super) fn prepare(
    root: &Path,
    contract: &ExternalWorkerContract,
    package_files: &BTreeMap<String, Vec<u8>>,
    supplied: &BTreeMap<&str, &EmbeddedResource>,
    target: BuildTarget,
) -> Result<Vec<EmbeddedResource>, String> {
    let prefix = format!(
        "commands/{}",
        digest(&serde_json::to_vec(&contract.sdk_manifest()).map_err(|error| error.to_string())?)
    );
    let mut tree = Tree::new(prefix.clone());
    let source = command_files(root, &contract.entry, package_files)?;
    let binary = &source["program/tool"];
    runtime::validate_binary(&contract.entry, binary, target.platform())?;
    let machine = match target {
        BuildTarget::LinuxX86_64 => header::EM_X86_64,
        BuildTarget::LinuxAarch64 => header::EM_AARCH64,
    };
    let executable = elf::object("program/tool", binary, machine)?;
    let interpreter = executable.interpreter.map(str::to_owned);
    if interpreter.is_none() && !executable.libraries.is_empty() {
        return Err("command executable has dynamic dependencies without an interpreter".into());
    }
    let mut files = source
        .iter()
        .map(|(path, bytes)| (path.clone(), bytes.as_slice()))
        .collect::<BTreeMap<_, _>>();
    files.extend(
        supplied
            .iter()
            .filter(|(path, _)| path.starts_with("sandbox/lib/"))
            .map(|(path, entry)| ((*path).to_owned(), entry.bytes.as_slice())),
    );
    let closure = elf::Closure {
        files: &files,
        machine,
        directories: vec!["program/lib".into(), "sandbox/lib".into()],
    };
    for (path, bytes) in &source {
        let object = elf::object(path, bytes, machine)?;
        reject_loader_hooks(&object)?;
        // The logical program root becomes /worker in the sandbox. A path
        // escaping it could otherwise resolve a library from a file grant.
        for fields in [&object.rpaths, &object.runpaths] {
            for directory in elf::origin_directories(path, fields)? {
                if directory != "program" && !directory.starts_with("program/") {
                    return Err("command library search path escapes its packaged tree".into());
                }
            }
        }
        if let Some(name) = path.strip_prefix("program/lib/") {
            if supplied.contains_key(format!("sandbox/lib/{name}").as_str()) {
                return Err(format!(
                    "command library '{name}' overrides a signed sandbox library"
                ));
            }
            if object.header.e_type != header::ET_DYN
                || object.interpreter.is_some()
                || object.soname.is_some_and(|soname| soname != name)
            {
                return Err(format!(
                    "command library '{name}' is not a matching SONAME shared object"
                ));
            }
        }
        closure.follow(path, Vec::new(), &BTreeMap::new())?;
    }
    for (path, bytes) in source {
        let name = path.strip_prefix("program/").expect("owned command tree");
        tree.add(name, bytes, name == "tool")?;
    }
    let (executable, arguments) = if let Some(interpreter) = interpreter {
        let name = interpreter
            .rsplit('/')
            .next()
            .expect("validated GNU loader");
        let loader = supplied
            .get(format!("sandbox/lib/{name}").as_str())
            .ok_or("command requires the signed target GNU loader")?;
        runtime::validate_binary(name, &loader.bytes, target.platform())?;
        tree.add("loader", loader.bytes.clone(), true)?;
        (
            format!("{prefix}/loader"),
            vec![
                json!({"literal": "--inhibit-cache"}),
                json!({"literal": "--library-path"}),
                json!({"literal": "lib:/lib"}),
                json!({"resource": format!("{prefix}/tool")}),
            ],
        )
    } else {
        (format!("{prefix}/tool"), Vec::new())
    };
    let manifest = json!({
        "format": "dever-command-launch-v1", "ecosystem": "command",
        "entry": contract.entry, "executable": executable,
        "arguments": arguments, "working_directory": prefix,
    });
    let mut output = tree.finish();
    output.push(resource(
        format!("{}.dever-command.json", contract.entry),
        serde_json::to_vec(&manifest).map_err(|error| error.to_string())?,
        false,
    ));
    Ok(output)
}

fn reject_loader_hooks(elf: &goblin::elf::Elf<'_>) -> Result<(), String> {
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
        return Err("command ELF contains unsupported dynamic loader hooks".into());
    }
    Ok(())
}

fn command_files(
    root: &Path,
    entry: &str,
    package_files: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    valid_path(entry)?;
    let parent = entry
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .ok_or("command entry must belong to a domain")?;
    let library_prefix = format!("{parent}/lib/");
    let packaged = crate::packages::owns_worker(root, entry)?;
    let binary = if packaged {
        package_files
            .get(entry)
            .cloned()
            .ok_or("Package command executable is missing")?
    } else {
        read_file(root, entry)?
    };
    let mut files = BTreeMap::from([("program/tool".to_owned(), binary)]);
    let mut bytes = files["program/tool"].len();
    if bytes == 0 || bytes > MAX_TREE_BYTES {
        return Err("command executable is empty or exceeds the byte budget".into());
    }
    let names = if packaged {
        package_files
            .keys()
            .filter_map(|path| path.strip_prefix(&library_prefix))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    } else {
        library_names(&root.join(&library_prefix))?
    };
    for name in names {
        valid_path(&name)?;
        if name.contains('/') || files.len() >= MAX_TREE_FILES {
            return Err("command lib directory must contain bounded regular library files".into());
        }
        let path = format!("{library_prefix}{name}");
        let library = if packaged {
            package_files[&path].clone()
        } else {
            read_file(root, &path)?
        };
        bytes = bytes
            .checked_add(library.len())
            .ok_or("command library size overflow")?;
        if bytes > MAX_TREE_BYTES {
            return Err("command libraries exceed the byte budget".into());
        }
        files.insert(format!("program/lib/{name}"), library);
    }
    Ok(files)
}

fn library_names(directory: &Path) -> Result<Vec<String>, String> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot inspect command lib directory: {error}")),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("command lib directory must be a real directory".into());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if names.len() >= MAX_TREE_FILES
            || !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
        {
            return Err("command lib directory must contain bounded regular library files".into());
        }
        names.push(
            entry
                .file_name()
                .into_string()
                .map_err(|_| "command library name is not UTF-8")?,
        );
    }
    names.sort();
    Ok(names)
}

fn read_file(root: &Path, entry: &str) -> Result<Vec<u8>, String> {
    let path = crate::libs::owned_worker(root, entry)?;
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_TREE_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read command resource '{entry}': {error}"))?;
    if bytes.len() > MAX_TREE_BYTES {
        return Err("command resource exceeds the byte budget".into());
    }
    Ok(bytes)
}
