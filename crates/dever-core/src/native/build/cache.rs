use std::ffi::OsStr;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};

static NEXT_RUNTIME_IDENTITY: AtomicU64 = AtomicU64::new(0);

/// Cargo may rebuild its shared target after returning. Compile against the exact
/// bytes used for the cache identity rather than mutable files in that target.
pub(super) struct RuntimeInputs {
    files: RuntimeFiles,
    identity: String,
}

enum RuntimeFiles {
    Snapshot(Vec<(PathBuf, Vec<u8>)>),
    Unchanged { root: PathBuf, paths: Vec<PathBuf> },
}

impl RuntimeInputs {
    pub(super) fn publish(
        root: &Path,
        runtime: &Path,
        dependencies: &[PathBuf],
    ) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let staging = unique_directory(root, ".dever-runtime-input")?;
        let result = (|| {
            fs::create_dir(staging.join("deps"))?;
            link_or_copy(runtime, &staging.join("libdever_runtime.rlib"))?;
            for dependency in dependencies {
                let name = dependency.file_name().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "runtime dependency has no filename",
                    )
                })?;
                link_or_copy(dependency, &staging.join("deps").join(name))?;
            }
            let staged = Self::read(&staging)?;
            let destination = root.join(&staged.identity);
            match fs::rename(&staging, &destination) {
                Ok(()) => Ok(Self::read(&destination)?),
                Err(_error) if is_real_directory(&destination) => {
                    let existing = Self::read(&destination)?;
                    if existing.identity != staged.identity {
                        return Err(io::Error::other("native runtime input identity collision"));
                    }
                    Ok(existing)
                }
                Err(error) => Err(error),
            }
        })();
        if staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    pub(super) fn read(root: &Path) -> io::Result<Self> {
        validate_runtime_root(root)?;
        let mut paths = vec![PathBuf::from("libdever_runtime.rlib")];
        for entry in fs::read_dir(root.join("deps"))? {
            let path = entry?.path();
            paths.push(Path::new("deps").join(path.file_name().expect("dependency filename")));
        }
        paths.sort();
        let metadata = paths
            .iter()
            .map(|path| {
                let metadata = fs::metadata(root.join(path))?;
                Ok((path.clone(), metadata.len(), metadata.modified()?))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let metadata_identity = fingerprint(&metadata);
        if let Some(identity) = read_runtime_identity(root, &metadata_identity) {
            cleanup_runtime_identities(root, &metadata_identity);
            return Ok(Self {
                files: RuntimeFiles::Unchanged {
                    root: root.to_path_buf(),
                    paths,
                },
                identity,
            });
        }
        let files = read_files(root, &paths)?;
        let identity = fingerprint(&files);
        write_runtime_identity(root, &metadata_identity, &identity);
        Ok(Self {
            files: RuntimeFiles::Snapshot(files),
            identity,
        })
    }

    pub(super) fn write(&self, root: &Path) -> io::Result<()> {
        fs::create_dir(root)?;
        fs::create_dir(root.join("deps"))?;
        match &self.files {
            RuntimeFiles::Snapshot(files) => write_files(root, files),
            RuntimeFiles::Unchanged {
                root: source_root,
                paths,
            } => {
                let files = read_files(source_root, paths)?;
                if fingerprint(&files) != self.identity {
                    return Err(io::Error::other(
                        "native runtime changed after its cache identity was read",
                    ));
                }
                write_files(root, &files)
            }
        }
    }
}

fn validate_runtime_root(root: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native runtime input '{}' is not a real directory",
                root.display()
            ),
        ));
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let metadata = fs::symlink_metadata(entry.path())?;
        let valid = match name.as_ref() {
            "libdever_runtime.rlib" => metadata.is_file() && !metadata.file_type().is_symlink(),
            "deps" => metadata.is_dir() && !metadata.file_type().is_symlink(),
            name if name.starts_with(".dever-runtime-identity-") => {
                metadata.is_file() && !metadata.file_type().is_symlink()
            }
            _ => false,
        };
        if !valid {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unexpected native runtime input '{}'",
                    entry.path().display()
                ),
            ));
        }
    }
    let runtime = root.join("libdever_runtime.rlib");
    let metadata = fs::symlink_metadata(&runtime)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native runtime input '{}' is not a regular file",
                runtime.display()
            ),
        ));
    }
    for entry in fs::read_dir(root.join("deps"))? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let supported = path.extension().is_some_and(|extension| {
            matches!(extension.to_str(), Some("rlib" | "so" | "dylib" | "dll"))
        });
        if metadata.file_type().is_symlink() || !metadata.is_file() || !supported {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected native runtime dependency '{}'", path.display()),
            ));
        }
    }
    Ok(())
}

fn is_real_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

fn unique_directory(root: &Path, prefix: &str) -> io::Result<PathBuf> {
    loop {
        let path = root.join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT_RUNTIME_IDENTITY.fetch_add(1, Ordering::Relaxed),
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn link_or_copy(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "native runtime input '{}' is not a regular file",
                source.display()
            ),
        ));
    }
    match fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(source, destination)?;
            Ok(())
        }
    }
}

fn read_files(root: &Path, paths: &[PathBuf]) -> io::Result<Vec<(PathBuf, Vec<u8>)>> {
    paths
        .iter()
        .map(|path| Ok((path.clone(), fs::read(root.join(path))?)))
        .collect()
}

fn write_files(root: &Path, files: &[(PathBuf, Vec<u8>)]) -> io::Result<()> {
    for (path, bytes) in files {
        fs::write(root.join(path), bytes)?;
    }
    Ok(())
}

fn read_runtime_identity(root: &Path, metadata_identity: &str) -> Option<String> {
    let text =
        fs::read_to_string(root.join(format!(".dever-runtime-identity-{metadata_identity}")))
            .ok()?;
    let mut lines = text.lines();
    if lines.next()? != "dever-runtime-identity-v1" || lines.next()? != metadata_identity {
        return None;
    }
    let identity = lines.next()?;
    if lines.next().is_some()
        || identity.len() != 64
        || !identity.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(identity.to_owned())
}

fn write_runtime_identity(root: &Path, metadata_identity: &str, identity: &str) {
    let path = root.join(format!(".dever-runtime-identity-{metadata_identity}"));
    let staging = root.join(format!(
        ".dever-runtime-identity-{}-{}",
        std::process::id(),
        NEXT_RUNTIME_IDENTITY.fetch_add(1, Ordering::Relaxed),
    ));
    if fs::write(
        &staging,
        format!("dever-runtime-identity-v1\n{metadata_identity}\n{identity}\n"),
    )
    .is_ok()
    {
        let _ = fs::rename(&staging, &path);
    }
    let _ = fs::remove_file(staging);
    cleanup_runtime_identities(root, metadata_identity);
}

fn cleanup_runtime_identities(root: &Path, metadata_identity: &str) {
    let current = format!(".dever-runtime-identity-{metadata_identity}");
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let stored_identity = name.strip_prefix(".dever-runtime-identity-");
            if name != current
                && stored_identity.is_some_and(|identity| {
                    matches!(identity.len(), 32 | 64)
                        && identity.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

pub(super) struct Cache {
    directory: PathBuf,
    identity: String,
}

impl Cache {
    pub(super) fn for_artifact(root: &Path, identity: &str) -> Self {
        Self {
            directory: root.join(fingerprint(&identity)),
            identity: identity.to_owned(),
        }
    }

    pub(super) fn new(
        root: &Path,
        generated: &str,
        rustc: &OsStr,
        version: &[u8],
        runtime: &RuntimeInputs,
        options: &[&str],
    ) -> io::Result<Self> {
        static COMPILER: OnceLock<String> = OnceLock::new();
        let compiler = match COMPILER.get() {
            Some(identity) => identity.clone(),
            None => {
                // Generated source already captures every backend input. Partition
                // compiler builds by their file identity without rehashing a large
                // debug compiler binary on every separate CLI invocation.
                let path = std::env::current_exe()?;
                let metadata = fs::metadata(&path)?;
                let identity = fingerprint(&(
                    env!("CARGO_PKG_VERSION"),
                    path,
                    metadata.len(),
                    metadata.modified()?,
                ));
                COMPILER.get_or_init(|| identity).clone()
            }
        };
        let tool = executable_path(rustc)?;
        let environment: Vec<_> = [
            "PATH",
            "RUSTUP_TOOLCHAIN",
            "RUSTC_BOOTSTRAP",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_TARGET",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CC",
            "CFLAGS",
            "LDFLAGS",
            "LIBRARY_PATH",
            "LD_LIBRARY_PATH",
            "SDKROOT",
            "MACOSX_DEPLOYMENT_TARGET",
            "SOURCE_DATE_EPOCH",
        ]
        .into_iter()
        .map(|name| (name, std::env::var_os(name)))
        .collect();
        let identity = format!(
            "dever-native-cache-v1\ncompiler={compiler}\ngenerated={}\ntool={}\nversion={}\nruntime={}\noptions={}\nplatform={}-{}\nenvironment={}\n",
            fingerprint(&generated),
            fingerprint(&(rustc, tool.as_os_str(), file_fingerprint(&tool)?)),
            fingerprint(&version),
            runtime.identity,
            fingerprint(&options),
            std::env::consts::OS,
            std::env::consts::ARCH,
            fingerprint(&environment),
        );
        Ok(Self {
            directory: root.join(fingerprint(&identity)),
            identity,
        })
    }

    pub(super) fn restore(&self, executable: &Path) -> io::Result<bool> {
        let stored = match fs::read_to_string(self.directory.join("identity")) {
            Ok(identity) => identity,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        if stored != self.identity {
            return Err(io::Error::other("native cache identity mismatch"));
        }
        let program = self.directory.join("program");
        let expected = fs::read_to_string(self.directory.join("executable-fingerprint"))?;
        if file_fingerprint(&program)? != expected {
            return Err(io::Error::other(
                "native cache executable is incomplete or modified",
            ));
        }
        fs::copy(program, executable)?;
        Ok(true)
    }

    pub(super) fn publish(&self, executable: &Path, staging: &Path) -> io::Result<()> {
        fs::copy(executable, staging.join("program"))?;
        fs::write(staging.join("identity"), &self.identity)?;
        fs::write(
            staging.join("executable-fingerprint"),
            file_fingerprint(executable)?,
        )?;
        match fs::rename(staging, &self.directory) {
            Ok(()) => Ok(()),
            // Another complete build won the same key. Incomplete staging directories
            // never have a cache key and are never considered by restore.
            Err(error) => match fs::read_to_string(self.directory.join("identity")) {
                Ok(identity) if identity == self.identity => Ok(()),
                Ok(_) => Err(io::Error::other("native cache identity collision")),
                Err(_) => Err(error),
            },
        }
    }
}

fn executable_path(command: &OsStr) -> io::Result<PathBuf> {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return fs::canonicalize(path);
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(command);
        if path.is_file() {
            return fs::canonicalize(path);
        }
        #[cfg(windows)]
        {
            let path = path.with_extension("exe");
            if path.is_file() {
                return fs::canonicalize(path);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "compiler executable not found",
    ))
}

fn fingerprint(value: &impl Hash) -> String {
    let mut hasher = CryptographicHasher(Sha256::new());
    value.hash(&mut hasher);
    hex(&hasher.0.finalize())
}

fn file_fingerprint(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes = [0_u8; 65536];
    loop {
        let length = file.read(&mut bytes)?;
        if length == 0 {
            break;
        }
        hasher.update(&bytes[..length]);
    }
    Ok(hex(&hasher.finalize()))
}

struct CryptographicHasher(Sha256);

impl Hasher for CryptographicHasher {
    fn finish(&self) -> u64 {
        let digest = self.0.clone().finalize();
        u64::from_le_bytes(digest[..8].try_into().expect("SHA-256 prefix"))
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}
