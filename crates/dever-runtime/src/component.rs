//! Supervised process boundary for external Adapter implementations.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

mod command;

const VERSION: &str = "dever-component-1";
const COMMAND_CAPACITY: usize = 64;
const START_LIMIT: usize = 3;
const START_WINDOW: Duration = Duration::from_secs(10);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct Definition {
    pub key: String,
    pub ecosystem: String,
    pub entry: String,
    pub port: String,
    pub adapter: String,
    pub schema: String,
    pub capabilities: Vec<String>,
    pub operations: Vec<String>,
    pub setting: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug)]
pub enum Reply {
    Result(String),
    Error { identity: String, payload: String },
}

struct Managed {
    definition: Definition,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    sender: Option<mpsc::Sender<Request>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<crate::task::Task<()>>,
    starts: VecDeque<Instant>,
    stopping: bool,
}

struct Request {
    operation: String,
    payload: String,
    response: oneshot::Sender<Result<Reply, String>>,
    command_lease: Option<command::RequestLease>,
}

tokio::task_local! {
    static CURRENT: Option<Arc<Session>>;
}

/// Worker handles belong to one runtime invocation, including partial startup.
pub(crate) struct Session {
    scope: crate::task::Scope,
    registry: Mutex<Registry>,
    shutdown: Mutex<()>,
    pub(crate) settings: std::sync::OnceLock<Result<Arc<crate::config::Settings>, String>>,
    #[cfg(feature = "external")]
    pub(crate) resources: std::sync::OnceLock<crate::external::PreparedBundle>,
}

#[derive(Default)]
struct Registry {
    workers: BTreeMap<String, Arc<Managed>>,
    stopping: bool,
    closed: bool,
}

impl Session {
    pub(crate) fn new(scope: crate::task::Scope) -> Arc<Self> {
        Arc::new(Self {
            scope,
            registry: Mutex::new(Registry::default()),
            shutdown: Mutex::new(()),
            settings: std::sync::OnceLock::new(),
            #[cfg(feature = "external")]
            resources: std::sync::OnceLock::new(),
        })
    }
}

pub(crate) fn current() -> Option<Arc<Session>> {
    CURRENT.try_with(Clone::clone).ok().flatten()
}

fn session() -> Result<Arc<Session>, String> {
    current().ok_or_else(|| "external Adapter requires a Dever invocation".into())
}

/// One settings snapshot owns both Adapter selection and deployment grants.
pub fn adapter_settings() -> Result<Arc<crate::config::Settings>, String> {
    #[cfg(feature = "api")]
    if let Some(application) = crate::application::current() {
        return Ok(application.settings().clone());
    }
    session()?
        .settings
        .get_or_init(|| crate::config::Settings::load_adapter_settings().map(Arc::new))
        .clone()
}

pub(crate) fn scope_resources<T>(
    session: Option<Arc<Session>>,
    future: impl Future<Output = T>,
) -> impl Future<Output = T> {
    CURRENT.scope(session, future)
}

pub(crate) fn with_resources<T>(session: Option<Arc<Session>>, apply: impl FnOnce() -> T) -> T {
    CURRENT.sync_scope(session, apply)
}

pub async fn start(definition: Definition) -> Result<(), String> {
    validate_definition(&definition)?;
    let session = session()?;
    let mut registered = session.registry.lock().await;
    if registered.stopping || registered.closed {
        return Err("external Adapter invocation is stopped".into());
    }
    if registered.workers.contains_key(&definition.key) {
        return Err("external Adapter initialized twice".into());
    }
    let managed = Arc::new(Managed {
        definition,
        state: Mutex::new(State::default()),
    });
    // Register before awaiting startup so cancellation cannot orphan the owner.
    registered
        .workers
        .insert(managed.definition.key.clone(), managed.clone());
    drop(registered);
    managed.ensure_started().await?;
    Ok(())
}

pub async fn call(
    key: &str,
    operation: &str,
    payload: &crate::wire::Encoded,
) -> Result<Reply, String> {
    let session = session()?;
    let registered = session.registry.lock().await;
    if registered.stopping || registered.closed {
        return Err("external Adapter invocation is stopped".into());
    }
    let managed = registered
        .workers
        .get(key)
        .cloned()
        .ok_or_else(|| "external Adapter is not initialized".to_owned())?;
    drop(registered);
    if !managed
        .definition
        .operations
        .iter()
        .any(|name| name == operation)
    {
        return Err("external Adapter operation is not declared by its Port".into());
    }
    for _ in 0..2 {
        let sender = managed.ensure_started().await?;
        let (response, result) = oneshot::channel();
        let lease = (managed.definition.ecosystem == "command")
            .then(command::call_lease)
            .transpose()?;
        let request = Request {
            operation: operation.to_owned(),
            payload: payload.as_str().to_owned(),
            response,
            command_lease: lease.as_ref().map(|lease| lease.request()),
        };
        if sender.send(request).await.is_ok() {
            return result
                .await
                .unwrap_or_else(|_| Err("external Adapter worker stopped".into()));
        }
    }
    Err("external Adapter worker is unavailable".into())
}

pub async fn shutdown() -> Result<(), String> {
    shutdown_session(false).await
}

pub(crate) async fn close() -> Result<(), String> {
    shutdown_session(true).await
}

async fn shutdown_session(terminal: bool) -> Result<(), String> {
    let session = session()?;
    let _shutdown = session.shutdown.lock().await;
    let managed = {
        let mut registered = session.registry.lock().await;
        registered.stopping = true;
        registered.closed |= terminal;
        registered.workers.values().cloned().collect::<Vec<_>>()
    };
    let mut first = None;
    for component in managed {
        if let Err(error) = component.stop().await {
            first = Some(match first {
                Some(previous) => format!("{previous}; worker cleanup: {error}"),
                None => error,
            });
        }
        session
            .registry
            .lock()
            .await
            .workers
            .remove(&component.definition.key);
    }
    if !terminal {
        session.registry.lock().await.stopping = false;
    }
    first.map_or(Ok(()), Err)
}

impl Managed {
    async fn ensure_started(&self) -> Result<mpsc::Sender<Request>, String> {
        let mut state = self.state.lock().await;
        if state.stopping {
            return Err("external Adapter worker is stopped".into());
        }
        if let Some(sender) = &state.sender
            && !sender.is_closed()
        {
            return Ok(sender.clone());
        }
        if state.task.is_some() {
            let _ = crate::task::wait_resource(&mut state.task).await;
        }
        state.sender = None;
        state.stop = None;
        if self.definition.ecosystem != "command" {
            let now = Instant::now();
            while state
                .starts
                .front()
                .is_some_and(|started| now.duration_since(*started) >= START_WINDOW)
            {
                state.starts.pop_front();
            }
            if state.starts.len() == START_LIMIT {
                return Err("external Adapter worker restart limit exceeded".into());
            }
            state.starts.push_back(now);
        }
        let (sender, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (stop, stopping) = oneshot::channel();
        let (ready, started) = oneshot::channel();
        let definition = self.definition.clone();
        let session = session()?;
        let task = crate::task::run_in(&session.scope, async move {
            supervise(definition, receiver, stopping, ready).await
        })?;
        state.stop = Some(stop);
        state.task = Some(task);
        match started.await {
            Ok(Ok(())) => {
                state.sender = Some(sender.clone());
                Ok(sender)
            }
            Ok(Err(error)) => {
                let cleanup = crate::task::wait_resource(&mut state.task).await;
                state.stop = None;
                match cleanup {
                    Err(cleanup) if cleanup != error => {
                        Err(format!("{error}; worker cleanup: {cleanup}"))
                    }
                    _ => Err(error),
                }
            }
            Err(_) => crate::task::wait_resource(&mut state.task)
                .await
                .and_then(|_| Err("external Adapter worker stopped during startup".into())),
        }
    }

    async fn stop(&self) -> Result<(), String> {
        let mut state = self.state.lock().await;
        state.stopping = true;
        state.sender = None;
        if let Some(stop) = state.stop.take() {
            let _ = stop.send(());
        }
        // Keep the task in its owner if an explicit shutdown caller is cancelled.
        crate::task::wait_resource(&mut state.task).await
    }
}

fn validate_definition(definition: &Definition) -> Result<(), String> {
    if !matches!(
        definition.ecosystem.as_str(),
        "exec" | "pip" | "npm" | "go" | "command"
    ) {
        return Err("invalid external Adapter ecosystem".into());
    }
    if definition.key.is_empty()
        || definition.port.is_empty()
        || definition.adapter.is_empty()
        || definition.schema.is_empty()
        || definition.operations.is_empty()
        || definition.timeout_ms == 0
    {
        return Err("incomplete external Adapter definition".into());
    }
    unique(&definition.capabilities, "capability")?;
    if definition
        .capabilities
        .iter()
        .any(|capability| !matches!(capability.as_str(), "network" | "file" | "process" | "gpu"))
    {
        return Err("external Adapter declares an unknown capability".into());
    }
    unique(&definition.operations, "operation")?;
    if definition.ecosystem == "command"
        && (definition.operations.len() != 1 || definition.setting.is_some())
    {
        return Err("command Adapter requires one operation and no setting".into());
    }
    Ok(())
}

fn unique(values: &[String], label: &str) -> Result<(), String> {
    let mut found = BTreeSet::new();
    if values
        .iter()
        .any(|value| value.is_empty() || !found.insert(value))
    {
        return Err(format!(
            "external Adapter has an invalid or duplicate {label}"
        ));
    }
    Ok(())
}

async fn supervise(
    definition: Definition,
    receiver: mpsc::Receiver<Request>,
    stopping: oneshot::Receiver<()>,
    ready: oneshot::Sender<Result<(), String>>,
) -> Result<(), String> {
    if definition.ecosystem == "command" {
        return command::supervise(definition, receiver, stopping, ready).await;
    }
    let mut command = worker_command(&definition)?;
    command
        .arg("--dever-component")
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot start external Adapter worker: {error}"))?;
    let Some(stdin) = child.stdin.take() else {
        terminate_child(&mut child).await?;
        return Err("external Adapter worker has no stdin".into());
    };
    let Some(stdout) = child.stdout.take() else {
        terminate_child(&mut child).await?;
        return Err("external Adapter worker has no stdout".into());
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_child(&mut child).await?;
        return Err("external Adapter worker has no stderr".into());
    };
    let protocol = protocol_loop(
        definition, &mut child, stdin, stdout, receiver, stopping, ready,
    );
    let logs = drain_stderr(stderr);
    let (protocol, logs) = tokio::join!(protocol, logs);
    protocol.and(logs)
}

fn worker_command(definition: &Definition) -> Result<Command, String> {
    #[cfg(feature = "external")]
    {
        crate::external::worker_command(definition).map(Command::from)
    }
    #[cfg(not(feature = "external"))]
    {
        let _ = definition;
        Err("external Worker sandbox runtime feature is not enabled".into())
    }
}

async fn protocol_loop(
    definition: Definition,
    child: &mut Child,
    mut input: ChildStdin,
    mut output: ChildStdout,
    mut requests: mpsc::Receiver<Request>,
    mut stopping: oneshot::Receiver<()>,
    ready: oneshot::Sender<Result<(), String>>,
) -> Result<(), String> {
    let protocol = protocol_session(
        &definition,
        &mut input,
        &mut output,
        &mut requests,
        &mut stopping,
        ready,
    )
    .await;
    drop(input);
    drop(output);
    match protocol {
        Ok(()) => wait_for_exit(child).await,
        Err(error) => match terminate_child(child).await {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}; worker cleanup failed: {cleanup}")),
        },
    }
}

async fn protocol_session(
    definition: &Definition,
    input: &mut ChildStdin,
    output: &mut ChildStdout,
    requests: &mut mpsc::Receiver<Request>,
    stopping: &mut oneshot::Receiver<()>,
    ready: oneshot::Sender<Result<(), String>>,
) -> Result<(), String> {
    let startup = handshake(definition, input, output).await;
    if let Err(error) = startup {
        let _ = ready.send(Err(error.clone()));
        return Err(error);
    }
    let _ = ready.send(Ok(()));
    let mut next_id = 1_u64;
    loop {
        let request = tokio::select! {
            biased;
            _ = &mut *stopping => return shutdown_protocol(input, output).await,
            request = requests.recv() => request,
        };
        let Some(mut request) = request else {
            return shutdown_protocol(input, output).await;
        };
        let id = next_id;
        next_id = next_id
            .checked_add(1)
            .ok_or_else(|| "external Adapter request id exhausted".to_owned())?;
        if request.response.is_closed() {
            continue;
        }
        let message = call_message(id, &request.operation, &request.payload)?;
        tokio::select! {
            biased;
            _ = &mut *stopping => {
                let _ = request.response.send(Err("external Adapter worker stopped".into()));
                return Err("external Adapter worker stopped while writing a call".into());
            }
            result = write_frame_timeout(input, &message, Duration::from_millis(definition.timeout_ms)) => result?,
        }
        let timeout = tokio::time::sleep(Duration::from_millis(definition.timeout_ms));
        tokio::pin!(timeout);
        let result = tokio::select! {
            biased;
            _ = &mut *stopping => {
                let _ = request.response.send(Err("external Adapter worker stopped".into()));
                cancel_request(input, output, id).await?;
                return shutdown_protocol(input, output).await;
            }
            frame = read_frame(output) => frame.and_then(|frame| parse_reply(frame, id)),
            _ = request.response.closed() => {
                cancel_request(input, output, id).await?;
                continue;
            }
            _ = &mut timeout => {
                let _ = request.response.send(Err("external Adapter call timed out".into()));
                cancel_request(input, output, id).await?;
                continue;
            }
        };
        match result {
            Ok(reply) => {
                let _ = request.response.send(Ok(reply));
            }
            Err(error) => {
                requests.close();
                let _ = request.response.send(Err(error.clone()));
                return Err(error);
            }
        }
    }
}

async fn cancel_request(
    input: &mut ChildStdin,
    output: &mut ChildStdout,
    id: u64,
) -> Result<(), String> {
    write_frame_timeout(input, &id_message("cancel", id)?, SHUTDOWN_GRACE).await?;
    let _ = parse_reply(read_frame_timeout(output, SHUTDOWN_GRACE).await?, id)?;
    Ok(())
}

async fn shutdown_protocol(input: &mut ChildStdin, output: &mut ChildStdout) -> Result<(), String> {
    write_frame_timeout(input, &kind_message("shutdown")?, SHUTDOWN_GRACE).await?;
    let shutdown = read_frame_timeout(output, SHUTDOWN_GRACE).await?;
    expect_kind(&shutdown, "shutdown")?;
    Ok(())
}

async fn wait_for_exit(child: &mut Child) -> Result<(), String> {
    match tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!("external Adapter worker exited with {status}")),
        Ok(Err(error)) => Err(format!("cannot wait for external Adapter worker: {error}")),
        Err(_) => {
            terminate_child(child).await?;
            Err("external Adapter worker exceeded shutdown deadline".into())
        }
    }
}

async fn terminate_child(child: &mut Child) -> Result<(), String> {
    match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(error) => return Err(format!("cannot inspect external Adapter worker: {error}")),
    }
    if let Err(error) = child.start_kill()
        && !matches!(child.try_wait(), Ok(Some(_)))
    {
        return Err(format!("cannot stop external Adapter worker: {error}"));
    }
    match tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(format!("cannot wait for external Adapter worker: {error}")),
        Err(_) => Err("external Adapter worker could not be reaped before its deadline".into()),
    }
}

async fn handshake(
    definition: &Definition,
    input: &mut ChildStdin,
    output: &mut ChildStdout,
) -> Result<(), String> {
    let timeout = Duration::from_millis(definition.timeout_ms);
    write_frame_timeout(input, &hello_message(definition)?, timeout).await?;
    let ready = read_frame_timeout(output, timeout).await?;
    validate_ready(definition, &ready)?;
    write_frame_timeout(input, &id_message("health", 0)?, timeout).await?;
    let health = read_frame_timeout(output, timeout).await?;
    let node = crate::wire::parse(&health)?;
    let fields = node.fields(&["kind", "id"])?;
    if text_field(fields, "kind")? != "health" || int_field(fields, "id")? != 0 {
        return Err("external Adapter returned an invalid health response".into());
    }
    require_fields(fields, &["kind", "id"])
}

async fn drain_stderr(mut stderr: ChildStderr) -> Result<(), String> {
    let mut buffer = [0_u8; 8192];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(format!("cannot read external Adapter stderr: {error}")),
        }
    }
}

async fn write_frame(output: &mut (impl AsyncWrite + Unpin), body: &str) -> Result<(), String> {
    if body.is_empty() || body.len() > crate::wire::MAX_BYTES {
        return Err("component protocol frame exceeds byte limit".into());
    }
    let length =
        u32::try_from(body.len()).map_err(|_| "component protocol frame exceeds byte limit")?;
    output
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|error| format!("cannot write component protocol frame header: {error}"))?;
    output
        .write_all(body.as_bytes())
        .await
        .map_err(|error| format!("cannot write component protocol frame body: {error}"))?;
    output
        .flush()
        .await
        .map_err(|error| format!("cannot flush component protocol frame: {error}"))
}

async fn write_frame_timeout(
    output: &mut (impl AsyncWrite + Unpin),
    body: &str,
    timeout: Duration,
) -> Result<(), String> {
    tokio::time::timeout(timeout, write_frame(output, body))
        .await
        .map_err(|_| "component protocol write timed out".to_owned())?
}

async fn read_frame(input: &mut (impl AsyncRead + Unpin)) -> Result<String, String> {
    let mut length = [0_u8; 4];
    input
        .read_exact(&mut length)
        .await
        .map_err(|error| format!("cannot read component protocol frame header: {error}"))?;
    let length = usize::try_from(u32::from_be_bytes(length)).expect("u32 fits usize");
    if length == 0 || length > crate::wire::MAX_BYTES {
        return Err("component protocol frame exceeds byte limit".into());
    }
    let mut body = vec![0_u8; length];
    input
        .read_exact(&mut body)
        .await
        .map_err(|error| format!("cannot read complete component protocol frame: {error}"))?;
    String::from_utf8(body).map_err(|_| "component protocol frame is not UTF-8".into())
}

async fn read_frame_timeout(
    input: &mut (impl AsyncRead + Unpin),
    timeout: Duration,
) -> Result<String, String> {
    tokio::time::timeout(timeout, read_frame(input))
        .await
        .map_err(|_| "component protocol response timed out".to_owned())?
}

fn hello_message(definition: &Definition) -> Result<String, String> {
    let mut writer = crate::wire::Encoder::default();
    writer.begin_object()?;
    field_text(&mut writer, "kind", "hello")?;
    field_text(&mut writer, "version", VERSION)?;
    field_text(&mut writer, "port", &definition.port)?;
    field_text(&mut writer, "schema", &definition.schema)?;
    field_text(&mut writer, "adapter", &definition.adapter)?;
    field_text_list(&mut writer, "capabilities", &definition.capabilities)?;
    field_text_list(&mut writer, "operations", &definition.operations)?;
    writer.key("setting")?;
    match &definition.setting {
        Some(setting) => writer.json(setting)?,
        None => writer.null()?,
    }
    writer.end()?;
    Ok(writer.finish()?.as_str().to_owned())
}

fn call_message(id: u64, operation: &str, payload: &str) -> Result<String, String> {
    let mut writer = crate::wire::Encoder::default();
    writer.begin_object()?;
    field_text(&mut writer, "kind", "call")?;
    field_int(&mut writer, "id", id)?;
    field_text(&mut writer, "operation", operation)?;
    writer.key("payload")?;
    writer.json(payload)?;
    writer.end()?;
    Ok(writer.finish()?.as_str().to_owned())
}

fn id_message(kind: &str, id: u64) -> Result<String, String> {
    let mut writer = crate::wire::Encoder::default();
    writer.begin_object()?;
    field_text(&mut writer, "kind", kind)?;
    field_int(&mut writer, "id", id)?;
    writer.end()?;
    Ok(writer.finish()?.as_str().to_owned())
}

fn kind_message(kind: &str) -> Result<String, String> {
    let mut writer = crate::wire::Encoder::default();
    writer.begin_object()?;
    field_text(&mut writer, "kind", kind)?;
    writer.end()?;
    Ok(writer.finish()?.as_str().to_owned())
}

fn field_text(writer: &mut crate::wire::Encoder, name: &str, value: &str) -> Result<(), String> {
    writer.key(name)?;
    writer.text(value)
}

fn field_int(writer: &mut crate::wire::Encoder, name: &str, value: u64) -> Result<(), String> {
    let value = i64::try_from(value).map_err(|_| "component protocol request id exhausted")?;
    writer.key(name)?;
    writer.int(value)
}

fn field_text_list(
    writer: &mut crate::wire::Encoder,
    name: &str,
    values: &[String],
) -> Result<(), String> {
    writer.key(name)?;
    writer.begin_array()?;
    for value in values {
        writer.text(value)?;
    }
    writer.end()
}

fn validate_ready(definition: &Definition, body: &str) -> Result<(), String> {
    let node = crate::wire::parse(body)?;
    let names = [
        "kind",
        "version",
        "port",
        "schema",
        "adapter",
        "capabilities",
        "operations",
    ];
    let fields = node.fields(&names)?;
    require_fields(fields, &names)?;
    if text_field(fields, "kind")? != "ready"
        || text_field(fields, "version")? != VERSION
        || text_field(fields, "port")? != definition.port
        || text_field(fields, "schema")? != definition.schema
        || text_field(fields, "adapter")? != definition.adapter
        || text_list_field(fields, "capabilities")? != definition.capabilities
        || text_list_field(fields, "operations")? != definition.operations
    {
        return Err("external Adapter handshake does not match its compiled Port schema".into());
    }
    Ok(())
}

fn parse_reply(body: String, expected_id: u64) -> Result<Reply, String> {
    let node = crate::wire::parse(&body)?;
    let fields = node.object()?;
    let kind = text_field(fields, "kind")?;
    let id = int_field(fields, "id")?;
    if id != expected_id {
        return Err("component protocol response has an unknown or out-of-order request id".into());
    }
    match kind {
        "result" => {
            require_fields(fields, &["kind", "id", "payload"])?;
            Ok(Reply::Result(raw_field(fields, "payload")?.to_owned()))
        }
        "error" => {
            require_fields(fields, &["kind", "id", "error", "payload"])?;
            let identity = text_field(fields, "error")?;
            if identity.is_empty() {
                return Err("component protocol error identity is empty".into());
            }
            Ok(Reply::Error {
                identity: identity.to_owned(),
                payload: raw_field(fields, "payload")?.to_owned(),
            })
        }
        _ => Err("component protocol returned an unknown message kind".into()),
    }
}

fn expect_kind(body: &str, expected: &str) -> Result<(), String> {
    let node = crate::wire::parse(body)?;
    let fields = node.fields(&["kind"])?;
    require_fields(fields, &["kind"])?;
    if text_field(fields, "kind")? == expected {
        Ok(())
    } else {
        Err("component protocol returned an unexpected message kind".into())
    }
}

fn require_fields(
    fields: &BTreeMap<String, crate::wire::Node<'_>>,
    names: &[&str],
) -> Result<(), String> {
    if fields.len() != names.len() || names.iter().any(|name| !fields.contains_key(*name)) {
        return Err("component protocol message has missing or extra fields".into());
    }
    Ok(())
}

fn text_field<'a>(
    fields: &'a BTreeMap<String, crate::wire::Node<'a>>,
    name: &str,
) -> Result<&'a str, String> {
    fields
        .get(name)
        .ok_or_else(|| "component protocol message is missing a field".to_owned())?
        .text()
}

fn int_field(fields: &BTreeMap<String, crate::wire::Node<'_>>, name: &str) -> Result<u64, String> {
    let value = fields
        .get(name)
        .ok_or_else(|| "component protocol message is missing a field".to_owned())?
        .int()?;
    u64::try_from(value).map_err(|_| "component protocol request id must be non-negative".into())
}

fn raw_field<'a>(
    fields: &'a BTreeMap<String, crate::wire::Node<'a>>,
    name: &str,
) -> Result<&'a str, String> {
    fields
        .get(name)
        .map(crate::wire::Node::raw)
        .ok_or_else(|| "component protocol message is missing a field".to_owned())
}

fn text_list_field<'a>(
    fields: &'a BTreeMap<String, crate::wire::Node<'a>>,
    name: &str,
) -> Result<Vec<&'a str>, String> {
    let values = fields
        .get(name)
        .ok_or_else(|| "component protocol message is missing a field".to_owned())?
        .list()?;
    let mut result = Vec::with_capacity(values.len());
    let mut unique = BTreeSet::new();
    for value in values {
        let value = value.text()?;
        if !unique.insert(value) {
            return Err("component protocol list contains a duplicate value".into());
        }
        result.push(value);
    }
    Ok(result)
}
