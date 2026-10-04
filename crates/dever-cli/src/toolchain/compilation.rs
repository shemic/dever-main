//! Bounded source-only protocol shared by callers, the service and its worker.

use super::BuildTarget;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use dever_core::native::EmbeddedResource;
use dever_core::source::SourceMap;
use dever_runtime::config::CompilationBindings;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

// Base64 plus source/metadata must fit while embedding a complete interpreter.
pub const MAX_COMPILE_BYTES: usize = 384 * 1024 * 1024;
const MAX_RESOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_RESOURCE_TOTAL: usize = 256 * 1024 * 1024;
pub const WORKER_REQUEST_FILE: &str = "request.json";
pub const WORKER_OUTPUT_FILE: &str = "program";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CompileKind {
    Application,
    TestSuite,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompileRequest {
    pub version: String,
    pub kind: CompileKind,
    pub target: BuildTarget,
    pub sources: Vec<CompilationSource>,
    pub bindings: Option<CompilationBindings>,
    pub resources: Vec<CompilationResource>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationSource {
    pub logical_path: String,
    pub is_test: bool,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationResource {
    pub path: String,
    #[serde(with = "resource_bytes")]
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub executable: bool,
}

impl CompileRequest {
    pub fn new(
        version: String,
        kind: CompileKind,
        sources: &SourceMap,
        bindings: Option<CompilationBindings>,
        resources: &[EmbeddedResource],
        target: BuildTarget,
    ) -> Result<Self, String> {
        let request = Self {
            version,
            kind,
            target,
            sources: sources
                .compilation_sources()?
                .into_iter()
                .map(|source| CompilationSource {
                    logical_path: source.logical_path,
                    is_test: source.is_test,
                    text: source.text,
                })
                .collect(),
            bindings,
            resources: resources
                .iter()
                .map(|resource| CompilationResource {
                    path: resource.path.clone(),
                    bytes: resource.bytes.clone(),
                    sha256: resource.sha256.clone(),
                    executable: resource.executable,
                })
                .collect(),
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), String> {
        super::Version::parse(&self.version)?;
        if self.kind == CompileKind::TestSuite && self.target != BuildTarget::host()? {
            return Err("test compilation requires the compiler host target".into());
        }
        if self.sources.is_empty()
            || self.sources.len() > 4096
            || self
                .sources
                .iter()
                .map(|source| source.text.len())
                .sum::<usize>()
                > 16 * 1024 * 1024
            || self
                .sources
                .iter()
                .any(|source| source.logical_path.len() > 4096)
            || !self.sources.iter().any(|source| !source.is_test)
        {
            return Err("compilation requires application sources, at most 4096 files and 16 MiB of source text".into());
        }
        self.source_map()?;
        match self.kind {
            CompileKind::Application => {
                if self.sources.iter().any(|source| source.is_test) {
                    return Err("application compilation cannot contain test sources".into());
                }
                self.bindings
                    .as_ref()
                    .ok_or("application compilation requires deployment bindings")?
                    .validate()?;
            }
            CompileKind::TestSuite if self.bindings.is_some() || !self.resources.is_empty() => {
                return Err("test compilation derives case-local bindings and cannot contain deployment bindings or external resources".into());
            }
            CompileKind::TestSuite => {}
        }
        if self.resources.len() > 4096
            || self
                .resources
                .iter()
                .map(|resource| resource.bytes.len())
                .sum::<usize>()
                > MAX_RESOURCE_TOTAL
        {
            return Err("compilation resources exceed 4096 files or 256 MiB".into());
        }
        let mut paths = BTreeSet::new();
        for resource in &self.resources {
            relative_path(&resource.path)?;
            if !paths.insert(&resource.path) {
                return Err("duplicate compilation resource path".into());
            }
            if resource.bytes.len() > MAX_RESOURCE_BYTES
                || format!("{:x}", Sha256::digest(&resource.bytes)) != resource.sha256
            {
                return Err(format!(
                    "compilation resource '{}' exceeds 128 MiB or has an invalid digest",
                    resource.path
                ));
            }
        }
        Ok(())
    }

    pub fn source_map(&self) -> Result<SourceMap, String> {
        SourceMap::from_compilation_sources(
            &self
                .sources
                .iter()
                .map(|source| dever_core::source::CompilationSource {
                    logical_path: source.logical_path.clone(),
                    is_test: source.is_test,
                    text: source.text.clone(),
                })
                .collect::<Vec<_>>(),
        )
    }

    pub fn embedded_resources(&self) -> Vec<EmbeddedResource> {
        self.resources
            .iter()
            .map(|resource| EmbeddedResource {
                path: resource.path.clone(),
                bytes: resource.bytes.clone(),
                sha256: resource.sha256.clone(),
                executable: resource.executable,
            })
            .collect()
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        #[derive(Serialize)]
        struct Canonical<'a> {
            version: &'a str,
            kind: CompileKind,
            target: BuildTarget,
            sources: Vec<&'a CompilationSource>,
            bindings: &'a Option<CompilationBindings>,
            resources: Vec<&'a CompilationResource>,
        }
        // Sort references: interpreter payloads must not be cloned to order JSON.
        let mut sources = self.sources.iter().collect::<Vec<_>>();
        sources.sort_by(|left, right| {
            (left.is_test, &left.logical_path).cmp(&(right.is_test, &right.logical_path))
        });
        let mut resources = self.resources.iter().collect::<Vec<_>>();
        resources.sort_by(|left, right| left.path.cmp(&right.path));
        let canonical = Canonical {
            version: &self.version,
            kind: self.kind,
            target: self.target,
            sources,
            bindings: &self.bindings,
            resources,
        };
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|error| format!("cannot encode compile request: {error}"))?;
        if bytes.len() > MAX_COMPILE_BYTES {
            return Err("compile request exceeds 384 MiB".into());
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_COMPILE_BYTES {
            return Err("compile request exceeds 384 MiB".into());
        }
        let request: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid compile request: {error}"))?;
        // Requiring one representation rejects duplicate map keys as well as
        // ambiguous ordering/escapes before requests acquire a cache identity.
        if request.encode()? != bytes {
            return Err("compile request must use canonical JSON without duplicate keys".into());
        }
        Ok(request)
    }
}

fn relative_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\\', ':', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("compilation resources require normalized relative paths".into());
    }
    Ok(())
}

/// Workers accept inputs only inside the service owner's private compile stage.
/// This prevents the hidden entry from becoming a client-side managed fallback.
pub fn worker_directory(layout: &super::Layout) -> Result<std::path::PathBuf, String> {
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::MetadataExt;

        let directory = std::env::current_dir()
            .map_err(|error| format!("cannot locate compiler worker directory: {error}"))?;
        let staging = layout.native_cache().join("staging");
        if directory.parent() != Some(staging.as_path())
            || !directory
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.strip_prefix("compile-")
                        .is_some_and(|suffix| !suffix.is_empty())
                })
        {
            return Err(
                "compile worker requires a service-owned native cache staging directory".into(),
            );
        }
        let owner = rustix::process::geteuid().as_raw();
        for (path, private) in [
            (layout.state(), false),
            (layout.cache(), false),
            (layout.native_cache(), false),
            (staging, true),
            (directory.clone(), true),
        ] {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot inspect compiler worker directory: {error}"))?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.uid() != owner
                || metadata.mode() & 0o022 != 0
                || private && metadata.mode() & 0o777 != 0o700
            {
                return Err("compiler worker directories must be real and service-owned without group/other writes; staging must be private (0700)".into());
            }
        }
        let request = fs::symlink_metadata(directory.join(WORKER_REQUEST_FILE))
            .map_err(|error| format!("cannot inspect compiler worker request: {error}"))?;
        if request.file_type().is_symlink()
            || !request.is_file()
            || request.uid() != owner
            || request.mode() & 0o777 != 0o600
        {
            return Err(
                "compiler worker request must be a service-owned regular file with mode 0600"
                    .into(),
            );
        }
        Ok(directory)
    }
    #[cfg(not(unix))]
    {
        let _ = layout;
        Err("trusted compiler worker directories are unavailable on this platform".into())
    }
}

mod resource_bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        if encoded.len() > MAX_RESOURCE_BYTES * 4 / 3 + 4 {
            return Err(serde::de::Error::custom(
                "compilation resource exceeds 128 MiB",
            ));
        }
        STANDARD.decode(encoded).map_err(serde::de::Error::custom)
    }
}
