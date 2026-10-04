//! Deterministic ecosystem packs inside the same signed native release.

use std::fs::{self, File};

use flate2::{Compression, GzBuilder};
use pep508_rs::MarkerEnvironment;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    Artifact, LinkInput, PayloadWriter, read_bounded, set_mode, sha256_file, write_metadata,
};
use crate::libs::{Ecosystem, InstalledRegistryPack, RegistryRuntime, RuntimePack};
use crate::workers::{MAX_TREE_BYTES, MAX_TREE_FILES, runtime};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeInputs {
    pip: Option<RuntimeInput>,
    npm: Option<RuntimeInput>,
    go: Option<RuntimeInput>,
}

impl RuntimeInputs {
    pub(super) fn entries(self) -> impl Iterator<Item = (Ecosystem, RuntimeInput)> {
        [
            (Ecosystem::Pip, self.pip),
            (Ecosystem::Npm, self.npm),
            (Ecosystem::Go, self.go),
        ]
        .into_iter()
        .filter_map(|(ecosystem, input)| input.map(|input| (ecosystem, input)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeInput {
    name: String,
    version: String,
    python_markers: Option<MarkerEnvironment>,
    #[serde(default)]
    python_wheel_tags: Vec<String>,
    npm_libc: Option<String>,
    files: Vec<LinkInput>,
}

pub(super) fn assemble(
    writer: &mut PayloadWriter<'_>,
    ecosystem: Ecosystem,
    input: RuntimeInput,
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<Artifact>, String> {
    let target = target.platform().to_owned();
    let mut metadata = InstalledRegistryPack {
        format: "dever-registry-runtime-v1".into(),
        runtime: RegistryRuntime {
            pack: RuntimePack {
                name: input.name,
                version: input.version,
                sha256: "0".repeat(64),
            },
            target: target.clone(),
            python_markers: input.python_markers,
            python_wheel_tags: input.python_wheel_tags,
            npm_libc: input.npm_libc,
        },
    };
    metadata.validate(&ecosystem, &target)?;
    if input.files.len() > MAX_TREE_FILES {
        return Err("managed runtime pack has too many files".into());
    }
    let mut files = Vec::with_capacity(input.files.len());
    let mut remaining = MAX_TREE_BYTES as u64;
    for input in input.files {
        super::runtime_pack::relative_path(&input.path)?;
        // A runtime archive is built from its leaf files, never another completed pack.
        if input.path == "runtime.pack" {
            return Err("runtime pack cannot include another runtime.pack".into());
        }
        let source = writer.source(&input.source, &input.path, &input.sha256)?;
        let bytes = read_bounded(&source, remaining, "runtime payload")?;
        if format!("{:x}", Sha256::digest(&bytes)) != input.sha256 {
            return Err("runtime payload changed during packaging".into());
        }
        remaining -= bytes.len() as u64;
        files.push((input.path, bytes));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let manifest = runtime::validate(&files, &ecosystem, &target)?;
    if manifest.python_wheel_tags != metadata.runtime.python_wheel_tags
        || manifest.python_markers != metadata.runtime.python_markers
    {
        return Err(
            "runtime pack Python metadata differs from its signed registry descriptor".into(),
        );
    }
    let prefix = format!("runtime/{}/{target}", ecosystem.as_str());
    let archive_name = format!("{prefix}/runtime.pack");
    let archive_path = writer.reserve(&archive_name)?;
    let output = File::create(&archive_path)
        .map_err(|error| format!("cannot create runtime pack: {error}"))?;
    let encoder = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(output, Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (path, bytes) in &files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(if manifest.executable(path) {
            0o755
        } else {
            0o644
        });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive
            .append_data(&mut header, path, bytes.as_slice())
            .map_err(|error| format!("cannot write runtime pack entry: {error}"))?;
    }
    archive
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(|error| format!("cannot finish runtime pack: {error}"))?;
    set_mode(&archive_path, false)?;
    let size = fs::metadata(&archive_path)
        .map_err(|error| error.to_string())?
        .len();
    if size > MAX_TREE_BYTES as u64 {
        return Err("compressed runtime pack exceeds 256 MiB".into());
    }
    metadata.runtime.pack.sha256 = sha256_file(&archive_path)?;
    let pack = Artifact {
        path: archive_name,
        bytes: size,
        sha256: metadata.runtime.pack.sha256.clone(),
    };
    let metadata_name = format!("{prefix}/manifest.json");
    let metadata_path = writer.reserve(&metadata_name)?;
    let bytes = write_metadata(&metadata_path, &metadata)?;
    Ok(vec![
        pack,
        Artifact {
            path: metadata_name,
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        },
    ])
}
