//! Explicit test inputs: no downloads, recursive builds or host tool lookup.
use std::fs;
use std::path::{Path, PathBuf};

use dever_cli::toolchain::sha256_file as hash_file;
use serde_json::{Value, json};

#[path = "llvm_inputs.rs"]
mod inputs;
#[cfg(test)]
#[path = "native_objects.rs"]
mod native_objects;
use inputs::{core_libraries, native_inputs};

#[cfg(test)]
pub fn compiler(source: &Path, root: &Path, name: &str) -> PathBuf {
    fs::create_dir_all(root).unwrap();
    let path = root.join(name);
    fs::hard_link(source, &path).unwrap();
    compiler_library(root).unwrap();
    path
}

pub fn pack_root(root: &Path) -> PathBuf {
    root.join("runtime")
        .join(dever_cli::toolchain::platform_identity())
}

#[cfg(test)]
pub fn manifest(root: &Path, real: bool) -> Value {
    let destination = pack_root(root);
    fs::create_dir_all(&destination).unwrap();
    let sources = native_inputs();
    let mut files = Vec::new();
    for (name, source) in sources {
        let output = destination.join(name);
        if real {
            assert!(
                source.is_file(),
                "prepare the explicit native archive and CRT fixture first: {}",
                source.display()
            );
            // Only immutable reads follow. Corruption cases use synthetic files.
            fs::hard_link(source, &output).unwrap();
        } else {
            fs::write(
                &output,
                if name.ends_with(".a") {
                    native_objects::archive(
                        dever_cli::toolchain::BuildTarget::host().unwrap(),
                        "fixture.o",
                    )
                } else {
                    native_objects::object(dever_cli::toolchain::BuildTarget::host().unwrap())
                },
            )
            .unwrap();
        }
        files.push(json!({"path":name, "bytes":fs::metadata(&output).unwrap().len(), "sha256":digest(&output)}));
    }
    let document = document(files);
    write_manifest(root, &document);
    document
}

fn document(files: Vec<Value>) -> Value {
    let profile = json!({
        "start":["crt1.o","crti.o","crtbeginT.o"], "runtime":"runtime.a",
        "libraries":["libc.a","libm.a","libmvec.a","libgcc.a","libgcc_eh.a"],
        "end":["crtend.o","crtn.o"]
    });
    json!({
        "format":"dever-native-runtime-v1", "compiler_version":env!("CARGO_PKG_VERSION"),
        "abi":dever_backend_bridge::RUNTIME_ABI_VERSION,
        "platform":dever_cli::toolchain::platform_identity(),
        "target":"x86_64-unknown-linux-gnu",
        "profiles":{"base":profile,"sqlite":profile,"postgres":profile,"both":profile},
        "files":files
    })
}

fn compiler_library(root: &Path) -> Result<(), String> {
    let libraries = root.join("lib");
    create_or_check_directory(&libraries)?;
    for (name, source) in core_libraries() {
        let target = libraries.join(name);
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                if hash_file(&target)? != hash_file(&source)? {
                    return Err(
                        "existing compiler library differs from the explicit author SDK".into(),
                    );
                }
            }
            Ok(_) => return Err("compiler library must be a real file".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::hard_link(source, target).map_err(|error| error.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn create_or_check_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(format!("'{}' must be a real directory", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
pub fn write_manifest(root: &Path, manifest: &Value) {
    fs::write(
        pack_root(root).join("manifest.json"),
        serde_json::to_vec(manifest).unwrap(),
    )
    .unwrap();
}

#[cfg(test)]
pub fn digest(path: &Path) -> String {
    hash_file(path).unwrap()
}
