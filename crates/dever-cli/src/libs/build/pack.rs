//! Signed, target-specific source-build tools. Never sourced from host PATH.

use crate::libs::{Ecosystem, RegistryRuntime};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PACK_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_EXPANDED_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_FILES: usize = 65_536;

pub type PackFiles = Vec<(String, Vec<u8>)>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub ecosystem: Ecosystem,
    pub target: String,
    pub runtime_sha256: String,
    pub python_runtime_sha256: Option<String>,
    pub executables: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub format: String,
    pub ecosystem: Ecosystem,
    pub target: String,
    pub runtime_sha256: String,
    pub python_runtime_sha256: Option<String>,
    pub sha256: String,
}

fn hash(value: &str) -> Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("build pack identity must be canonical SHA-256".into());
    }
    Ok(())
}

impl Descriptor {
    pub fn validate(
        &self,
        runtime: &RegistryRuntime,
        python: Option<&RegistryRuntime>,
    ) -> Result<(), String> {
        if self.format != "dever-registry-build-v1"
            || self.ecosystem == Ecosystem::Go
            || self.target != runtime.target
            || self.runtime_sha256 != runtime.pack.sha256
        {
            return Err("signed build pack does not match its runtime and target".into());
        }
        hash(&self.sha256)?;
        hash(&self.runtime_sha256)?;
        match (&self.ecosystem, &self.python_runtime_sha256, python) {
            (Ecosystem::Pip, None, _) => Ok(()),
            (Ecosystem::Npm, Some(identity), Some(python))
                if identity == &python.pack.sha256 && python.target == self.target =>
            {
                hash(identity)
            }
            _ => Err("build pack has no exact auxiliary Python runtime binding".into()),
        }
    }
}

impl Manifest {
    pub fn validate(
        &self,
        files: &[(String, Vec<u8>)],
        descriptor: &Descriptor,
    ) -> Result<(), String> {
        if self.format != "dever-lib-build-v1"
            || self.ecosystem != descriptor.ecosystem
            || self.target != descriptor.target
            || self.runtime_sha256 != descriptor.runtime_sha256
            || self.python_runtime_sha256 != descriptor.python_runtime_sha256
        {
            return Err("build pack manifest differs from its signed descriptor".into());
        }
        let total = files
            .iter()
            .try_fold(0usize, |total, (_, bytes)| total.checked_add(bytes.len()))
            .ok_or("build pack expanded size overflow")?;
        if files.len() > MAX_FILES || total > MAX_EXPANDED_BYTES {
            return Err("build pack exceeds file or byte budget".into());
        }
        let mut names = BTreeSet::new();
        for (path, _) in files {
            crate::workers::valid_path(path)?;
            if path != "dever-build.json"
                && !path.starts_with("rootfs/usr/")
                && !(self.ecosystem == Ecosystem::Pip && path.starts_with("runtime/include/"))
                && !(self.ecosystem == Ecosystem::Npm
                    && (path.starts_with("npm/include/node/")
                        || path.starts_with("npm/frontend/node_modules/")))
            {
                return Err(format!(
                    "build pack path '{path}' is outside its fixed tool layout"
                ));
            }
            if !names.insert(path.as_str()) {
                return Err("build pack repeats a path".into());
            }
        }
        let mut executable_names = BTreeSet::new();
        for path in &self.executables {
            if !names.contains(path.as_str())
                || !executable_names.insert(path.as_str())
                || !(path.starts_with("rootfs/usr/")
                    || self.ecosystem == Ecosystem::Npm
                        && path.starts_with("npm/frontend/node_modules/"))
            {
                return Err(
                    "build executable is absent, repeated, or outside its tool root".into(),
                );
            }
        }
        for name in ["sh", "cc", "c++", "as", "ld", "ar", "ranlib", "make"] {
            if !executable_names.contains(format!("rootfs/usr/bin/{name}").as_str()) {
                return Err(format!("build pack is missing required tool '{name}'"));
            }
        }
        Ok(())
    }
}

pub fn unpack(bytes: &[u8], descriptor: &Descriptor) -> Result<(Manifest, PackFiles), String> {
    if bytes.len() > MAX_PACK_BYTES || crate::libs::sha256(bytes) != descriptor.sha256 {
        return Err("build pack exceeds its budget or SHA-256 differs".into());
    }
    let files = super::super::registry::tar_files(bytes, "tgz", MAX_EXPANDED_BYTES as u64)?;
    let metadata = files
        .iter()
        .find(|(path, _)| path == "dever-build.json")
        .ok_or("build pack is missing dever-build.json")?;
    let manifest: Manifest = serde_json::from_slice(&metadata.1)
        .map_err(|error| format!("invalid build pack manifest: {error}"))?;
    manifest.validate(&files, descriptor)?;
    Ok((manifest, files))
}

pub fn validate_tools(
    manifest: &Manifest,
    files: &[(String, Vec<u8>)],
    assets: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let mut supplied = files
        .iter()
        .map(|(path, bytes)| (path.clone(), bytes.as_slice()))
        .collect::<BTreeMap<_, _>>();
    for (path, bytes) in assets {
        supplied.insert(format!("sandbox/{path}"), bytes);
    }
    crate::workers::python_wheel::native::validate_build_tools(
        &manifest.target,
        &manifest.executables,
        &supplied,
    )
}
