//! The stable Linux entry points share a small, separately checked loader closure.
use serde::Deserialize;

use super::{Artifact, Input, LinkInput, PayloadWriter};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inputs {
    launcher: Input,
    daemon: Input,
    libraries: Vec<LinkInput>,
}

pub(super) fn assemble(
    writer: &mut PayloadWriter<'_>,
    input: Inputs,
) -> Result<Vec<Artifact>, String> {
    if !cfg!(target_os = "linux") {
        return Err("bootstrap authoring currently requires Linux".into());
    }
    let mut artifacts = Vec::new();
    for (name, input) in [("dever", input.launcher), ("deverd", input.daemon)] {
        artifacts.push(writer.copy(
            &input.source,
            &format!("bootstrap/{name}"),
            &input.sha256,
            true,
        )?);
    }
    for library in input.libraries {
        crate::toolchain::core_libraries::library_name(&library.path)?;
        artifacts.push(writer.copy(
            &library.source,
            &format!("bootstrap/lib/{}", library.path),
            &library.sha256,
            false,
        )?);
    }
    #[cfg(target_os = "linux")]
    {
        use sha2::{Digest, Sha256};
        for (path, bytes) in [
            (
                "bootstrap/deverd.service",
                crate::toolchain::bootstrap::SERVICE.as_bytes(),
            ),
            (
                "bootstrap/dever-sandbox",
                dever_sandbox::APPARMOR_PROFILE.as_bytes(),
            ),
        ] {
            let destination = writer.reserve(path)?;
            std::fs::write(&destination, bytes).map_err(|error| error.to_string())?;
            super::set_mode(&destination, false)?;
            artifacts.push(Artifact {
                path: path.into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
            });
        }
        crate::toolchain::bootstrap::validate_payload(writer.staging, &artifacts, true)?;
    }
    Ok(artifacts)
}
