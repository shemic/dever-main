use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::release::{Layout, Version, hex};

static NEXT_CACHE_OPERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct BuildIdentity<'a> {
    pub version: &'a Version,
    pub compiler: &'a [u8],
    pub runtime: &'a [u8],
    pub target: &'a str,
    pub generated: &'a [u8],
    pub options: &'a [u8],
    pub environment: &'a [u8],
}

impl BuildIdentity<'_> {
    pub fn key(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"dever-machine-cache-v1\0");
        for value in [
            self.version.as_str().as_bytes(),
            self.compiler,
            self.runtime,
            self.target.as_bytes(),
            self.generated,
            self.options,
            self.environment,
        ] {
            digest.update((value.len() as u64).to_le_bytes());
            digest.update(value);
        }
        hex(&digest.finalize())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CacheEntry {
    pub format: String,
    pub key: String,
    pub version: Version,
    pub target: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct CacheStatus {
    #[serde(default)]
    pub verified: bool,
    pub entries: u64,
    pub bytes: u64,
    pub versions: BTreeMap<String, u64>,
    pub corrupt_entries: u64,
    pub staging_entries: u64,
    #[serde(default)]
    pub artifact_entries: u64,
    #[serde(default)]
    pub artifact_bytes: u64,
    #[serde(default)]
    pub corrupt_artifacts: u64,
}

/// The service accepts content identities, never caller-selected cache paths.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    pub sha256: String,
    pub bytes: u64,
}

pub const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const MAX_COMPILED_BYTES: u64 = 320 * 1024 * 1024;
const MAX_ARTIFACT_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARTIFACT_ENTRIES: usize = 4096;

impl ArtifactReceipt {
    pub fn validate(&self) -> Result<(), String> {
        validate_key(&self.sha256)?;
        if self.bytes > MAX_ARTIFACT_BYTES {
            return Err("shared artifact exceeds the 64 MiB transfer limit".into());
        }
        Ok(())
    }

    pub(super) fn validate_compiled(&self) -> Result<(), String> {
        validate_key(&self.sha256)?;
        if self.bytes == 0 || self.bytes > MAX_COMPILED_BYTES {
            return Err("compiled output must be between 1 byte and 320 MiB".into());
        }
        Ok(())
    }
}

pub struct StoredArtifact {
    pub receipt: ArtifactReceipt,
    pub file: File,
    _operation: CacheOperation,
}

/// Filesystem engine owned by the privileged cache process. It deliberately has
/// no transport or caller-authentication semantics; project processes must use
/// the authenticated `deverd` IPC client rather than constructing this store directly.
pub struct CacheStore {
    root: PathBuf,
}

impl CacheStore {
    pub fn new(layout: &Layout) -> Self {
        Self {
            root: layout.native_cache(),
        }
    }

    /// Hold across input staging, compilation, publication and response transfer.
    pub(crate) fn operation(&self) -> Result<CacheOperation, String> {
        CacheOperation::acquire(&self.root)
    }

    pub(crate) fn compilation_directory(&self) -> Result<CompilationDirectory, String> {
        let root = self.root.join("staging");
        create_directory(&root)?;
        let path = root.join(format!(
            "compile-{}-{}",
            std::process::id(),
            NEXT_CACHE_OPERATION.fetch_add(1, Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|error| format!("cannot stage compilation: {error}"))?;
        Ok(CompilationDirectory(path))
    }

    /// Hash while receiving into an owned staging file. Only verified complete
    /// bytes are published, and concurrent identical uploads share one entry.
    pub fn publish_artifact(
        &self,
        expected: &ArtifactReceipt,
        input: &mut impl Read,
    ) -> Result<ArtifactReceipt, String> {
        expected.validate()?;
        let _operation = CacheOperation::acquire(&self.root)?;
        let staging_root = self.root.join("staging");
        create_directory(&staging_root)?;
        let staging = staging_root.join(format!(
            "artifact-{}-{}",
            std::process::id(),
            NEXT_CACHE_OPERATION.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&staging)
                .map_err(|error| format!("cannot stage shared artifact: {error}"))?;
            let mut digest = Sha256::new();
            let mut remaining = expected.bytes;
            let mut buffer = [0_u8; 64 * 1024];
            while remaining > 0 {
                let limit = remaining.min(buffer.len() as u64) as usize;
                input
                    .read_exact(&mut buffer[..limit])
                    .map_err(|error| format!("incomplete shared artifact upload: {error}"))?;
                file.write_all(&buffer[..limit])
                    .map_err(|error| format!("cannot stage shared artifact: {error}"))?;
                digest.update(&buffer[..limit]);
                remaining -= limit as u64;
            }
            if hex(&digest.finalize()) != expected.sha256 {
                return Err("shared artifact upload failed SHA-256 verification".into());
            }
            file.sync_all()
                .map_err(|error| format!("cannot sync shared artifact: {error}"))?;
            drop(file);
            let artifacts = self.root.join("artifacts");
            create_directory(&artifacts)?;
            let _publication = ArtifactPublication::acquire(&self.root)?;
            let destination = artifacts.join(&expected.sha256);
            match fs::symlink_metadata(&destination) {
                Ok(_) => {
                    self.read_artifact_file(expected)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let (used, count) = directory_entries(&artifacts)?.try_fold(
                        (0_u64, 0_usize),
                        |(sum, count), entry| {
                            let metadata = fs::symlink_metadata(entry.path())
                                .map_err(|error| error.to_string())?;
                            if !metadata.is_file() || metadata.file_type().is_symlink() {
                                return Err(
                                    "shared artifact cache contains a non-regular file".into()
                                );
                            }
                            Ok::<_, String>((sum.saturating_add(metadata.len()), count + 1))
                        },
                    )?;
                    if count >= MAX_ARTIFACT_ENTRIES
                        || used.saturating_add(expected.bytes) > MAX_ARTIFACT_CACHE_BYTES
                    {
                        return Err("shared artifact cache is full; administrator must run 'dever cache clean'".into());
                    }
                    fs::rename(&staging, &destination)
                        .map_err(|error| format!("cannot publish shared artifact: {error}"))?;
                    sync_directory(&artifacts)
                        .map_err(|error| format!("cannot sync shared artifacts: {error}"))?;
                }
                Err(error) => return Err(format!("cannot inspect shared artifact: {error}")),
            }
            Ok(expected.clone())
        })();
        if staging.exists() {
            let _ = fs::remove_file(&staging);
        }
        result
    }

    pub fn artifact(&self, expected: &ArtifactReceipt) -> Result<StoredArtifact, String> {
        expected.validate()?;
        let operation = CacheOperation::acquire(&self.root)?;
        let file = self.read_artifact_file(expected)?;
        Ok(StoredArtifact {
            receipt: expected.clone(),
            file,
            _operation: operation,
        })
    }

    fn read_artifact_file(&self, expected: &ArtifactReceipt) -> Result<File, String> {
        let path = self.artifact_path(expected)?;
        if sha256_file(&path)? != expected.sha256 {
            return Err("shared artifact failed SHA-256 verification".into());
        }
        File::open(&path).map_err(|error| format!("cannot read shared artifact: {error}"))
    }

    fn artifact_path(&self, expected: &ArtifactReceipt) -> Result<PathBuf, String> {
        expected.validate()?;
        let directory = self.root.join("artifacts");
        ensure_real_directory(&directory)
            .map_err(|error| format!("shared artifact unavailable: {error}"))?;
        let path = directory.join(&expected.sha256);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("shared artifact unavailable: {error}"))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != expected.bytes
        {
            return Err("shared artifact is incomplete or modified".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let owner = fs::symlink_metadata(&self.root)
                .map_err(|error| error.to_string())?
                .uid();
            if metadata.uid() != owner || metadata.mode() & 0o077 != 0 {
                return Err("shared artifact must be private to the service owner".into());
            }
        }
        Ok(path)
    }

    pub fn publish(
        &self,
        identity: &BuildIdentity<'_>,
        executable: &Path,
    ) -> Result<String, String> {
        let _operation = CacheOperation::acquire(&self.root)?;
        let _publication = ArtifactPublication::acquire(&self.root)?;
        let key = identity.key();
        let destination = self.root.join("entries").join(&key);
        if destination.exists() {
            self.read_entry(&key)?;
            return Ok(key);
        }
        let staging_root = self.root.join("staging");
        create_directory(&staging_root)?;
        create_directory(&self.root.join("entries"))?;
        let bytes = fs::symlink_metadata(executable)
            .map_err(|error| format!("cannot inspect compiled output: {error}"))?
            .len();
        if bytes == 0 || bytes > MAX_COMPILED_BYTES {
            return Err("compiled output must be between 1 byte and 320 MiB".into());
        }
        let (used, count) = directory_entries(&self.root.join("entries"))?.try_fold(
            (0_u64, 0_usize),
            |(used, count), entry| {
                let key = entry.file_name().to_string_lossy().into_owned();
                let manifest = self.inspect_entry(&key, false)?;
                Ok::<_, String>((used.saturating_add(manifest.bytes), count + 1))
            },
        )?;
        if count >= MAX_ARTIFACT_ENTRIES || used.saturating_add(bytes) > MAX_ARTIFACT_CACHE_BYTES {
            return Err("shared compilation cache quota exceeded (2 GiB / 4096 entries)".into());
        }
        let staging = staging_root.join(format!(
            "publish-{}-{}",
            std::process::id(),
            NEXT_CACHE_OPERATION.fetch_add(1, Ordering::Relaxed),
        ));
        create_directory(&staging)?;
        let result = (|| {
            let program = staging.join("program");
            copy_regular_file(executable, &program)?;
            let metadata = fs::metadata(&program).map_err(|error| error.to_string())?;
            let entry = CacheEntry {
                format: "dever-machine-cache-entry-v1".into(),
                key: key.clone(),
                version: identity.version.clone(),
                target: identity.target.to_owned(),
                bytes: metadata.len(),
                sha256: sha256_file(&program)?,
            };
            let manifest = serde_json::to_vec(&entry).map_err(|error| error.to_string())?;
            write_new(&staging.join("entry.json"), &manifest)?;
            sync_directory(&staging).map_err(|error| error.to_string())?;
            match fs::rename(&staging, &destination) {
                Ok(()) => {
                    sync_directory(&self.root.join("entries")).map_err(|error| error.to_string())
                }
                Err(_error) if destination.exists() => {
                    self.read_entry(&key)?;
                    Ok(())
                }
                Err(error) => Err(format!("cannot publish shared cache entry: {error}")),
            }
        })();
        if staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        result.map(|()| key)
    }

    pub fn restore(&self, identity: &BuildIdentity<'_>) -> Result<Option<CachedArtifact>, String> {
        let key = identity.key();
        let entry = match self.read_entry(&key) {
            Ok(entry) => entry,
            Err(error) if error.starts_with("shared cache miss:") => return Ok(None),
            Err(error) => return Err(error),
        };
        if entry.version != *identity.version || entry.target != identity.target {
            return Err("shared cache identity collision".into());
        }
        let lease = CacheLease::acquire(&self.root, &key)?;
        let path = self.root.join("entries").join(&key).join("program");
        let bytes = read_regular_file(&path)?;
        if bytes.len() as u64 != entry.bytes || sha256(&bytes) != entry.sha256 {
            return Err(format!(
                "shared cache entry {key} is incomplete or modified"
            ));
        }
        Ok(Some(CachedArtifact {
            bytes,
            _lease: lease,
        }))
    }

    pub fn status(&self) -> Result<CacheStatus, String> {
        self.inspect_status(true)
    }

    /// Unprivileged status is metadata-only: it cannot force hashing all
    /// libraries/programs. It never establishes an artifact integrity claim.
    pub fn summary(&self) -> Result<CacheStatus, String> {
        self.inspect_status(false)
    }

    fn inspect_status(&self, verify_bytes: bool) -> Result<CacheStatus, String> {
        let mut status = CacheStatus {
            verified: verify_bytes,
            ..CacheStatus::default()
        };
        for entry in directory_entries(&self.root.join("staging"))? {
            let _ = entry;
            status.staging_entries += 1;
        }
        for entry in directory_entries(&self.root.join("entries"))? {
            let key = entry
                .file_name()
                .into_string()
                .map_err(|_| "shared cache key is not UTF-8".to_owned())?;
            match self.inspect_entry(&key, verify_bytes) {
                Ok(manifest) => {
                    status.entries += 1;
                    status.bytes = status.bytes.saturating_add(manifest.bytes);
                    *status
                        .versions
                        .entry(manifest.version.to_string())
                        .or_default() += 1;
                }
                Err(_) => status.corrupt_entries += 1,
            }
        }
        for entry in directory_entries(&self.root.join("artifacts"))? {
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            let expected = ArtifactReceipt {
                sha256: entry.file_name().to_string_lossy().into_owned(),
                bytes: metadata.len(),
            };
            let inspected = if verify_bytes {
                self.read_artifact_file(&expected).map(|_| ())
            } else {
                self.artifact_path(&expected).map(|_| ())
            };
            match inspected {
                Ok(_) => {
                    status.artifact_entries += 1;
                    status.artifact_bytes = status.artifact_bytes.saturating_add(expected.bytes);
                }
                Err(_) => status.corrupt_artifacts += 1,
            }
        }
        Ok(status)
    }

    pub fn clean(&self, installed: &BTreeSet<Version>) -> Result<CacheStatus, String> {
        let _lock = CacheCleanLock::acquire(&self.root)?;
        if directory_entries(&self.root.join("operations"))?
            .next()
            .is_some()
        {
            return Err("shared cache is busy; clean did not remove active build state".into());
        }
        remove_children(&self.root.join("staging"))?;
        // Artifacts are a recoverable download cache. Explicit administrator
        // cleanup removes them only while no upload or reader is active.
        remove_children(&self.root.join("artifacts"))?;
        for entry in directory_entries(&self.root.join("entries"))? {
            let key = entry
                .file_name()
                .into_string()
                .map_err(|_| "shared cache key is not UTF-8".to_owned())?;
            if self.has_lease(&key)? {
                continue;
            }
            let remove = self
                .read_entry(&key)
                .map(|entry| !installed.contains(&entry.version))
                .unwrap_or(true);
            if remove {
                remove_entry(entry.path())?;
            }
        }
        self.status()
    }

    fn read_entry(&self, key: &str) -> Result<CacheEntry, String> {
        self.inspect_entry(key, true)
    }

    fn inspect_entry(&self, key: &str, verify_bytes: bool) -> Result<CacheEntry, String> {
        validate_key(key)?;
        let root = self.root.join("entries").join(key);
        if !root.exists() {
            return Err(format!("shared cache miss: {key}"));
        }
        ensure_real_directory(&root)?;
        let manifest: CacheEntry =
            serde_json::from_slice(&read_regular_file(&root.join("entry.json"))?)
                .map_err(|error| format!("invalid shared cache manifest {key}: {error}"))?;
        if manifest.format != "dever-machine-cache-entry-v1"
            || manifest.key != key
            || manifest.bytes == 0
            || manifest.bytes > MAX_COMPILED_BYTES
        {
            return Err(format!("invalid shared cache manifest {key}"));
        }
        let program = root.join("program");
        let metadata = fs::symlink_metadata(&program)
            .map_err(|error| format!("cannot inspect shared cache entry {key}: {error}"))?;
        if !metadata.file_type().is_file()
            || metadata.len() != manifest.bytes
            || (verify_bytes && sha256_file(&program)? != manifest.sha256)
        {
            return Err(format!(
                "shared cache entry {key} is incomplete or modified"
            ));
        }
        Ok(manifest)
    }

    fn has_lease(&self, key: &str) -> Result<bool, String> {
        let directory = self.root.join("leases").join(key);
        Ok(directory_entries(&directory)?.next().is_some())
    }
}

pub struct CachedArtifact {
    bytes: Vec<u8>,
    _lease: CacheLease,
}

impl CachedArtifact {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

struct CacheLease {
    path: PathBuf,
}

impl CacheLease {
    fn acquire(root: &Path, key: &str) -> Result<Self, String> {
        if root.join("clean.lock").exists() {
            return Err("shared cache maintenance is active".into());
        }
        let directory = root.join("leases").join(key);
        create_directory(&directory)?;
        let path = directory.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT_CACHE_OPERATION.fetch_add(1, Ordering::Relaxed),
        ));
        write_new(&path, b"dever-cache-lease-v1\n")?;
        if root.join("clean.lock").exists() {
            let _ = fs::remove_file(&path);
            return Err("shared cache maintenance is active".into());
        }
        Ok(Self { path })
    }
}

impl Drop for CacheLease {
    fn drop(&mut self) {
        let parent = self.path.parent().map(Path::to_path_buf);
        let _ = fs::remove_file(&self.path);
        if let Some(parent) = parent {
            let _ = fs::remove_dir(parent);
        }
    }
}

struct CacheCleanLock(PathBuf);

impl CacheCleanLock {
    fn acquire(root: &Path) -> Result<Self, String> {
        create_directory(root)?;
        let path = root.join("clean.lock");
        write_new(&path, format!("{}\n", std::process::id()).as_bytes())?;
        Ok(Self(path))
    }
}

pub(crate) struct CacheOperation(PathBuf);

pub(crate) struct CompilationDirectory(pub(crate) PathBuf);

impl Drop for CompilationDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl CacheOperation {
    fn acquire(root: &Path) -> Result<Self, String> {
        create_directory(root)?;
        if root.join("clean.lock").exists() {
            return Err("shared cache maintenance is active".into());
        }
        let directory = root.join("operations");
        create_directory(&directory)?;
        let path = directory.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT_CACHE_OPERATION.fetch_add(1, Ordering::Relaxed),
        ));
        write_new(&path, b"dever-cache-operation-v1\n")?;
        if root.join("clean.lock").exists() {
            let _ = fs::remove_file(&path);
            return Err("shared cache maintenance is active".into());
        }
        Ok(Self(path))
    }
}

impl Drop for CacheOperation {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Drop for CacheCleanLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Serializes quota accounting and publication, not network reception.
struct ArtifactPublication(File);

impl ArtifactPublication {
    fn acquire(root: &Path) -> Result<Self, String> {
        let path = root.join("artifact-publish.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err("artifact publication lock must be a regular file".into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                options.create_new(true);
            }
            Err(error) => return Err(format!("cannot inspect artifact publication lock: {error}")),
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|error| format!("cannot open artifact publication lock: {error}"))?,
            Err(error) => return Err(format!("cannot open artifact publication lock: {error}")),
        };
        fs2::FileExt::lock_exclusive(&file)
            .map_err(|error| format!("cannot lock artifact publication: {error}"))?;
        Ok(Self(file))
    }
}

impl Drop for ArtifactPublication {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.len() != 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("invalid shared cache key".into());
    }
    Ok(())
}

fn directory_entries(path: &Path) -> Result<impl Iterator<Item = fs::DirEntry>, String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new().into_iter()),
        Err(error) => return Err(format!("cannot inspect '{}': {error}", path.display())),
    };
    let entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?;
    Ok(entries.into_iter())
}

fn remove_children(path: &Path) -> Result<(), String> {
    for entry in directory_entries(path)? {
        remove_entry(entry.path())?;
    }
    Ok(())
}

fn remove_entry(path: PathBuf) -> Result<(), String> {
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_file(&path)
    }
    .map_err(|error| format!("cannot remove '{}': {error}", path.display()))
}

fn create_directory(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|error| format!("cannot create '{}': {error}", path.display()))?;
    ensure_real_directory(path)
}

fn ensure_real_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("'{}' must be a real directory", path.display()));
    }
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("cannot inspect '{}': {error}", source.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("'{}' must be a regular file", source.display()));
    }
    fs::copy(source, destination)
        .map_err(|error| format!("cannot copy '{}': {error}", source.display()))?;
    fs::set_permissions(destination, metadata.permissions())
        .map_err(|error| format!("cannot preserve '{}': {error}", destination.display()))
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect '{}': {error}", path.display()))?;
    if !metadata.file_type().is_file() {
        return Err(format!("'{}' must be a regular file", path.display()));
    }
    fs::read(path).map_err(|error| format!("cannot read '{}': {error}", path.display()))
}

fn write_new(path: &Path, value: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create '{}': {error}", path.display()))?;
    file.write_all(value)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write '{}': {error}", path.display()))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let length = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot hash '{}': {error}", path.display()))?;
        if length == 0 {
            break;
        }
        digest.update(&buffer[..length]);
    }
    Ok(hex(&digest.finalize()))
}

fn sha256(value: &[u8]) -> String {
    hex(&Sha256::digest(value))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}
