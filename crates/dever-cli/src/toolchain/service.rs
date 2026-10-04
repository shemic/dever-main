//! The small authenticated boundary around the machine cache.
//!
//! `CacheStore` remains an on-disk engine owned by this process.  Clients use
//! this module's length-delimited local IPC instead of constructing the store
//! in the launcher.  Platform installers can replace the fixture socket with
//! the documented system endpoint without changing the request contract.

use std::collections::BTreeSet;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
#[cfg(unix)]
use std::sync::{Arc, Mutex, mpsc};

#[cfg(unix)]
use fs2::FileExt;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};

use super::cache::{ArtifactReceipt, CacheStatus, CacheStore};
use super::release::{Layout, Version};
use sha2::{Digest, Sha256};

#[cfg(unix)]
mod compile;

pub use self::compile_request::compile;

mod compile_request;

const TOKEN_BYTES: usize = 32;
const MAX_MESSAGE: usize = 1024 * 1024;
#[cfg(unix)]
const CLIENT_WORKERS: usize = 8;

#[cfg(unix)]
struct Client {
    stream: std::os::unix::net::UnixStream,
    caller: Option<u32>,
    #[cfg(target_os = "linux")]
    _admission: Option<ForeignAdmission>,
}

#[cfg(target_os = "linux")]
struct ForeignAdmission {
    uid: u32,
    users: Arc<Mutex<std::collections::BTreeMap<u32, usize>>>,
}

#[cfg(target_os = "linux")]
impl ForeignAdmission {
    fn acquire(
        users: &Arc<Mutex<std::collections::BTreeMap<u32, usize>>>,
        uid: u32,
    ) -> Option<Self> {
        let mut counts = users.lock().unwrap();
        // One stalled caller cannot fill the worker pool. Two service slots
        // stay available to the owner even when foreign users are saturated.
        if counts.values().sum::<usize>() >= CLIENT_WORKERS - 2
            || counts.get(&uid).copied().unwrap_or(0) >= 2
        {
            return None;
        }
        *counts.entry(uid).or_default() += 1;
        Some(Self {
            uid,
            users: Arc::clone(users),
        })
    }
}

#[cfg(target_os = "linux")]
impl Drop for ForeignAdmission {
    fn drop(&mut self) {
        let mut counts = self.users.lock().unwrap();
        if let Some(count) = counts.get_mut(&self.uid) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.uid);
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    token: String,
    operation: Operation,
    #[serde(default)]
    installed: Vec<Version>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Status,
    Clean,
    ArtifactPut(ArtifactReceipt),
    ArtifactGet(ArtifactReceipt),
    Compile { bytes: u64 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    ok: bool,
    status: Option<CacheStatus>,
    error: Option<String>,
    #[serde(default)]
    artifact: Option<ArtifactReceipt>,
}

/// Ensure a per-machine authentication token exists.  The token is not an
/// environment variable and is never accepted from a command line argument.
pub fn ensure_token(layout: &Layout) -> Result<(), String> {
    if layout.service_token().exists() {
        validate_token_file(layout)?;
        return Ok(());
    }
    let mut token = [0_u8; TOKEN_BYTES];
    SystemRandom::new()
        .fill(&mut token)
        .map_err(|_| "cannot generate deverd authentication token".to_owned())?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(layout.service_token())
        .map_err(|error| format!("cannot create deverd authentication token: {error}"))?;
    file.write_all(hex(&token).as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("cannot write deverd authentication token: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(layout.service_token(), fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("cannot protect deverd authentication token: {error}"))?;
    }
    Ok(())
}

/// Serve requests until the process is stopped.  This is intentionally a
/// local service primitive; OS service registration and privilege adapters
/// belong to the distribution layer.
pub fn serve(layout: &Layout) -> Result<(), String> {
    #[cfg(unix)]
    let _service_lock = acquire_service_lock(layout)?;
    ensure_token(layout)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::net::UnixListener;
        if let Some(parent) = layout.service_socket().parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create deverd state directory: {error}"))?;
        }
        match fs::symlink_metadata(layout.service_socket()) {
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() =>
            {
                return Err("deverd IPC endpoint must be a real Unix socket".into());
            }
            Ok(_) => fs::remove_file(layout.service_socket())
                .map_err(|error| format!("cannot replace deverd IPC endpoint: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect deverd IPC endpoint: {error}")),
        }
        let listener = UnixListener::bind(layout.service_socket())
            .map_err(|error| format!("cannot bind deverd IPC endpoint: {error}"))?;
        use std::os::unix::fs::PermissionsExt;
        // Linux obtains the calling uid from the kernel. The cache directories
        // remain private; only the socket is usable by other machine users.
        let mode = if cfg!(target_os = "linux") {
            0o666
        } else {
            0o600
        };
        fs::set_permissions(layout.service_socket(), fs::Permissions::from_mode(mode))
            .map_err(|error| format!("cannot protect deverd IPC endpoint: {error}"))?;
        let (sender, receiver) = mpsc::sync_channel(CLIENT_WORKERS);
        let receiver = Arc::new(Mutex::new(receiver));
        #[cfg(target_os = "linux")]
        let (owner, foreign_users) = {
            use std::os::unix::fs::MetadataExt;
            let owner = fs::symlink_metadata(layout.state())
                .map_err(|error| format!("cannot inspect deverd state: {error}"))?
                .uid();
            if owner != rustix::process::geteuid().as_raw() {
                return Err("deverd must run as the state directory owner".into());
            }
            (
                owner,
                Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            )
        };
        std::thread::scope(|scope| {
            for _ in 0..CLIENT_WORKERS {
                let receiver = Arc::clone(&receiver);
                scope.spawn(move || {
                    loop {
                        let next = { receiver.lock().unwrap().recv() };
                        match next {
                            Ok(client) => {
                                let Client {
                                    stream,
                                    caller,
                                    #[cfg(target_os = "linux")]
                                    _admission,
                                } = client;
                                serve_client(layout, stream, caller);
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
            let result = loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        #[cfg(target_os = "linux")]
                        let caller = match rustix::net::sockopt::socket_peercred(&stream) {
                            Ok(credentials) => Some(credentials.uid.as_raw()),
                            Err(_) => continue,
                        };
                        #[cfg(not(target_os = "linux"))]
                        let caller = None;
                        #[cfg(target_os = "linux")]
                        let admission = if caller != Some(owner) {
                            match ForeignAdmission::acquire(
                                &foreign_users,
                                caller.expect("checked kernel uid"),
                            ) {
                                Some(admission) => Some(admission),
                                None => continue,
                            }
                        } else {
                            None
                        };
                        let client = Client {
                            stream,
                            caller,
                            #[cfg(target_os = "linux")]
                            _admission: admission,
                        };
                        let _ = sender.try_send(client);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => break Err(format!("deverd IPC accept failed: {error}")),
                }
            };
            drop(sender);
            result
        })
    }
    #[cfg(not(unix))]
    {
        let _ = layout;
        Err("deverd authenticated IPC is not implemented for this platform yet".into())
    }
}

pub fn cache_status(layout: &Layout) -> Result<CacheStatus, String> {
    request(layout, Operation::Status, &[]).and_then(|response| {
        response.status.ok_or_else(|| {
            response
                .error
                .unwrap_or_else(|| "deverd returned no cache status".into())
        })
    })
}

pub fn cache_clean(layout: &Layout, installed: &BTreeSet<Version>) -> Result<CacheStatus, String> {
    request(
        layout,
        Operation::Clean,
        &installed.iter().cloned().collect::<Vec<_>>(),
    )
    .and_then(|response| {
        response.status.ok_or_else(|| {
            response
                .error
                .unwrap_or_else(|| "deverd returned no cache status".into())
        })
    })
}

pub fn artifact_put(layout: &Layout, bytes: &[u8]) -> Result<ArtifactReceipt, String> {
    let expected = ArtifactReceipt {
        sha256: hex(&Sha256::digest(bytes)),
        bytes: bytes.len() as u64,
    };
    expected.validate()?;
    #[cfg(unix)]
    {
        let (mut stream, token) = connect(layout)?;
        write_frame(
            &mut stream,
            &Request {
                token,
                operation: Operation::ArtifactPut(expected.clone()),
                installed: Vec::new(),
            },
        )?;
        require_artifact(read_frame(&mut stream)?, &expected)?;
        stream
            .write_all(bytes)
            .map_err(|error| format!("deverd artifact upload failed: {error}"))?;
        require_artifact(read_frame(&mut stream)?, &expected)?;
        Ok(expected)
    }
    #[cfg(not(unix))]
    {
        let _ = layout;
        Err("deverd artifact IPC is not implemented for this platform".into())
    }
}

pub fn artifact_get(layout: &Layout, sha256: &str, bytes: u64) -> Result<Vec<u8>, String> {
    let expected = ArtifactReceipt {
        sha256: sha256.to_owned(),
        bytes,
    };
    expected.validate()?;
    #[cfg(unix)]
    {
        let (mut stream, token) = connect(layout)?;
        write_frame(
            &mut stream,
            &Request {
                token,
                operation: Operation::ArtifactGet(expected.clone()),
                installed: Vec::new(),
            },
        )?;
        require_artifact(read_frame(&mut stream)?, &expected)?;
        let mut bytes = vec![0; expected.bytes as usize];
        stream
            .read_exact(&mut bytes)
            .map_err(|error| format!("deverd artifact download is incomplete: {error}"))?;
        if hex(&Sha256::digest(&bytes)) != expected.sha256 {
            return Err("deverd artifact download failed SHA-256 verification".into());
        }
        Ok(bytes)
    }
    #[cfg(not(unix))]
    {
        let _ = layout;
        Err("deverd artifact IPC is not implemented for this platform".into())
    }
}

fn require_artifact(response: Response, expected: &ArtifactReceipt) -> Result<(), String> {
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "deverd artifact request failed".into()));
    }
    if response.artifact.as_ref() != Some(expected) {
        return Err("deverd returned a different artifact identity".into());
    }
    Ok(())
}

#[cfg(unix)]
fn connect(layout: &Layout) -> Result<(std::os::unix::net::UnixStream, String), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::os::unix::net::UnixStream;

    let state = fs::symlink_metadata(layout.state())
        .map_err(|error| format!("cannot inspect deverd state directory: {error}"))?;
    if !state.is_dir() || state.file_type().is_symlink() || state.mode() & 0o022 != 0 {
        return Err("deverd state directory must not be writable by other users".into());
    }
    let endpoint = fs::symlink_metadata(layout.service_socket())
        .map_err(|error| format!("authenticated deverd cache service unavailable: {error}"))?;
    if !endpoint.file_type().is_socket() || endpoint.uid() != state.uid() {
        return Err("deverd IPC endpoint does not belong to the service owner".into());
    }
    let stream = UnixStream::connect(layout.service_socket())
        .map_err(|error| format!("authenticated deverd cache service unavailable: {error}"))?;
    #[cfg(target_os = "linux")]
    if rustix::net::sockopt::socket_peercred(&stream)
        .map_err(|error| format!("cannot authenticate deverd OS peer: {error}"))?
        .uid
        .as_raw()
        != state.uid()
    {
        return Err("deverd OS peer does not match the service owner".into());
    }
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .and_then(|()| stream.set_write_timeout(Some(std::time::Duration::from_secs(5))))
        .map_err(|error| format!("cannot configure deverd IPC timeout: {error}"))?;
    #[cfg(target_os = "linux")]
    if rustix::process::geteuid().as_raw() != state.uid() {
        return Ok((stream, String::new()));
    }
    validate_token_file(layout)?;
    let token = String::from_utf8(read_file(&layout.service_token())?)
        .map_err(|_| "invalid deverd authentication token".to_owned())?;
    Ok((stream, token.trim().to_owned()))
}

fn request(
    layout: &Layout,
    operation: Operation,
    installed: &[Version],
) -> Result<Response, String> {
    #[cfg(unix)]
    {
        let (mut stream, token) = connect(layout)?;
        let request = Request {
            token,
            operation,
            installed: installed.to_owned(),
        };
        write_frame(&mut stream, &request)?;
        read_frame(&mut stream)
    }
    #[cfg(not(unix))]
    {
        let _ = (layout, operation, installed);
        Err(
            "authenticated deverd cache service unavailable: platform IPC is not implemented"
                .into(),
        )
    }
}

#[cfg(unix)]
fn handle(
    layout: &Layout,
    stream: &mut std::os::unix::net::UnixStream,
    caller: Option<u32>,
) -> Response {
    let request: Result<Request, _> = read_frame(stream);
    let request = match request {
        Ok(request) => request,
        Err(error) => return failure(error),
    };
    if let Err(error) = validate_token_file(layout) {
        return failure(error);
    }
    let bytes = match read_file(&layout.service_token()) {
        Ok(bytes) => bytes,
        Err(error) => return failure(error),
    };
    let expected = match String::from_utf8(bytes) {
        Ok(token) => token.trim().as_bytes().to_vec(),
        Err(_) => return failure("invalid deverd authentication token".into()),
    };
    #[cfg(target_os = "linux")]
    let foreign = match caller {
        Some(uid) => {
            use std::os::unix::fs::MetadataExt;
            match fs::symlink_metadata(layout.state()) {
                Ok(state) => uid != state.uid(),
                Err(error) => return failure(format!("cannot inspect deverd state: {error}")),
            }
        }
        None => return failure("deverd OS caller credentials are unavailable".into()),
    };
    #[cfg(not(target_os = "linux"))]
    let foreign = {
        let _ = caller;
        false
    };
    if foreign {
        if matches!(request.operation, Operation::Clean) {
            return failure("deverd cache clean requires the service owner".into());
        }
    } else if !constant_time_equal(request.token.as_bytes(), &expected) {
        return failure("deverd authentication failed".into());
    }
    let store = CacheStore::new(layout);
    let result = match request.operation {
        Operation::Status => {
            if foreign {
                store.summary()
            } else {
                store.status()
            }
        }
        Operation::Clean => {
            if request.installed.len() > 256 {
                return failure("too many installed Dever versions".into());
            }
            store.clean(&request.installed.into_iter().collect())
        }
        Operation::ArtifactPut(expected) => {
            if let Err(error) = expected.validate() {
                return failure(error);
            }
            if let Err(error) = write_frame(stream, &artifact_response(expected.clone())) {
                return failure(error);
            }
            let mut input = TransferReader {
                input: stream,
                started: std::time::Instant::now(),
            };
            return match store.publish_artifact(&expected, &mut input) {
                Ok(receipt) => artifact_response(receipt),
                Err(error) => failure(error),
            };
        }
        Operation::ArtifactGet(expected) => {
            let mut artifact = match store.artifact(&expected) {
                Ok(artifact) => artifact,
                Err(error) => return failure(error),
            };
            if let Err(error) = write_frame(stream, &artifact_response(artifact.receipt)) {
                return failure(error);
            }
            if let Err(error) = std::io::copy(&mut artifact.file, stream) {
                return failure(format!("deverd artifact transfer failed: {error}"));
            }
            return Response {
                ok: true,
                status: None,
                artifact: None,
                error: None,
            };
        }
        Operation::Compile { bytes } => {
            return match compile::serve(layout, stream, bytes) {
                Ok(()) => Response {
                    ok: true,
                    status: None,
                    artifact: None,
                    error: None,
                },
                Err(error) => failure(error),
            };
        }
    };
    match result {
        Ok(status) => Response {
            ok: true,
            status: Some(status),
            error: None,
            artifact: None,
        },
        Err(error) => failure(error),
    }
}

fn failure(error: String) -> Response {
    Response {
        ok: false,
        status: None,
        error: Some(error),
        artifact: None,
    }
}

fn artifact_response(artifact: ArtifactReceipt) -> Response {
    Response {
        ok: true,
        status: None,
        error: None,
        artifact: Some(artifact),
    }
}

struct TransferReader<'a, R> {
    input: &'a mut R,
    started: std::time::Instant,
}

impl<R: Read> Read for TransferReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.started.elapsed() >= std::time::Duration::from_secs(60) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "deverd artifact transfer deadline exceeded",
            ));
        }
        self.input.read(buffer)
    }
}

#[cfg(unix)]
fn serve_client(layout: &Layout, mut stream: std::os::unix::net::UnixStream, caller: Option<u32>) {
    if stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .and_then(|()| stream.set_write_timeout(Some(std::time::Duration::from_secs(5))))
        .is_err()
    {
        return;
    }
    let response = handle(layout, &mut stream, caller);
    let _ = write_frame(&mut stream, &response);
}

#[cfg(unix)]
fn acquire_service_lock(layout: &Layout) -> Result<File, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let path = layout.state().join("deverd.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            validate_owned_file(layout, &path, "deverd lock")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            options.create_new(true).mode(0o600);
        }
        Err(error) => return Err(format!("cannot inspect deverd lock: {error}")),
    }
    let file = options
        .open(&path)
        .map_err(|error| format!("cannot open deverd lock: {error}"))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("cannot inspect deverd lock: {error}"))?;
    let current = validate_owned_file(layout, &path, "deverd lock")?;
    if opened.dev() != current.dev() || opened.ino() != current.ino() {
        return Err("deverd lock changed while opening".into());
    }
    FileExt::try_lock_exclusive(&file).map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            "deverd is already running for this machine root".into()
        } else {
            format!("cannot lock deverd state: {error}")
        }
    })?;
    Ok(file)
}

fn validate_token_file(layout: &Layout) -> Result<(), String> {
    let path = layout.service_token();
    validate_owned_file(layout, &path, "deverd authentication token")?;
    let token = read_file(&path)?;
    let token = token.strip_suffix(b"\n").unwrap_or(&token);
    if token.len() != TOKEN_BYTES * 2
        || !token
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("invalid deverd authentication token".into());
    }
    Ok(())
}

fn validate_owned_file(layout: &Layout, path: &Path, name: &str) -> Result<fs::Metadata, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("cannot inspect {name}: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("{name} must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let state = fs::symlink_metadata(layout.state())
            .map_err(|error| format!("cannot inspect deverd state directory: {error}"))?;
        if !state.is_dir() || state.uid() != metadata.uid() {
            return Err(format!("{name} must have the state directory owner"));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(format!("{name} must be accessible only to its owner"));
        }
    }
    Ok(metadata)
}

fn write_frame<T: Serialize>(stream: &mut impl Write, value: &T) -> Result<(), String> {
    let payload =
        serde_json::to_vec(value).map_err(|error| format!("deverd IPC encode failed: {error}"))?;
    if payload.len() > MAX_MESSAGE {
        return Err("deverd IPC message is too large".into());
    }
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .and_then(|_| stream.write_all(&payload))
        .and_then(|_| stream.flush())
        .map_err(|error| format!("deverd IPC write failed: {error}"))
}

fn read_frame<T: for<'de> Deserialize<'de>>(stream: &mut impl Read) -> Result<T, String> {
    let mut length = [0_u8; 4];
    stream
        .read_exact(&mut length)
        .map_err(|error| format!("deverd IPC read failed: {error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err("deverd IPC message is too large".into());
    }
    let mut payload = vec![0; length];
    stream
        .read_exact(&mut payload)
        .map_err(|error| format!("deverd IPC read failed: {error}"))?;
    let text =
        std::str::from_utf8(&payload).map_err(|_| "deverd IPC payload must be UTF-8 JSON")?;
    dever_runtime::wire::parse(text)
        .map_err(|error| format!("deverd IPC decode failed: {error}"))?;
    serde_json::from_slice(&payload).map_err(|error| format!("deverd IPC decode failed: {error}"))
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("cannot read '{}': {error}", path.display()))
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
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
