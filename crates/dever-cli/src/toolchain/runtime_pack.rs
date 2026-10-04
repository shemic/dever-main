//! Compiler-relative, explicitly supplied native link inputs. This is not a
//! package resolver or a substitute for the managed compiler service.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use dever_runtime::config::RuntimeProfile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{BuildTarget, sha256_file};

const MANIFEST_LIMIT: u64 = 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) format: String,
    pub(super) compiler_version: String,
    pub(super) abi: u32,
    pub(super) platform: String,
    pub(super) target: String,
    pub(super) profiles: Profiles,
    pub(super) files: Vec<RuntimeArtifact>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profiles {
    pub(super) base: Option<Profile>,
    pub(super) sqlite: Option<Profile>,
    pub(super) postgres: Option<Profile>,
    pub(super) both: Option<Profile>,
}

impl Profiles {
    fn all(&self) -> [(&'static str, Option<&Profile>); 4] {
        [
            ("base", self.base.as_ref()),
            ("sqlite", self.sqlite.as_ref()),
            ("postgres", self.postgres.as_ref()),
            ("both", self.both.as_ref()),
        ]
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    pub(super) start: Vec<String>,
    pub(super) runtime: String,
    pub(super) libraries: Vec<String>,
    pub(super) end: Vec<String>,
}

impl Profile {
    fn inputs(&self) -> impl Iterator<Item = &str> {
        self.start
            .iter()
            .chain(std::iter::once(&self.runtime))
            .chain(&self.libraries)
            .chain(&self.end)
            .map(String::as_str)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeArtifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

pub struct RuntimePack {
    root: PathBuf,
    pub target: BuildTarget,
    pub identity: String,
    profile: Profile,
    files: BTreeMap<String, RuntimeArtifact>,
}

pub struct LinkInputs {
    pub start: Vec<PathBuf>,
    pub libraries: Vec<PathBuf>,
    pub end: Vec<PathBuf>,
}

struct LoadedManifest {
    root: PathBuf,
    target: BuildTarget,
    bytes: Vec<u8>,
    profiles: Profiles,
    files: BTreeMap<String, RuntimeArtifact>,
}

fn load_manifest(
    directory: &Path,
    compiler_version: &str,
    selected: BuildTarget,
) -> Result<LoadedManifest, String> {
    let target = selected.triple();
    let platform = selected.platform();
    let root = checked_path(directory, &format!("runtime/{platform}"), false)?;
    let path = checked_path(&root, "manifest.json", true)?;
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .and_then(|file| file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read runtime pack manifest: {error}"))?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err("runtime pack manifest exceeds 1 MiB".into());
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid runtime pack manifest: {error}"))?;
    if manifest.format != "dever-native-runtime-v1"
        || manifest.compiler_version != compiler_version
        // A v1 manifest describes v1 runtime symbols. Even bridge metadata
        // references would link the native SDK into the launcher and daemon.
        || manifest.abi != 1
        || manifest.platform != platform
        || manifest.target != target
    {
        return Err(
            "runtime pack format, compiler version, ABI or target does not match this compiler"
                .into(),
        );
    }
    let files = validate_files(manifest.files)?;
    validate_profiles(&manifest.profiles, &files)?;
    Ok(LoadedManifest {
        root,
        target: selected,
        bytes,
        profiles: manifest.profiles,
        files,
    })
}

/// Return the entire runtime closure that must be covered by the signed release.
pub fn validate(
    directory: &Path,
    compiler_version: &str,
    target: BuildTarget,
) -> Result<Vec<RuntimeArtifact>, String> {
    let manifest = load_manifest(directory, compiler_version, target)?;
    let prefix = format!("runtime/{}", target.platform());
    let mut artifacts = vec![RuntimeArtifact {
        path: format!("{prefix}/manifest.json"),
        bytes: manifest.bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&manifest.bytes)),
    }];
    for (path, artifact) in manifest.files {
        verify_file(
            &checked_path(&manifest.root, &path, true)?,
            &artifact,
            target,
        )?;
        artifacts.push(RuntimeArtifact {
            path: format!("{prefix}/{path}"),
            ..artifact
        });
    }
    Ok(artifacts)
}

pub(super) fn host_target() -> Result<&'static str, String> {
    Ok(BuildTarget::host()?.triple())
}

impl RuntimePack {
    pub fn load(
        directory: &Path,
        profile: RuntimeProfile,
        selected: BuildTarget,
    ) -> Result<Self, String> {
        let LoadedManifest {
            root,
            target,
            bytes,
            profiles,
            files,
        } = load_manifest(directory, env!("CARGO_PKG_VERSION"), selected)?;
        let name = match (profile.sqlite, profile.postgres) {
            (false, false) => "base",
            (true, false) => "sqlite",
            (false, true) => "postgres",
            (true, true) => "both",
        };
        let profile = profiles
            .all()
            .into_iter()
            .find_map(|(key, value)| (key == name).then_some(value).flatten())
            .ok_or_else(|| format!("runtime pack is missing the required '{name}' profile"))?
            .clone();
        // Validate even on a cache hit: a corrupt installed pack is never
        // silently replaced with an old cached executable.
        for input in profile.inputs() {
            verify_file(&checked_path(&root, input, true)?, &files[input], selected)?;
        }
        Ok(Self {
            root,
            target,
            identity: format!("{name}:{:x}", Sha256::digest(bytes)),
            profile,
            files,
        })
    }

    pub fn snapshot(&self, directory: &Path) -> Result<LinkInputs, String> {
        let destination = directory.join("runtime");
        fs::create_dir(&destination)
            .map_err(|error| format!("cannot create runtime input snapshot: {error}"))?;
        let mut inputs = Vec::new();
        for (index, input) in self.profile.inputs().enumerate() {
            let source = checked_path(&self.root, input, true)?;
            let extension = Path::new(input)
                .extension()
                .expect("validated input extension");
            let output = destination
                .join(index.to_string())
                .with_extension(extension);
            fs::copy(&source, &output)
                .map_err(|error| format!("cannot snapshot runtime input '{input}': {error}"))?;
            verify_file(&output, &self.files[input], self.target)?;
            inputs.push(output);
        }
        let start = inputs.drain(..self.profile.start.len()).collect();
        let end = inputs.split_off(1 + self.profile.libraries.len());
        Ok(LinkInputs {
            start,
            libraries: inputs,
            end,
        })
    }
}

fn validate_files(
    artifacts: Vec<RuntimeArtifact>,
) -> Result<BTreeMap<String, RuntimeArtifact>, String> {
    if artifacts.is_empty() || artifacts.len() > 4096 {
        return Err("runtime pack must declare 1..4096 files".into());
    }
    let mut files = BTreeMap::new();
    for artifact in artifacts {
        relative_path(&artifact.path)?;
        if artifact.bytes == 0
            || artifact.bytes > 1024 * 1024 * 1024
            || artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "invalid runtime file digest or size for '{}'",
                artifact.path
            ));
        }
        if files.insert(artifact.path.clone(), artifact).is_some() {
            return Err("duplicate runtime file path".into());
        }
    }
    Ok(files)
}

fn validate_profiles(
    profiles: &Profiles,
    files: &BTreeMap<String, RuntimeArtifact>,
) -> Result<(), String> {
    let mut referenced = BTreeSet::new();
    for (_, profile) in profiles.all() {
        let Some(profile) = profile else { continue };
        if profile.start.is_empty() || profile.end.is_empty() || profile.libraries.is_empty() {
            return Err("runtime profile requires CRT start/end and system libraries".into());
        }
        let mut unique = BTreeSet::new();
        for input in profile.inputs() {
            if !files.contains_key(input) || !unique.insert(input) {
                return Err(format!(
                    "runtime profile has an undeclared or duplicate input '{input}'"
                ));
            }
            referenced.insert(input);
        }
        for (paths, extension) in [
            (profile.start.as_slice(), "o"),
            (profile.end.as_slice(), "o"),
            (profile.libraries.as_slice(), "a"),
            (std::slice::from_ref(&profile.runtime), "a"),
        ] {
            if paths.iter().any(|path| {
                Path::new(path).extension().and_then(|value| value.to_str()) != Some(extension)
            }) {
                return Err(
                    "runtime profiles accept only explicit object files and static archives".into(),
                );
            }
        }
    }
    if referenced.len() != files.len() {
        return Err("runtime pack contains unreferenced files".into());
    }
    Ok(())
}

pub(super) fn relative_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\\', ':', '\0'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("runtime pack paths must be normalized relative paths".into());
    }
    Ok(())
}

pub(super) fn checked_path(root: &Path, relative: &str, file: bool) -> Result<PathBuf, String> {
    relative_path(relative)?;
    let mut path = root.to_owned();
    let mut segments = relative.split('/').peekable();
    while let Some(segment) = segments.next() {
        path.push(segment);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!("cannot inspect runtime pack '{}': {error}", path.display())
        })?;
        let expect_file = file && segments.peek().is_none();
        if metadata.file_type().is_symlink()
            || if expect_file {
                !metadata.is_file()
            } else {
                !metadata.is_dir()
            }
        {
            return Err(format!(
                "runtime pack '{}' must be a real {}",
                path.display(),
                if expect_file { "file" } else { "directory" }
            ));
        }
    }
    Ok(path)
}

fn verify_file(path: &Path, artifact: &RuntimeArtifact, target: BuildTarget) -> Result<(), String> {
    let length = fs::metadata(path)
        .map_err(|error| format!("cannot inspect runtime file '{}': {error}", path.display()))?
        .len();
    if length != artifact.bytes || sha256_file(path)? != artifact.sha256 {
        return Err(format!(
            "runtime file '{}' failed size or SHA-256 verification",
            artifact.path
        ));
    }
    let mut prefix = [0_u8; 8];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut prefix))
        .map_err(|error| format!("cannot read runtime input header: {error}"))?;
    let valid = match Path::new(&artifact.path)
        .extension()
        .and_then(|value| value.to_str())
    {
        Some("a") => &prefix == b"!<arch>\n",
        Some("o") => &prefix[..4] == b"\x7fELF",
        _ => false,
    };
    if !valid {
        return Err(format!(
            "runtime input '{}' is not an ELF object or self-contained archive; linker scripts and thin archives are forbidden",
            artifact.path
        ));
    }
    validate_target(path, length, target).map_err(|error| {
        format!(
            "runtime input '{}' failed target verification: {error}",
            artifact.path
        )
    })?;
    Ok(())
}

fn validate_target(path: &Path, length: u64, target: BuildTarget) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    if path.extension().is_some_and(|extension| extension == "o") {
        return validate_object(&mut file, length, target);
    }
    // Walk archive members without loading large runtime archives into memory.
    let mut offset = 8_u64;
    while offset < length {
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| error.to_string())?;
        let mut header = [0; 60];
        file.read_exact(&mut header)
            .map_err(|_| "truncated runtime archive member")?;
        if &header[58..] != b"`\n" {
            return Err("invalid runtime archive member header".into());
        }
        let size = std::str::from_utf8(&header[48..58])
            .ok()
            .and_then(|size| size.trim().parse::<u64>().ok())
            .ok_or("invalid runtime archive member size")?;
        let start = offset
            .checked_add(60)
            .ok_or("runtime archive size overflow")?;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= length)
            .ok_or("truncated runtime archive member")?;
        let name = std::str::from_utf8(&header[..16])
            .map_err(|_| "invalid runtime archive member name")?
            .trim();
        if !matches!(
            name,
            "/" | "//" | "/SYM64/" | "__.SYMDEF" | "__.SYMDEF SORTED"
        ) {
            let name_bytes = match name.strip_prefix("#1/") {
                Some(bytes) => bytes
                    .parse::<u64>()
                    .map_err(|_| "invalid archive extended name")?,
                None => 0,
            };
            let object_size = size
                .checked_sub(name_bytes)
                .ok_or("invalid archive extended name size")?;
            file.seek(SeekFrom::Start(start + name_bytes))
                .map_err(|error| error.to_string())?;
            validate_object(&mut file, object_size, target)?;
        }
        offset = end
            .checked_add(size % 2)
            .filter(|end| *end <= length)
            .ok_or("truncated runtime archive padding")?;
    }
    Ok(())
}

fn validate_object(file: &mut fs::File, length: u64, target: BuildTarget) -> Result<(), String> {
    let mut header = [0; 20];
    if length < header.len() as u64 {
        return Err("runtime input requires a complete target ELF object header".into());
    }
    file.read_exact(&mut header)
        .map_err(|error| format!("cannot read runtime object: {error}"))?;
    let machine = match target {
        BuildTarget::LinuxX86_64 => 62,
        BuildTarget::LinuxAarch64 => 183,
    };
    if &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || header[6] != 1
        || u16::from_le_bytes([header[16], header[17]]) != 1
        || u16::from_le_bytes([header[18], header[19]]) != machine
    {
        return Err(format!(
            "runtime input is not a relocatable ELF object for {}",
            target.platform()
        ));
    }
    Ok(())
}
