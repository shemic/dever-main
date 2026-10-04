//! Assemble source-build tools against already prepared runtime identities.

use flate2::{Compression, GzBuilder};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};

use super::{Artifact, LinkInput, PayloadWriter, read_bounded, set_mode, write_metadata};
use crate::libs::build::pack::{self, Descriptor, Manifest};
use crate::libs::{Ecosystem, InstalledRegistryPack};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inputs {
    pip: Option<Input>,
    npm: Option<Input>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    executables: Vec<String>,
    files: Vec<LinkInput>,
}

impl Inputs {
    pub(super) fn entries(self) -> impl Iterator<Item = (Ecosystem, Input)> {
        [(Ecosystem::Pip, self.pip), (Ecosystem::Npm, self.npm)]
            .into_iter()
            .filter_map(|(ecosystem, input)| input.map(|input| (ecosystem, input)))
    }
}

pub(super) fn assemble(
    writer: &mut PayloadWriter<'_>,
    ecosystem: Ecosystem,
    input: Input,
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<Artifact>, String> {
    let target = target.platform().to_owned();
    let runtime = |ecosystem: &Ecosystem| -> Result<InstalledRegistryPack, String> {
        let path = writer.staging.join(format!(
            "runtime/{}/{target}/manifest.json",
            ecosystem.as_str()
        ));
        let manifest: InstalledRegistryPack = serde_json::from_slice(&read_bounded(
            &path,
            1024 * 1024,
            "build runtime descriptor",
        )?)
        .map_err(|error| format!("invalid build runtime descriptor: {error}"))?;
        manifest.validate(ecosystem, &target)?;
        Ok(manifest)
    };
    let runtime = runtime(&ecosystem)?;
    let python = if ecosystem == Ecosystem::Npm {
        let path = writer
            .staging
            .join(format!("runtime/pip/{target}/manifest.json"));
        let manifest: InstalledRegistryPack = serde_json::from_slice(&read_bounded(
            &path,
            1024 * 1024,
            "auxiliary Python runtime descriptor",
        )?)
        .map_err(|error| format!("invalid auxiliary Python runtime: {error}"))?;
        manifest.validate(&Ecosystem::Pip, &target)?;
        Some(manifest)
    } else {
        None
    };
    let mut descriptor = Descriptor {
        format: "dever-registry-build-v1".into(),
        ecosystem: ecosystem.clone(),
        target: target.clone(),
        runtime_sha256: runtime.runtime.pack.sha256.clone(),
        python_runtime_sha256: python
            .as_ref()
            .map(|runtime| runtime.runtime.pack.sha256.clone()),
        sha256: "0".repeat(64),
    };
    descriptor.validate(
        &runtime.runtime,
        python.as_ref().map(|manifest| &manifest.runtime),
    )?;
    let manifest = Manifest {
        format: "dever-lib-build-v1".into(),
        ecosystem: ecosystem.clone(),
        target: target.clone(),
        runtime_sha256: descriptor.runtime_sha256.clone(),
        python_runtime_sha256: descriptor.python_runtime_sha256.clone(),
        executables: input.executables,
    };
    if input.files.len() >= pack::MAX_FILES {
        return Err("build pack has too many leaves".into());
    }
    let metadata = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    let mut remaining = pack::MAX_EXPANDED_BYTES
        .checked_sub(metadata.len())
        .ok_or("build manifest is too large")? as u64;
    let mut files = vec![("dever-build.json".into(), metadata)];
    for input in input.files {
        let path = writer.source(&input.source, &input.path, &input.sha256)?;
        let bytes = read_bounded(&path, remaining, "build tool leaf")?;
        if format!("{:x}", Sha256::digest(&bytes)) != input.sha256 {
            return Err("build tool changed during packaging".into());
        }
        remaining -= bytes.len() as u64;
        files.push((input.path, bytes));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    manifest.validate(&files, &descriptor)?;
    let mut assets = Vec::new();
    for directory in ["bin", "lib"] {
        for entry in fs::read_dir(writer.staging.join(format!("sandbox/{target}/{directory}")))
            .map_err(|error| format!("build pack requires prepared sandbox assets: {error}"))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "sandbox asset name is not UTF-8")?;
            assets.push((
                format!("{directory}/{name}"),
                read_bounded(&entry.path(), 32 * 1024 * 1024, "sandbox build asset")?,
            ));
        }
    }
    dever_sandbox::validate_assets_for_target(&assets, &target)?;
    pack::validate_tools(&manifest, &files, &assets)?;
    let prefix = format!("build/{}/{target}", ecosystem.as_str());
    let archive_name = format!("{prefix}/build.pack");
    let path = writer.reserve(&archive_name)?;
    let encoder = GzBuilder::new().mtime(0).operating_system(255).write(
        File::create(&path).map_err(|error| error.to_string())?,
        Compression::default(),
    );
    let mut archive = tar::Builder::new(encoder);
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(if manifest.executables.contains(&path) {
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
            .map_err(|error| error.to_string())?;
    }
    archive
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(|error| error.to_string())?;
    set_mode(&path, false)?;
    let bytes = fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .len();
    if bytes > pack::MAX_PACK_BYTES as u64 {
        return Err("compressed build pack exceeds 256 MiB".into());
    }
    descriptor.sha256 = super::sha256_file(&path)?;
    let descriptor_name = format!("{prefix}/manifest.json");
    let metadata = write_metadata(&writer.reserve(&descriptor_name)?, &descriptor)?;
    Ok(vec![
        Artifact {
            path: archive_name,
            bytes,
            sha256: descriptor.sha256,
        },
        Artifact {
            path: descriptor_name,
            bytes: metadata.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&metadata)),
        },
    ])
}
