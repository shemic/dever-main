//! Private authoring of signed native releases from explicitly prepared inputs.
//! Compilation and tool discovery do not belong to this entry point.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ring::signature::Ed25519KeyPair;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::runtime_pack::{self, Manifest, Profile, Profiles, RuntimeArtifact};
use super::{
    Artifact, BuildTarget, Extension, ExtensionKind, ReleaseManifest, Version, sha256_file,
    validate_catalog,
};

static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

mod bootstrap;
mod builds;
mod ecosystems;
mod sandbox;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    format: String,
    version: String,
    target: String,
    signing_key: String,
    core: Input,
    core_libraries: Vec<LinkInput>,
    skill: Vec<LinkInput>,
    #[serde(default)]
    bootstrap: Option<bootstrap::Inputs>,
    profiles: ArchiveInputs,
    start: Vec<LinkInput>,
    libraries: Vec<LinkInput>,
    end: Vec<LinkInput>,
    #[serde(default)]
    native_targets: BTreeMap<BuildTarget, NativeInputs>,
    #[serde(default)]
    runtimes: ecosystems::RuntimeInputs,
    #[serde(default)]
    sandbox: Option<Vec<LinkInput>>,
    #[serde(default)]
    builds: builds::Inputs,
    #[serde(default)]
    ecosystem_targets: BTreeMap<BuildTarget, ecosystems::RuntimeInputs>,
    #[serde(default)]
    build_targets: BTreeMap<BuildTarget, builds::Inputs>,
    #[serde(default)]
    sandbox_targets: BTreeMap<BuildTarget, Vec<LinkInput>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeInputs {
    profiles: ArchiveInputs,
    start: Vec<LinkInput>,
    libraries: Vec<LinkInput>,
    end: Vec<LinkInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    source: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkInput {
    source: String,
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveInputs {
    base: Input,
    sqlite: Input,
    postgres: Input,
    both: Input,
}

struct SigningKey {
    pair: Ed25519KeyPair,
    sha256: String,
}

struct PayloadWriter<'a> {
    root: &'a Path,
    staging: &'a Path,
    key: &'a SigningKey,
    destinations: BTreeSet<String>,
}

/// Create a new release directory. Inputs come only from author-root/config/setting.json.
pub fn create(author_root: &Path, output: &Path) -> Result<ReleaseManifest, String> {
    let target = runtime_pack::host_target()?;
    let metadata = fs::symlink_metadata(author_root)
        .map_err(|error| format!("cannot inspect author root: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("author root must be a real directory".into());
    }
    let root = fs::canonicalize(author_root)
        .map_err(|error| format!("cannot resolve author root: {error}"))?;
    let config = runtime_pack::checked_path(&root, "config/setting.json", true)?;
    let bytes = read_bounded(&config, 16 * 1024 * 1024, "author configuration")?;
    // Do not echo configuration values: they include the private signing-key location.
    let settings: Settings = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "invalid author configuration at line {}, column {}",
            error.line(),
            error.column()
        )
    })?;
    if settings.format != "dever-native-release-input-v1" || settings.target != target {
        return Err("unsupported native release input format or target".into());
    }
    let version = Version::parse(&settings.version)?;
    let key = read_signing_key(&root, &settings.signing_key)?;
    let output = output_path(output)?;
    let staging = create_staging(output.parent().expect("resolved output parent"))?;
    let result = assemble(&root, &staging, settings, version, &key)
        .and_then(|manifest| publish(&staging, &output).map(|()| manifest));
    if result.is_err()
        && let Err(error) = fs::remove_dir_all(&staging)
    {
        return Err(format!(
            "{}; cannot remove owned release staging: {error}",
            result.expect_err("failed release creation")
        ));
    }
    result
}

fn assemble(
    root: &Path,
    staging: &Path,
    settings: Settings,
    version: Version,
    key: &SigningKey,
) -> Result<ReleaseManifest, String> {
    let host = BuildTarget::host()?;
    let mut writer = PayloadWriter {
        root,
        staging,
        key,
        destinations: BTreeSet::from(["manifest.json".to_owned(), "manifest.sig".to_owned()]),
    };
    let mut artifacts = Vec::new();
    artifacts.push(writer.copy(
        &settings.core.source,
        "dever-core",
        &settings.core.sha256,
        true,
    )?);
    for input in &settings.core_libraries {
        super::core_libraries::library_name(&input.path)?;
        artifacts.push(writer.copy(
            &input.source,
            &format!("lib/{}", input.path),
            &input.sha256,
            false,
        )?);
    }
    super::core_libraries::validate(staging, &artifacts)?;
    if settings.skill.len() > 256 || !settings.skill.iter().any(|input| input.path == "SKILL.md") {
        return Err("release skill requires SKILL.md and at most 256 files".into());
    }
    let mut skill_bytes = 0_u64;
    for input in settings.skill {
        let source = writer.source(&input.source, &input.path, &input.sha256)?;
        let bytes = fs::metadata(source)
            .map_err(|error| error.to_string())?
            .len();
        skill_bytes = skill_bytes
            .checked_add(bytes)
            .ok_or("release skill size overflow")?;
        if bytes > 1024 * 1024 || skill_bytes > 8 * 1024 * 1024 {
            return Err("release skill exceeds its 1 MiB file or 8 MiB total limit".into());
        }
        artifacts.push(writer.copy(
            &input.source,
            &format!("skills/dever-language/{}", input.path),
            &input.sha256,
            false,
        )?);
    }
    if let Some(input) = settings.bootstrap {
        artifacts.extend(bootstrap::assemble(&mut writer, input)?);
    }
    artifacts.extend(assemble_native(
        &mut writer,
        &version,
        host,
        NativeInputs {
            profiles: settings.profiles,
            start: settings.start,
            libraries: settings.libraries,
            end: settings.end,
        },
    )?);
    let mut extensions = Vec::new();
    let mut target_artifacts = BTreeMap::new();
    for (target, inputs) in settings.native_targets {
        target_artifacts.insert(
            target,
            assemble_native(&mut writer, &version, target, inputs)?,
        );
    }
    for (ecosystem, input) in settings.runtimes.entries() {
        extensions.push(Extension {
            kind: ExtensionKind::Runtime(ecosystem.clone()),
            target: host,
            artifacts: ecosystems::assemble(&mut writer, ecosystem, input, host)?,
        });
    }
    for (target, inputs) in settings.ecosystem_targets {
        for (ecosystem, input) in inputs.entries() {
            extensions.push(Extension {
                kind: ExtensionKind::Runtime(ecosystem.clone()),
                target,
                artifacts: ecosystems::assemble(&mut writer, ecosystem, input, target)?,
            });
        }
    }
    if let Some(inputs) = settings.sandbox {
        artifacts.extend(sandbox::assemble(&mut writer, inputs, host)?);
    }
    for (target, inputs) in settings.sandbox_targets {
        let selected = target_artifacts
            .get_mut(&target)
            .ok_or("target sandbox requires a matching native target")?;
        selected.extend(sandbox::assemble(&mut writer, inputs, target)?);
    }
    for (target, artifacts) in target_artifacts {
        extensions.push(Extension {
            kind: ExtensionKind::Target,
            target,
            artifacts,
        });
    }
    for (ecosystem, input) in settings.builds.entries() {
        extensions.push(Extension {
            kind: ExtensionKind::Build(ecosystem.clone()),
            target: host,
            artifacts: builds::assemble(&mut writer, ecosystem, input, host)?,
        });
    }
    for (target, inputs) in settings.build_targets {
        for (ecosystem, input) in inputs.entries() {
            extensions.push(Extension {
                kind: ExtensionKind::Build(ecosystem.clone()),
                target,
                artifacts: builds::assemble(&mut writer, ecosystem, input, target)?,
            });
        }
    }
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    for extension in &mut extensions {
        extension
            .artifacts
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
    let mut manifest = ReleaseManifest::new(version, artifacts);
    manifest.extensions = extensions;
    validate_catalog(&manifest)?;
    let bytes = write_metadata(&staging.join("manifest.json"), &manifest)?;
    let signature = key.pair.sign(&bytes);
    let hex: String = signature
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    fs::write(staging.join("manifest.sig"), format!("{hex}\n"))
        .map_err(|error| format!("cannot write release signature: {error}"))?;
    set_mode(&staging.join("manifest.sig"), false)?;
    Ok(manifest)
}

fn assemble_native(
    writer: &mut PayloadWriter<'_>,
    version: &Version,
    target: BuildTarget,
    inputs: NativeInputs,
) -> Result<Vec<Artifact>, String> {
    let runtime_prefix = format!("runtime/{}", target.platform());
    let manifest_path = writer.reserve(&format!("{runtime_prefix}/manifest.json"))?;
    let archive_inputs = inputs.profiles;
    let mut files = Vec::new();
    for (name, input) in [
        ("base", archive_inputs.base),
        ("sqlite", archive_inputs.sqlite),
        ("postgres", archive_inputs.postgres),
        ("both", archive_inputs.both),
    ] {
        let path = format!("archives/{name}.a");
        let artifact = writer.copy(
            &input.source,
            &format!("{runtime_prefix}/{path}"),
            &input.sha256,
            false,
        )?;
        files.push(RuntimeArtifact {
            path,
            bytes: artifact.bytes,
            sha256: artifact.sha256,
        });
    }
    for input in inputs
        .start
        .iter()
        .chain(&inputs.libraries)
        .chain(&inputs.end)
    {
        runtime_pack::relative_path(&input.path)?;
        let artifact = writer.copy(
            &input.source,
            &format!("{runtime_prefix}/{}", input.path),
            &input.sha256,
            false,
        )?;
        files.push(RuntimeArtifact {
            path: input.path.clone(),
            bytes: artifact.bytes,
            sha256: artifact.sha256,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let link_paths = |inputs: &[LinkInput]| inputs.iter().map(|input| input.path.clone()).collect();
    let profile = |name| {
        Some(Profile {
            start: link_paths(&inputs.start),
            runtime: format!("archives/{name}.a"),
            libraries: link_paths(&inputs.libraries),
            end: link_paths(&inputs.end),
        })
    };
    let runtime = Manifest {
        format: "dever-native-runtime-v1".into(),
        compiler_version: version.to_string(),
        abi: 1,
        platform: target.platform().into(),
        target: target.triple().into(),
        profiles: Profiles {
            base: profile("base"),
            sqlite: profile("sqlite"),
            postgres: profile("postgres"),
            both: profile("both"),
        },
        files,
    };
    write_metadata(&manifest_path, &runtime)?;
    Ok(
        runtime_pack::validate(writer.staging, version.as_str(), target)?
            .into_iter()
            .map(|artifact| Artifact {
                path: artifact.path,
                bytes: artifact.bytes,
                sha256: artifact.sha256,
            })
            .collect(),
    )
}

impl PayloadWriter<'_> {
    fn reserve(&mut self, destination: &str) -> Result<PathBuf, String> {
        runtime_pack::relative_path(destination)?;
        if self.destinations.iter().any(|path| {
            path == destination
                || path.starts_with(&format!("{destination}/"))
                || destination.starts_with(&format!("{path}/"))
        }) {
            return Err("duplicate release destination or file/directory conflict".into());
        }
        self.destinations.insert(destination.to_owned());
        let output = self.staging.join(destination);
        create_directories(self.staging, output.parent().expect("payload parent"))?;
        Ok(output)
    }

    fn source(&self, source: &str, destination: &str, digest: &str) -> Result<PathBuf, String> {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("payload requires a lowercase SHA-256 digest".into());
        }
        if digest == self.key.sha256 {
            return Err("signing key cannot be included as a release payload".into());
        }
        let source = runtime_pack::checked_path(self.root, source, true)
            .map_err(|_| format!("invalid source for release payload '{destination}'"))?;
        if sha256_file(&source).map_err(|_| "cannot hash source payload")? != digest {
            return Err(format!(
                "release payload '{destination}' failed SHA-256 verification"
            ));
        }
        Ok(source)
    }

    fn copy(
        &mut self,
        source: &str,
        destination: &str,
        digest: &str,
        executable: bool,
    ) -> Result<Artifact, String> {
        let output = self.reserve(destination)?;
        let source = self.source(source, destination, digest)?;
        fs::copy(&source, &output)
            .map_err(|error| format!("cannot copy release payload '{destination}': {error}"))?;
        if sha256_file(&output)? != digest {
            return Err(format!(
                "copied release payload '{destination}' failed SHA-256 verification"
            ));
        }
        set_mode(&output, executable)?;
        Ok(Artifact {
            path: destination.to_owned(),
            bytes: fs::metadata(output)
                .map_err(|error| error.to_string())?
                .len(),
            sha256: digest.to_owned(),
        })
    }
}

fn read_signing_key(root: &Path, relative: &str) -> Result<SigningKey, String> {
    let path = runtime_pack::checked_path(root, relative, true)
        .map_err(|_| "signing key must be a real file within the author root")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&path).map_err(|_| "cannot inspect signing key")?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("signing key permissions must be private to its owner".into());
        }
    }
    let bytes = read_bounded(&path, 4096, "signing key")?;
    let pair = Ed25519KeyPair::from_pkcs8(&bytes)
        .map_err(|_| "signing key is not a valid Ed25519 PKCS#8 key")?;
    Ok(SigningKey {
        pair,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

fn read_bounded(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(limit + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("cannot read {label}: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label} exceeds its size limit"));
    }
    Ok(bytes)
}

fn write_metadata(path: &Path, value: &impl serde::Serialize) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(path, &bytes).map_err(|error| format!("cannot write release metadata: {error}"))?;
    set_mode(path, false)?;
    Ok(bytes)
}

fn output_path(output: &Path) -> Result<PathBuf, String> {
    let name = output
        .file_name()
        .ok_or("release output requires a new directory name")?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = fs::canonicalize(parent)
        .map_err(|error| format!("cannot resolve release output parent: {error}"))?;
    let output = parent.join(name);
    match fs::symlink_metadata(&output) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(output),
        Ok(_) => Err("release output already exists".into()),
        Err(error) => Err(format!("cannot inspect release output: {error}")),
    }
}

fn create_staging(parent: &Path) -> Result<PathBuf, String> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    loop {
        let path = parent.join(format!(
            ".native-release-{}-{}",
            std::process::id(),
            NEXT_STAGING.fetch_add(1, Ordering::Relaxed)
        ));
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create release staging: {error}")),
        }
    }
}

fn create_directories(root: &Path, directory: &Path) -> Result<(), String> {
    let mut current = root.to_owned();
    for component in directory
        .strip_prefix(root)
        .expect("owned staging path")
        .components()
    {
        current.push(component);
        if !current.exists() {
            fs::create_dir(&current)
                .map_err(|error| format!("cannot create payload directory: {error}"))?;
            set_mode(&current, true)?;
        }
    }
    Ok(())
}

fn set_mode(path: &Path, executable: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
        )
        .map_err(|error| format!("cannot set release permissions: {error}"))?;
    }
    #[cfg(not(unix))]
    let _ = (path, executable);
    Ok(())
}

fn publish(staging: &Path, output: &Path) -> Result<(), String> {
    set_mode(staging, true)?;
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{CWD, RenameFlags, renameat_with};
        renameat_with(CWD, staging, CWD, output, RenameFlags::NOREPLACE)
            .map_err(|error| format!("cannot publish new release directory: {error}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = output;
        Err("native release publication is not available for this platform".into())
    }
}
