//! Bounded single-frame Zstandard and exact signed USTAR extraction.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use super::extensions::{ARTIFACT_LIMIT, PAYLOAD_LIMIT, validate_artifacts};
use super::release::{Artifact, set_shared_file_permissions, sha256_file};

pub(super) fn extract(
    reader: impl Read,
    root: &Path,
    artifacts: &[Artifact],
) -> Result<(), String> {
    validate_artifacts(artifacts, &mut BTreeSet::new())?;
    let mut input = Bounded {
        input: reader,
        remaining: PAYLOAD_LIMIT,
    };
    let mut header = [0_u8; 5];
    input
        .read_exact(&mut header)
        .map_err(|error| format!("cannot read Zstandard header: {error}"))?;
    if header[..4] != [0x28, 0xb5, 0x2f, 0xfd] || header[4] & 4 == 0 {
        return Err("release requires a Zstandard frame with a content checksum".into());
    }
    let mut decoder = zstd::stream::read::Decoder::with_buffer(BufReader::new(
        io::Cursor::new(header).chain(input),
    ))
    .map_err(|error| format!("cannot initialize Zstandard decoder: {error}"))?
    .single_frame();
    decoder
        .window_log_max(27)
        .map_err(|error| format!("cannot bound Zstandard window: {error}"))?;
    unpack(&mut decoder, root, artifacts)?;
    // unpack drains the decoder through its checksum, including errors after TAR EOF.
    let mut input = decoder.finish();
    if !input
        .fill_buf()
        .map_err(|error| format!("cannot finish release frame: {error}"))?
        .is_empty()
    {
        return Err("release contains data after its single Zstandard frame".into());
    }
    Ok(())
}

pub(super) fn unpack(reader: impl Read, root: &Path, artifacts: &[Artifact]) -> Result<(), String> {
    validate_artifacts(artifacts, &mut BTreeSet::new())?;
    let expected: BTreeMap<_, _> = artifacts
        .iter()
        .map(|artifact| (artifact.path.as_str(), artifact))
        .collect();
    let mut seen = BTreeSet::new();
    let mut archive = Bounded {
        input: reader,
        remaining: PAYLOAD_LIMIT + ARTIFACT_LIMIT as u64 * 1024 + 10240,
    };
    while let Some(header) = read_header(&mut archive)? {
        let name = std::str::from_utf8(&header.path_bytes())
            .map_err(|_| "release archive path is not UTF-8")?
            .to_owned();
        let artifact = expected
            .get(name.as_str())
            .ok_or("release archive contains an unsigned file")?;
        if !seen.insert(name) || header.size().map_err(|error| error.to_string())? != artifact.bytes
        {
            return Err("release archive contains a duplicate or wrong-sized file".into());
        }
        extract_file(&mut archive, root, artifact)?;
    }
    if seen.len() != expected.len() {
        return Err("release archive is missing signed artifacts".into());
    }
    verify_trailer(&mut archive)
}

fn read_header(reader: &mut impl Read) -> Result<Option<tar::Header>, String> {
    let mut header = tar::Header::new_ustar();
    reader
        .read_exact(header.as_mut_bytes())
        .map_err(|error| format!("incomplete release TAR header: {error}"))?;
    if header.as_bytes().iter().all(|byte| *byte == 0) {
        return Ok(None);
    }
    let checksum = header.as_bytes()[..148]
        .iter()
        .chain(&header.as_bytes()[156..])
        .map(|byte| u32::from(*byte))
        .sum::<u32>()
        + 8 * u32::from(b' ');
    if header.cksum().map_err(|error| error.to_string())? != checksum {
        return Err("release TAR header checksum is invalid".into());
    }
    if !header.entry_type().is_file() || header.as_ustar().is_none() {
        return Err("release archives may contain only USTAR regular files".into());
    }
    Ok(Some(header))
}

fn extract_file(reader: &mut impl Read, root: &Path, artifact: &Artifact) -> Result<(), String> {
    let path = root.join(&artifact.path);
    let mut parent = root.to_path_buf();
    for part in Path::new(&artifact.path)
        .parent()
        .into_iter()
        .flat_map(Path::components)
    {
        parent.push(part);
        match fs::create_dir(&parent) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&parent).map_err(|error| error.to_string())?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("release artifact parent is not a real directory".into());
                }
            }
            Err(error) => return Err(format!("cannot create release artifact directory: {error}")),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o755))
                .map_err(|error| error.to_string())?;
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("cannot create release artifact: {error}"))?;
    if io::copy(&mut reader.by_ref().take(artifact.bytes), &mut file)
        .map_err(|error| format!("cannot extract release artifact: {error}"))?
        != artifact.bytes
    {
        return Err("release archive contains a truncated file".into());
    }
    let padding_length = ((512 - artifact.bytes % 512) % 512) as usize;
    let mut padding = [0; 512];
    reader
        .read_exact(&mut padding[..padding_length])
        .map_err(|error| format!("incomplete TAR entry padding: {error}"))?;
    if padding[..padding_length].iter().any(|byte| *byte != 0) {
        return Err("release TAR entry padding must be zero".into());
    }
    if sha256_file(&path)? != artifact.sha256 {
        return Err("release artifact failed integrity verification".into());
    }
    // First-install invokes the extracted signed bootstrap before machine install.
    // Archive mode bits are untrusted; executable roles come from fixed paths.
    let executable = matches!(
        artifact.path.as_str(),
        "dever-core" | "bootstrap/dever" | "bootstrap/deverd"
    );
    set_shared_file_permissions(&path, executable)?;
    file.sync_all().map_err(|error| error.to_string())
}

fn verify_trailer(reader: &mut impl Read) -> Result<(), String> {
    let mut tail_bytes = 0_usize;
    let mut tail = [0_u8; 4096];
    loop {
        let length = reader
            .read(&mut tail)
            .map_err(|error| format!("invalid release archive trailer: {error}"))?;
        if length == 0 {
            break;
        }
        tail_bytes += length;
        if tail_bytes > 10240 || tail[..length].iter().any(|byte| *byte != 0) {
            return Err("release archive contains an invalid TAR trailer".into());
        }
    }
    // The first zero header was consumed by read_header; require the second.
    if tail_bytes < 512 || !tail_bytes.is_multiple_of(512) {
        return Err("release archive is missing its complete TAR terminator".into());
    }
    Ok(())
}

struct Bounded<R> {
    input: R,
    remaining: u64,
}

impl<R: Read> Read for Bounded<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0];
            return match self.input.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "release byte limit exceeded",
                )),
            };
        }
        let limit = (self.remaining.min(buffer.len() as u64)) as usize;
        let length = self.input.read(&mut buffer[..limit])?;
        self.remaining -= length as u64;
        Ok(length)
    }
}
