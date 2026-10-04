use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use http_body_util::BodyExt;
use hyper::body::{Body, Incoming};

use super::{InputError, Inputs, QueryValue};
use crate::config::{HttpSettings, UploadLimits};
use crate::http::Request;

tokio::task_local! { static BODY: RefCell<(Option<Incoming>, HttpSettings)>; }

pub(crate) async fn scope<T>(
    body: Incoming,
    settings: HttpSettings,
    future: impl Future<Output = T>,
) -> T {
    BODY.scope(RefCell::new((Some(body), settings)), future)
        .await
}

fn take_body() -> Result<Option<(Incoming, HttpSettings)>, InputError> {
    BODY.try_with(|state| {
        let mut state = state.borrow_mut();
        let body = state
            .0
            .take()
            .ok_or_else(|| invalid("request body was already consumed"))?;
        Ok((body, state.1.clone()))
    })
    .ok()
    .transpose()
}

/// Called only after route selection and authentication. Buffered unit-test requests
/// have no Incoming owner and already carry their bounded body.
pub async fn read_body(request: &mut Request) -> Result<(), InputError> {
    let Some((mut body, settings)) = take_body()? else {
        return Ok(());
    };
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| invalid("incomplete request body"))?;
        let chunk = frame
            .into_data()
            .map_err(|_| invalid("request trailers are not supported"))?;
        if chunk.len() > settings.limits.body_bytes as usize - bytes.len() {
            return Err(invalid("request body exceeds body_bytes"));
        }
        bytes.extend_from_slice(&chunk);
    }
    request.body = crate::bytes::Bytes::new(bytes);
    Ok(())
}

struct Temporary {
    path: PathBuf,
    file: Mutex<Option<std::fs::File>>,
}

impl Drop for Temporary {
    fn drop(&mut self) {
        // Close before unlinking, including on platforms that cannot unlink open files.
        self.file
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::log::error("upload temporary cleanup failed", Vec::new());
        }
    }
}

/// Non-cloneable request resource. The path and file are never exposed to source code.
pub struct Upload {
    temporary: Arc<Temporary>,
    filename: String,
    content_type: String,
    size: i64,
}

impl std::fmt::Debug for Upload {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output.write_str("Upload(<request resource>)")
    }
}

impl Upload {
    pub fn filename(&self) -> String {
        self.filename.clone()
    }
    pub fn content_type(&self) -> String {
        self.content_type.clone()
    }
    pub fn size(&self) -> i64 {
        self.size
    }
    pub fn close(self) {
        drop(self);
    }
}

struct PendingStore {
    path: Option<PathBuf>,
    file: Option<std::fs::File>,
    key: crate::orm::Uuid,
}

impl PendingStore {
    fn commit(mut self) -> crate::orm::Uuid {
        self.file.take();
        self.path.take();
        self.key
    }
}

impl Drop for PendingStore {
    fn drop(&mut self) {
        self.file.take();
        if let Some(path) = self.path.take()
            && let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::log::error("uncommitted upload storage cleanup failed", Vec::new());
        }
    }
}

/// Only generated storage Adapter calls reach this consuming sink. If cancellation
/// drops the blocking result before delivery, PendingStore removes its target too.
pub async fn store(upload: Upload) -> Result<crate::orm::Uuid, String> {
    let directory = BODY
        .try_with(|state| state.borrow().1.upload_directory.with_file_name("upload"))
        .map_err(|_| "upload storage requires a live API request".to_owned())?;
    let pending = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&directory)
            .map_err(|_| "cannot create upload storage directory")?;
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random).map_err(|_| "upload storage randomness unavailable")?;
        random[6] = (random[6] & 0x0f) | 0x40;
        random[8] = (random[8] & 0x3f) | 0x80;
        let key = crate::orm::Uuid::from_bytes(random);
        let path = directory.join(key.to_string());
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|_| "cannot create upload storage file")?;
        let mut pending = PendingStore {
            path: Some(path),
            file: Some(file),
            key,
        };
        let mut source = upload
            .temporary
            .file
            .lock()
            .map_err(|_| "upload source lock failed")?;
        let source = source.as_mut().ok_or("upload source is closed")?;
        source.rewind().map_err(|_| "cannot rewind upload source")?;
        let target = pending.file.as_mut().expect("pending target owns its file");
        std::io::copy(source, target).map_err(|_| "upload storage copy failed")?;
        target
            .sync_all()
            .map_err(|_| "upload storage flush failed")?;
        Ok::<_, String>(pending)
    })
    .await
    .map_err(|_| "upload storage worker failed".to_owned())??;
    Ok(pending.commit())
}

pub struct Multipart {
    inputs: Inputs,
    uploads: BTreeMap<String, Upload>,
}

impl Multipart {
    pub fn raw_json(&mut self, name: &str) -> Result<String, InputError> {
        self.inputs.raw_json(name)
    }
    pub fn optional_raw_json(&mut self, name: &str) -> Result<Option<String>, InputError> {
        self.inputs.optional_raw_json(name)
    }
    pub fn take(&mut self, name: &str) -> Result<Upload, InputError> {
        self.uploads
            .remove(name)
            .ok_or_else(|| invalid("missing upload field"))
    }
    pub fn json(&mut self, name: &str, kind: QueryValue) -> Result<String, InputError> {
        self.inputs.json(name, kind)
    }
    pub fn optional_json(&mut self, name: &str, kind: QueryValue) -> Result<String, InputError> {
        self.inputs.optional_json(name, kind)
    }
    pub fn finish(self) -> Result<(), InputError> {
        if !self.uploads.is_empty() {
            return Err(invalid("unknown upload field"));
        }
        self.inputs.finish()
    }
}

pub async fn read_multipart(request: &Request) -> Result<Multipart, InputError> {
    let Some((body, settings)) = take_body()? else {
        return Err(invalid("multipart requires an incoming HTTP body"));
    };
    if request.method != "POST" || request.target.contains('?') {
        return Err(invalid("multipart requires POST without query inputs"));
    }
    let mut types = request
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("content-type"));
    let content_type = types
        .next()
        .and_then(|header| std::str::from_utf8(header.value.values()).ok())
        .ok_or_else(|| invalid("missing multipart Content-Type"))?;
    if types.next().is_some() {
        return Err(invalid("duplicate Content-Type"));
    }
    decode(
        body,
        content_type,
        &settings.upload_directory,
        settings.upload,
    )
    .await
}

/// The same streaming decoder is used by the HTTP owner and bounded body tests.
pub async fn decode<B>(
    body: B,
    content_type: &str,
    directory: &Path,
    limits: UploadLimits,
) -> Result<Multipart, InputError>
where
    B: Body<Data = bytes::Bytes> + Unpin,
{
    limits.validate().map_err(InputError)?;
    let (media, mut parameters) = parameters(content_type)?;
    let boundary = parameters
        .remove("boundary")
        .ok_or_else(|| invalid("missing multipart boundary"))?;
    if !media.eq_ignore_ascii_case("multipart/form-data")
        || !parameters.is_empty()
        || boundary.is_empty()
        || boundary.len() > 70
        || !boundary
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"'-_.,+/:=?()".contains(&byte))
    {
        return Err(invalid("invalid multipart Content-Type"));
    }
    let mut reader = Reader {
        body,
        pending: bytes::Bytes::new(),
        buffer: Vec::new(),
        total: 0,
        limit: limits.total_bytes,
        ended: false,
    };
    reader.exact(format!("--{boundary}\r\n").as_bytes()).await?;
    let delimiter = format!("\r\n--{boundary}").into_bytes();
    let mut uploads = BTreeMap::new();
    let mut fields = BTreeMap::new();
    let mut names = std::collections::BTreeSet::new();
    for part in 0..limits.parts {
        let headers = reader.until(b"\r\n\r\n", limits.header_bytes).await?;
        let (name, filename, content_type) = part_headers(&headers, limits.filename_bytes)?;
        if !names.insert(name.clone()) {
            return Err(invalid("duplicate multipart field"));
        }
        let mut upload = match filename {
            Some(filename) => Some(create(directory.to_path_buf(), filename, content_type).await?),
            None => {
                if fields.len() >= limits.fields {
                    return Err(invalid("multipart fields exceed limit"));
                }
                None
            }
        };
        let mut field = Vec::new();
        let mut size = 0usize;
        loop {
            let (chunk, end) = reader.part_chunk(&delimiter).await?;
            let maximum = if upload.is_some() {
                limits.file_bytes
            } else {
                limits.field_bytes
            };
            if chunk.len() > maximum - size {
                return Err(invalid("multipart part exceeds byte limit"));
            }
            size += chunk.len();
            if let Some(upload) = &mut upload {
                let temporary = upload.temporary.clone();
                tokio::task::spawn_blocking(move || {
                    temporary
                        .file
                        .lock()
                        .map_err(|_| invalid("upload file lock failed"))?
                        .as_mut()
                        .ok_or_else(|| invalid("upload file closed"))?
                        .write_all(&chunk)
                        .map_err(|_| invalid("upload write failed"))
                })
                .await
                .map_err(|_| invalid("upload writer failed"))??;
                upload.size = size as i64;
            } else {
                field.extend_from_slice(&chunk);
            }
            if end {
                break;
            }
        }
        if let Some(upload) = upload {
            uploads.insert(name, upload);
        } else {
            let value =
                String::from_utf8(field).map_err(|_| invalid("multipart scalar is not UTF-8"))?;
            fields.insert(
                name,
                serde_json::to_string(&value)
                    .map_err(|_| invalid("multipart scalar encoding failed"))?,
            );
        }
        reader.fill(2).await?;
        if reader.buffer.starts_with(b"--") {
            reader.exact(b"--\r\n").await?;
            if reader.fill(1).await? {
                return Err(invalid("multipart epilogue is not supported"));
            }
            return Ok(Multipart {
                inputs: Inputs::from_fields(fields),
                uploads,
            });
        }
        reader.exact(b"\r\n").await?;
        if part + 1 == limits.parts {
            return Err(invalid("multipart parts exceed limit"));
        }
    }
    Err(invalid("incomplete multipart body"))
}

async fn create(
    directory: PathBuf,
    filename: String,
    content_type: String,
) -> Result<Upload, InputError> {
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&directory)
            .map_err(|_| invalid("cannot create upload directory"))?;
        let mut random = [0u8; 16];
        getrandom::getrandom(&mut random).map_err(|_| invalid("upload randomness unavailable"))?;
        let name = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = directory.join(format!("upload-{name}"));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|_| invalid("cannot create upload file"))?;
        Ok(Upload {
            temporary: Arc::new(Temporary {
                path,
                file: Mutex::new(Some(file)),
            }),
            filename,
            content_type,
            size: 0,
        })
    })
    .await
    .map_err(|_| invalid("upload owner failed"))?
}

fn invalid(message: &str) -> InputError {
    InputError(message.to_owned())
}

fn parameters(value: &str) -> Result<(&str, BTreeMap<String, String>), InputError> {
    let mut parts = value.split(';');
    let name = parts.next().unwrap_or("").trim();
    let mut parameters = BTreeMap::new();
    for part in parts {
        let (key, value) = part
            .trim()
            .split_once('=')
            .ok_or_else(|| invalid("invalid multipart parameter"))?;
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        if value
            .bytes()
            .any(|byte| byte < 32 || byte == 127 || byte == b'"' || byte == b'\\')
            || parameters
                .insert(key.trim().to_ascii_lowercase(), value.to_owned())
                .is_some()
        {
            return Err(invalid("invalid or duplicate multipart parameter"));
        }
    }
    Ok((name, parameters))
}

fn part_headers(
    bytes: &[u8],
    filename_limit: usize,
) -> Result<(String, Option<String>, String), InputError> {
    let headers = std::str::from_utf8(bytes).map_err(|_| invalid("invalid multipart headers"))?;
    let mut disposition = None;
    let mut content_type = None;
    for line in headers.split("\r\n") {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("invalid multipart header"))?;
        if value.bytes().any(|byte| byte < 32 || byte == 127) {
            return Err(invalid("invalid multipart header value"));
        }
        match name.to_ascii_lowercase().as_str() {
            "content-disposition" if disposition.is_none() => disposition = Some(value.trim()),
            "content-type" if content_type.is_none() => {
                content_type = Some(value.trim().to_owned())
            }
            _ => return Err(invalid("unsupported or duplicate multipart header")),
        }
    }
    let (kind, mut parameters) =
        parameters(disposition.ok_or_else(|| invalid("missing Content-Disposition"))?)?;
    if !kind.eq_ignore_ascii_case("form-data") {
        return Err(invalid("expected form-data disposition"));
    }
    let name = parameters
        .remove("name")
        .ok_or_else(|| invalid("missing multipart field name"))?;
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(invalid("invalid multipart field name"));
    }
    let filename = parameters.remove("filename");
    if !parameters.is_empty() {
        return Err(invalid("unsupported multipart disposition parameter"));
    }
    if filename.as_ref().is_some_and(|name| {
        name.is_empty()
            || name.len() > filename_limit
            || name == "."
            || name == ".."
            || name.contains(['/', '\\', ':'])
    }) {
        return Err(invalid("invalid upload filename"));
    }
    let content_type = content_type.unwrap_or_else(|| "application/octet-stream".into());
    Ok((name, filename, content_type))
}

struct Reader<B> {
    body: B,
    pending: bytes::Bytes,
    buffer: Vec<u8>,
    total: usize,
    limit: usize,
    ended: bool,
}

impl<B: Body<Data = bytes::Bytes> + Unpin> Reader<B> {
    async fn fill(&mut self, required: usize) -> Result<bool, InputError> {
        while self.buffer.len() < required && !self.ended {
            if self.pending.is_empty() {
                match self.body.frame().await {
                    Some(Ok(frame)) => {
                        self.pending = frame
                            .into_data()
                            .map_err(|_| invalid("multipart trailers are not supported"))?;
                        if self.pending.len() > self.limit - self.total {
                            return Err(invalid("multipart total exceeds byte limit"));
                        }
                        self.total += self.pending.len();
                    }
                    Some(Err(_)) => return Err(invalid("incomplete multipart body")),
                    None => {
                        self.ended = true;
                        continue;
                    }
                }
            }
            let count = self.pending.len().min(16_384);
            self.buffer.extend_from_slice(&self.pending.split_to(count));
        }
        Ok(self.buffer.len() >= required)
    }

    async fn exact(&mut self, expected: &[u8]) -> Result<(), InputError> {
        if !self.fill(expected.len()).await? || !self.buffer.starts_with(expected) {
            return Err(invalid("invalid multipart delimiter"));
        }
        self.buffer.drain(..expected.len());
        Ok(())
    }

    async fn until(&mut self, delimiter: &[u8], limit: usize) -> Result<Vec<u8>, InputError> {
        loop {
            if let Some(index) = self
                .buffer
                .windows(delimiter.len())
                .position(|part| part == delimiter)
            {
                if index > limit {
                    return Err(invalid("multipart headers exceed limit"));
                }
                let bytes = self.buffer.drain(..index).collect();
                self.buffer.drain(..delimiter.len());
                return Ok(bytes);
            }
            if self.buffer.len() > limit + delimiter.len()
                || !self.fill(self.buffer.len() + 1).await?
            {
                return Err(invalid("incomplete or oversized multipart headers"));
            }
        }
    }

    async fn part_chunk(&mut self, delimiter: &[u8]) -> Result<(Vec<u8>, bool), InputError> {
        loop {
            if self.buffer.len() <= delimiter.len() + 2 && !self.ended {
                self.fill(16_384).await?;
            }
            let mut cursor = 0;
            while let Some(offset) = self.buffer[cursor..]
                .windows(delimiter.len())
                .position(|part| part == delimiter)
            {
                let index = cursor + offset;
                self.fill(index + delimiter.len() + 4).await?;
                let suffix = &self.buffer[index + delimiter.len()..];
                if suffix.starts_with(b"--\r\n") || suffix.starts_with(b"\r\n") {
                    let chunk = self.buffer.drain(..index).collect();
                    self.buffer.drain(..delimiter.len());
                    return Ok((chunk, true));
                }
                cursor = index + 1;
            }
            if self.buffer.len() > delimiter.len() + 2 {
                let count = self.buffer.len() - delimiter.len() - 2;
                return Ok((self.buffer.drain(..count).collect(), false));
            }
            if !self.fill(self.buffer.len() + 1).await? {
                return Err(invalid("incomplete multipart file"));
            }
        }
    }
}
