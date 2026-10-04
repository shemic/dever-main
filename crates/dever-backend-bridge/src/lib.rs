//! Private compiler bridge. LLVM/LLD handles never escape the safe interface.
//! This crate is not the HIR lowerer and does not expose language FFI.

/// Runtime packs must match the versioned symbols in include/runtime.h.
pub const RUNTIME_ABI_VERSION: u32 = 1;

#[cfg(any(feature = "embedded", feature = "runtime-abi"))]
mod ffi;

#[cfg(feature = "runtime-abi")]
pub use dever_runtime::abi::DecimalBits as AbiDecimal;

#[cfg(feature = "runtime-abi")]
pub const ABI_OK: u32 = 0;
#[cfg(feature = "runtime-abi")]
pub const ABI_RUNTIME_ERROR: u32 = 1;
#[cfg(feature = "runtime-abi")]
pub const ABI_INVALID_INPUT: u32 = 2;

/// The six release targets; target selection never consults host defaults.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    LinuxX86_64,
    LinuxAarch64,
    MacosX86_64,
    MacosAarch64,
    WindowsX86_64,
    WindowsAarch64,
}

impl Target {
    pub const ALL: [Self; 6] = [
        Self::LinuxX86_64,
        Self::LinuxAarch64,
        Self::MacosX86_64,
        Self::MacosAarch64,
        Self::WindowsX86_64,
        Self::WindowsAarch64,
    ];

    pub fn triple(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "x86_64-unknown-linux-gnu",
            Self::LinuxAarch64 => "aarch64-unknown-linux-gnu",
            Self::MacosX86_64 => "x86_64-apple-macosx11.0.0",
            Self::MacosAarch64 => "arm64-apple-macosx11.0.0",
            Self::WindowsX86_64 => "x86_64-pc-windows-msvc",
            Self::WindowsAarch64 => "aarch64-pc-windows-msvc",
        }
    }
}

/// Parse, verify, optimize and emit a position-independent native object.
/// Errors are LLVM diagnostics, not successful empty artifacts.
#[cfg(feature = "embedded")]
pub fn emit_object(ir: &str, target: Target) -> Result<Vec<u8>, String> {
    emit_object_with_resources(ir, target, &[])
}

/// A borrowed immutable initializer for one declared byte-array global.
#[derive(Clone, Copy, Debug)]
pub struct BinaryResource<'a> {
    pub symbol: &'a str,
    pub bytes: &'a [u8],
}

/// Attach binary payloads before verification and optimization, without text encoding.
#[cfg(feature = "embedded")]
pub fn emit_object_with_resources(
    ir: &str,
    target: Target,
    resources: &[BinaryResource<'_>],
) -> Result<Vec<u8>, String> {
    if ir.is_empty() || ir.len() > 8 * 1024 * 1024 {
        return Err("LLVM IR must contain 1..8388608 bytes".into());
    }
    if ir.as_bytes().contains(&0) {
        return Err("LLVM IR must escape NUL bytes".into());
    }
    if resources.len() > 4096 {
        return Err("LLVM resources exceed 4096 attachments".into());
    }
    let mut total = 0usize;
    let mut symbols = std::collections::BTreeSet::new();
    for resource in resources {
        if resource.symbol.is_empty()
            || resource.symbol.len() > 256
            || !resource.symbol.bytes().enumerate().all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
            })
        {
            return Err("LLVM resource requires an unquoted symbol name".into());
        }
        if !symbols.insert(resource.symbol) {
            return Err("duplicate LLVM resource symbol".into());
        }
        if resource.bytes.len() > 128 * 1024 * 1024 {
            return Err("LLVM resource exceeds 128 MiB".into());
        }
        total = total
            .checked_add(resource.bytes.len())
            .ok_or("LLVM resource size overflow")?;
        if total > 256 * 1024 * 1024 {
            return Err("LLVM resources exceed 256 MiB".into());
        }
    }
    ffi::emit_object(ir, target, resources)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkKind {
    Executable,
    /// Standard CRT entry with explicit archives; never search host shared libraries.
    StaticExecutable,
    Shared,
}

/// Inputs are explicit object/archive/import-library paths supplied by the
/// compiler's verified pack owner. LLD cannot search host library directories.
/// Output must be new and below a compiler-owned private build directory.
/// An unrecoverable LLD crash terminates the private compiler worker rather
/// than allowing cleanup or another call into potentially corrupted state.
#[cfg(feature = "embedded")]
pub fn link(
    target: Target,
    kind: LinkKind,
    inputs: &[std::path::PathBuf],
    output: &std::path::Path,
    entry: Option<&str>,
) -> Result<(), String> {
    use std::ffi::CString;

    if inputs.is_empty() || inputs.len() > 4096 {
        return Err("link requires 1..4096 explicit inputs".into());
    }
    let parent = output
        .parent()
        .ok_or("link output has no parent")?
        .canonicalize()
        .map_err(|error| format!("link output directory: {error}"))?;
    verify_link_directory(&parent)?;
    let output = parent.join(output.file_name().ok_or("link output has no filename")?);
    let mut args = link_arguments(target, kind, entry)?;
    args.extend(output_arguments(target, &output)?);
    for input in inputs {
        let path = input
            .canonicalize()
            .map_err(|error| format!("link input: {error}"))?;
        if !path.is_file() {
            return Err("link input is not a regular file".into());
        }
        args.push(
            path.to_str()
                .ok_or("link input path must be UTF-8")?
                .to_owned(),
        );
    }
    let args = args
        .into_iter()
        .map(CString::new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "link argument contains NUL")?;
    // Claim the new output before entering the serialized linker. Concurrent
    // callers cannot both pass a check-then-create and overwrite each other.
    let mut create = std::fs::OpenOptions::new();
    create.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        create.mode(0o600);
    }
    drop(
        create
            .open(&output)
            .map_err(|error| format!("cannot reserve new link output: {error}"))?,
    );
    let result = ffi::link(&args);
    // Only our new output is eligible for failure cleanup, never input packs.
    if result.is_err()
        && let Err(error) = std::fs::remove_file(&output)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!(
            "{}; link output cleanup: {error}",
            result.unwrap_err()
        ));
    }
    result
}

#[cfg(all(feature = "embedded", unix))]
fn verify_link_directory(directory: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let uid = rustix::process::geteuid().as_raw();
    for (index, ancestor) in directory.ancestors().enumerate() {
        let metadata = ancestor
            .symlink_metadata()
            .map_err(|error| format!("cannot inspect link directory: {error}"))?;
        if !metadata.is_dir() || !matches!(metadata.uid(), owner if owner == uid || owner == 0) {
            return Err("link directory has an untrusted ancestor".into());
        }
        let mode = metadata.mode();
        if index == 0 && (metadata.uid() != uid || mode & 0o077 != 0) {
            return Err("link output requires a compiler-owned private directory".into());
        }
        // Sticky OS temp roots protect our owned entry. A writable non-sticky
        // ancestor would permit another UID to rename an otherwise private leaf.
        if mode & 0o022 != 0 && mode & 0o1000 == 0 {
            return Err("link directory has a publicly writable ancestor".into());
        }
    }
    Ok(())
}

#[cfg(all(feature = "embedded", not(unix)))]
fn verify_link_directory(_: &std::path::Path) -> Result<(), String> {
    Err("compiler-private link directory verification is unavailable on this host".into())
}

#[cfg(feature = "embedded")]
fn output_arguments(target: Target, output: &std::path::Path) -> Result<Vec<String>, String> {
    let output = output.to_str().ok_or("link output path must be UTF-8")?;
    Ok(match target {
        Target::WindowsX86_64 | Target::WindowsAarch64 => vec![format!("/out:{output}")],
        _ => vec!["-o".into(), output.into()],
    })
}

#[cfg(feature = "embedded")]
fn link_arguments(
    target: Target,
    kind: LinkKind,
    entry: Option<&str>,
) -> Result<Vec<String>, String> {
    if let Some(entry) = entry
        && (entry.is_empty()
            || !entry
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    {
        return Err("link entry must be a symbol name".into());
    }
    let args: &[&str] = match target {
        Target::LinuxX86_64 => &["ld.lld", "-m", "elf_x86_64", "--no-undefined"],
        Target::LinuxAarch64 => &["ld.lld", "-m", "aarch64linux", "--no-undefined"],
        Target::MacosX86_64 => &[
            "ld64.lld",
            "-arch",
            "x86_64",
            "-platform_version",
            "macos",
            "11.0",
            "11.0",
        ],
        Target::MacosAarch64 => &[
            "ld64.lld",
            "-arch",
            "arm64",
            "-platform_version",
            "macos",
            "11.0",
            "11.0",
        ],
        Target::WindowsX86_64 => &[
            "lld-link",
            "/machine:x64",
            "/nodefaultlib",
            "/subsystem:console",
        ],
        Target::WindowsAarch64 => &[
            "lld-link",
            "/machine:arm64",
            "/nodefaultlib",
            "/subsystem:console",
        ],
    };
    let mut args = args
        .iter()
        .map(|arg| (*arg).into())
        .collect::<Vec<String>>();
    match (target, kind) {
        (Target::LinuxX86_64 | Target::LinuxAarch64, LinkKind::StaticExecutable) => {
            args.extend([
                "-static".into(),
                "--gc-sections".into(),
                "--strip-all".into(),
            ]);
        }
        (_, LinkKind::StaticExecutable) => {
            return Err("static executable runtime packs currently require a Linux target".into());
        }
        (Target::LinuxX86_64 | Target::LinuxAarch64, LinkKind::Shared) => {
            args.push("-shared".into())
        }
        (Target::MacosX86_64 | Target::MacosAarch64, LinkKind::Shared) => {
            args.push("-dylib".into())
        }
        (Target::WindowsX86_64 | Target::WindowsAarch64, LinkKind::Shared) => {
            args.extend(["/dll".into(), "/noentry".into()]);
        }
        _ => {}
    }
    if let Some(entry) = entry {
        match target {
            Target::WindowsX86_64 | Target::WindowsAarch64 => args.push(format!("/entry:{entry}")),
            _ => args.extend(["-e".into(), entry.into()]),
        }
    }
    Ok(args)
}
