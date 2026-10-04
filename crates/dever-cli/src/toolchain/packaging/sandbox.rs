//! Sandbox tools and OS libraries are authenticated by the existing release.

use super::{Artifact, LinkInput, PayloadWriter, read_bounded};
use sha2::{Digest, Sha256};

pub(super) fn assemble(
    writer: &mut PayloadWriter<'_>,
    inputs: Vec<LinkInput>,
    target: crate::toolchain::BuildTarget,
) -> Result<Vec<Artifact>, String> {
    if inputs.len() > 128 {
        return Err("sandbox pack contains too many files".into());
    }
    let mut files = Vec::new();
    let mut remaining = 32 * 1024 * 1024;
    for input in inputs {
        super::runtime_pack::relative_path(&input.path)?;
        let source = writer.source(&input.source, &input.path, &input.sha256)?;
        let bytes = read_bounded(&source, remaining, "sandbox asset")?;
        if format!("{:x}", Sha256::digest(&bytes)) != input.sha256 {
            return Err("sandbox asset changed during packaging".into());
        }
        remaining -= bytes.len() as u64;
        files.push((input.path, bytes));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    dever_sandbox::validate_assets_for_target(&files, target.platform())?;
    // Leaf artifacts avoid a second manifest/signature or extraction protocol.
    let mut artifacts = Vec::new();
    for (path, bytes) in files {
        let destination = format!("sandbox/{}/{path}", target.platform());
        let output = writer.reserve(&destination)?;
        std::fs::write(&output, &bytes)
            .map_err(|error| format!("cannot write sandbox asset: {error}"))?;
        super::set_mode(&output, dever_sandbox::asset_executable(&path))?;
        artifacts.push(Artifact {
            path: destination,
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        });
    }
    Ok(artifacts)
}
