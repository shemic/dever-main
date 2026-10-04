//! Only this service path may publish compiled entries. Artifact uploads remain
//! opaque bytes and can never satisfy a compilation lookup.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::super::cache::{ArtifactReceipt, BuildIdentity, CacheStore, MAX_COMPILED_BYTES};
use super::super::compilation::{
    CompileRequest, MAX_COMPILE_BYTES, WORKER_OUTPUT_FILE, WORKER_REQUEST_FILE,
};
use super::super::release::{InstalledCompiler, Layout, MachineManager, Version};
use super::{Response, artifact_response, hex, write_frame};

const COMPILE_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_DIAGNOSTIC_BYTES: u64 = 64 * 1024;
static ACTIVE_COMPILATIONS: AtomicUsize = AtomicUsize::new(0);

struct Admission;

impl Admission {
    fn acquire() -> Result<Self, String> {
        ACTIVE_COMPILATIONS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 2).then_some(count + 1)
            })
            .map_err(|_| "deverd compilation capacity is busy; retry later".to_owned())?;
        Ok(Self)
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        ACTIVE_COMPILATIONS.fetch_sub(1, Ordering::Release);
    }
}

pub(super) fn serve(layout: &Layout, stream: &mut UnixStream, bytes: u64) -> Result<(), String> {
    if bytes == 0 || bytes > MAX_COMPILE_BYTES as u64 {
        return Err("deverd compilation request exceeds its transfer limit".into());
    }
    let _admission = Admission::acquire()?;
    let started = Instant::now();
    let store = CacheStore::new(layout);
    let _operation = store.operation()?;
    write_frame(
        stream,
        &Response {
            ok: true,
            status: None,
            artifact: None,
            error: None,
        },
    )?;
    let mut payload = vec![0; bytes as usize];
    super::TransferReader {
        input: stream,
        started,
    }
    .read_exact(&mut payload)
    .map_err(|error| format!("incomplete compilation request: {error}"))?;
    let request = CompileRequest::decode(&payload)?;
    let version = Version::parse(&request.version)?;
    let target = request.target;
    // Canonical bytes own the cache identity; decoded interpreter copies are no
    // longer needed by the service while the worker rereads its private request.
    drop(request);
    let compiler = MachineManager::new(layout.clone()).compilation(&version)?;
    verify_pack(&compiler, target)?;
    let identity = BuildIdentity {
        version: &version,
        compiler: compiler.identity.as_bytes(),
        runtime: compiler.identity.as_bytes(),
        target: target.platform(),
        generated: &payload,
        options: b"dever-trusted-compilation-v1",
        environment: b"",
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(1)))
        .map_err(|error| format!("cannot monitor compilation caller: {error}"))?;
    require_connected(stream, started)?;
    if let Some(artifact) = store.restore(&identity)? {
        return send_output(stream, artifact.as_bytes(), started);
    }
    let directory = store.compilation_directory()?;
    write_private(&directory.0.join(WORKER_REQUEST_FILE), &payload)?;
    run_worker(&compiler.core, &directory.0, stream, started)?;
    require_connected(stream, started)?;
    let output = directory.0.join(WORKER_OUTPUT_FILE);
    let metadata = fs::symlink_metadata(&output)
        .map_err(|error| format!("compiler produced no executable: {error}"))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > MAX_COMPILED_BYTES
    {
        return Err("compiler output must be a regular executable of at most 320 MiB".into());
    }
    store.publish(&identity, &output)?;
    let artifact = store
        .restore(&identity)?
        .ok_or("compiled output was not published")?;
    send_output(stream, artifact.as_bytes(), started)
}

fn verify_pack(
    compiler: &InstalledCompiler,
    target: super::super::BuildTarget,
) -> Result<(), String> {
    let root = compiler
        .core
        .parent()
        .ok_or("installed compiler has no directory")?;
    let artifacts =
        super::super::runtime_pack::validate(root, compiler.manifest.version.as_str(), target)?;
    for input in artifacts {
        if !compiler.manifest.artifacts.iter().any(|signed| {
            signed.path == input.path
                && signed.bytes == input.bytes
                && signed.sha256 == input.sha256
        }) {
            return Err(format!(
                "native runtime input '{}' is not covered by the signed release",
                input.path
            ));
        }
    }
    // This is the embedded compiler's fixed Linux LLVM dependency, not a PATH lookup.
    if cfg!(target_os = "linux")
        && !compiler
            .manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "lib/libLLVM.so.18.1")
    {
        return Err("signed release is missing lib/libLLVM.so.18.1".into());
    }
    Ok(())
}

fn send_output(stream: &mut UnixStream, bytes: &[u8], started: Instant) -> Result<(), String> {
    let receipt = ArtifactReceipt {
        sha256: hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref()),
        bytes: bytes.len() as u64,
    };
    receipt.validate_compiled()?;
    let timeout = || {
        COMPILE_TIMEOUT
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .map(|remaining| remaining.min(Duration::from_secs(5)))
            .ok_or_else(|| "deverd compilation transfer deadline exceeded".to_owned())
    };
    stream
        .set_write_timeout(Some(timeout()?))
        .map_err(|error| error.to_string())?;
    write_frame(stream, &artifact_response(receipt))?;
    let mut remaining = bytes;
    while !remaining.is_empty() {
        stream
            .set_write_timeout(Some(timeout()?))
            .map_err(|error| error.to_string())?;
        match stream.write(&remaining[..remaining.len().min(64 * 1024)]) {
            Ok(0) => return Err("compiled output transfer closed".into()),
            Ok(written) => remaining = &remaining[written..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("compiled output transfer failed: {error}")),
        }
    }
    Ok(())
}

fn require_connected(stream: &mut UnixStream, started: Instant) -> Result<(), String> {
    if started.elapsed() >= COMPILE_TIMEOUT {
        return Err("deverd compilation deadline exceeded".into());
    }
    let mut signal = [0_u8; 1];
    match stream.read(&mut signal) {
        // Any byte after the exact request body explicitly cancels this request.
        Ok(_) => Err("deverd compilation cancelled by caller".into()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!("deverd compilation caller disconnected: {error}")),
    }
}

struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_worker(
    core: &Path,
    directory: &Path,
    stream: &mut UnixStream,
    started: Instant,
) -> Result<(), String> {
    let diagnostic = directory.join("diagnostic");
    let error_output = private_file(&diagnostic)?;
    let mut worker = Worker(
        Command::new(core)
            .arg("--dever-compile-worker")
            .env_clear()
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(error_output)
            .spawn()
            .map_err(|error| format!("cannot start trusted compile worker: {error}"))?,
    );
    loop {
        require_connected(stream, started)?;
        if fs::metadata(&diagnostic)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_DIAGNOSTIC_BYTES
        {
            return Err("compile worker diagnostic exceeded 64 KiB".into());
        }
        if let Some(status) = worker
            .0
            .try_wait()
            .map_err(|error| format!("cannot wait for compile worker: {error}"))?
        {
            if status.success() {
                return Ok(());
            }
            let mut bytes = Vec::new();
            File::open(&diagnostic)
                .map_err(|error| error.to_string())?
                .take(MAX_DIAGNOSTIC_BYTES)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            return Err(format!(
                "trusted compile worker failed ({status}): {}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn private_file(path: &Path) -> Result<File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("cannot create compilation input: {error}"))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    private_file(path)?
        .write_all(bytes)
        .map_err(|error| format!("cannot write compilation input: {error}"))
}
