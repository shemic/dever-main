//! One ordinary child per request, owned by the existing Adapter supervisor.
use super::{Definition, Reply, Request, adapter_settings, terminate_child, worker_command};
use crate::bytes::Bytes;
use crate::config::CommandLimits;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};

mod observation;
use observation::Observation;

#[cfg(feature = "external")]
const ARGUMENT_BYTES: usize = 65_536;
#[cfg(feature = "external")]
const STDIN_BYTES: usize = 8 * 1024 * 1024;

type CompletionSlot = std::sync::Arc<std::sync::Mutex<Option<crate::task::ScopedCompletion>>>;

pub(super) struct CallLease(CompletionSlot);
pub(super) struct RequestLease(CompletionSlot);

pub(super) fn call_lease() -> Result<CallLease, String> {
    Ok(CallLease(std::sync::Arc::new(std::sync::Mutex::new(Some(
        crate::task::scoped_completion()?,
    )))))
}

impl CallLease {
    pub(super) fn request(&self) -> RequestLease {
        RequestLease(self.0.clone())
    }
}

impl Drop for CallLease {
    fn drop(&mut self) {
        // Queued calls have no process to drain. An executing supervisor has
        // already taken the guard and retains it until the child is reaped.
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }
}

impl RequestLease {
    fn begin(self) -> Option<crate::task::ScopedCompletion> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }
}

struct Input {
    #[cfg(feature = "external")]
    arguments: Vec<String>,
    stdin: Option<Bytes>,
}

struct Execution {
    reply: Result<Reply, String>,
    stopped: bool,
    cleanup_error: Option<String>,
}

pub(super) async fn supervise(
    definition: Definition,
    mut requests: mpsc::Receiver<Request>,
    mut stopping: oneshot::Receiver<()>,
    ready: oneshot::Sender<Result<(), String>>,
) -> Result<(), String> {
    let prepared: Result<CommandLimits, String> = (|| {
        let limits = adapter_settings()?.command_limits(&definition.port)?;
        // Validate resources and grants without starting the user's executable.
        worker_command(&definition)?;
        Ok(limits)
    })();
    let limits = match prepared {
        Ok(limits) => limits,
        Err(error) => {
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    let _ = ready.send(Ok(()));
    loop {
        let request = tokio::select! {
            biased;
            _ = &mut stopping => break,
            request = requests.recv() => request,
        };
        let Some(mut request) = request else { break };
        if request.response.is_closed() {
            continue;
        }
        let Some(completion) = request.command_lease.take().and_then(RequestLease::begin) else {
            continue;
        };
        let execution = execute(&definition, limits, &mut request, &mut stopping).await;
        // execute has dropped I/O futures and reaped the child before replying.
        drop(completion);
        let _ = request.response.send(execution.reply);
        if let Some(error) = execution.cleanup_error {
            return Err(error);
        }
        if execution.stopped {
            break;
        }
    }
    requests.close();
    while let Some(request) = requests.recv().await {
        let _ = request
            .response
            .send(Err("external command Adapter stopped".into()));
    }
    Ok(())
}

#[cfg(feature = "external")]
fn input(payload: &str) -> Result<Input, String> {
    let node = crate::wire::parse(payload)?;
    let fields = node.fields(&["args", "stdin"])?;
    let arguments = fields
        .get("args")
        .ok_or("command args are missing")?
        .list()?;
    let mut bytes = 0_usize;
    let arguments = arguments
        .iter()
        .map(|value| {
            let value = value.text()?;
            if value.contains('\0') {
                return Err("command argument contains NUL".into());
            }
            // Include terminators so empty arguments also consume the budget.
            bytes = bytes.saturating_add(value.len()).saturating_add(1);
            if bytes > ARGUMENT_BYTES {
                return Err("command arguments exceed the 65536 byte limit".into());
            }
            Ok(value.to_owned())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let stdin = fields
        .get("stdin")
        .filter(|value| !value.is_null())
        .map(|value| value.bytes())
        .transpose()?;
    if stdin
        .as_ref()
        .is_some_and(|value| value.values().len() > STDIN_BYTES)
    {
        return Err("command stdin exceeds the 8388608 byte limit".into());
    }
    Ok(Input { arguments, stdin })
}

#[cfg(feature = "external")]
fn spawn(definition: &Definition, payload: &str) -> Result<(Input, Child, Observation), String> {
    let input = input(payload)?;
    let (command, observation) = crate::external::observed_command(definition)?;
    let observation = Observation::new(observation)?;
    let mut command = tokio::process::Command::from(command);
    // Resource arguments were mapped by the sandbox owner. Call arguments
    // are opaque values, including absolute sandbox paths and shell syntax.
    command
        .args(&input.arguments)
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .map_err(|error| format!("cannot start external command: {error}"))?;
    // Dropping Command closes the parent's copies of mapped pipe descriptors.
    drop(command);
    Ok((input, child, observation))
}

#[cfg(not(feature = "external"))]
fn spawn(_: &Definition, _: &str) -> Result<(Input, Child, Observation), String> {
    Err("external command sandbox runtime feature is not enabled".into())
}

async fn execute(
    definition: &Definition,
    limits: CommandLimits,
    request: &mut Request,
    stopping: &mut oneshot::Receiver<()>,
) -> Execution {
    let (input, mut child, mut observation) = match spawn(definition, &request.payload) {
        Ok(prepared) => prepared,
        Err(error) => {
            return Execution {
                reply: Err(error),
                stopped: false,
                cleanup_error: None,
            };
        }
    };
    let wrapper = child
        .id()
        .expect("newly spawned command launcher has a PID");
    let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
        let cleanup_error = cleanup(&mut child, &mut observation, wrapper).await.err();
        return Execution {
            reply: Err(cleanup_error
                .clone()
                .unwrap_or_else(|| "external command has incomplete pipes".into())),
            stopped: false,
            cleanup_error,
        };
    };
    let mut stopped = false;
    let result = {
        let completion = async {
            let (_, _, (stdout, stderr), status) = tokio::try_join!(
                observation.started(wrapper),
                write_stdin(stdin, input.stdin),
                capture(stdout, stderr, limits.output_limit),
                async {
                    child
                        .wait()
                        .await
                        .map_err(|error| format!("cannot wait for external command: {error}"))
                },
            )?;
            let code = status
                .code()
                .ok_or("external command sandbox terminated without an exit code")?;
            output(code, stdout, stderr)
        };
        tokio::select! {
            biased;
            _ = stopping => {
                stopped = true;
                Err("external command Adapter stopped".into())
            }
            _ = request.response.closed() => Err("external command call cancelled".into()),
            result = tokio::time::timeout(std::time::Duration::from_millis(limits.timeout_ms), completion) => {
                result.unwrap_or_else(|_| Err("external command call timed out".into()))
            }
        }
    };
    let cleanup_error = cleanup(&mut child, &mut observation, wrapper).await.err();
    let reply = match (result, &cleanup_error) {
        (result, None) => result,
        (Ok(_), Some(cleanup)) => Err(cleanup.clone()),
        (Err(error), Some(cleanup)) => Err(format!("{error}; command cleanup failed: {cleanup}")),
    };
    Execution {
        reply,
        stopped,
        cleanup_error,
    }
}

async fn cleanup(
    child: &mut Child,
    observation: &mut Observation,
    wrapper: u32,
) -> Result<(), String> {
    let namespace = tokio::time::timeout(super::SHUTDOWN_GRACE, observation.terminate(wrapper))
        .await
        .unwrap_or_else(|_| Err("external command namespace exceeded its cleanup deadline".into()));
    // Reap the owned launcher even if namespace observation itself failed.
    // Both stages have independent finite deadlines; neither error is lost.
    let launcher = terminate_child(child).await;
    match (namespace, launcher) {
        (Err(namespace), Err(launcher)) => {
            Err(format!("{namespace}; launcher cleanup failed: {launcher}"))
        }
        (namespace, launcher) => namespace.and(launcher),
    }
}

async fn write_stdin(mut stdin: ChildStdin, input: Option<Bytes>) -> Result<(), String> {
    if let Some(input) = input {
        match stdin.write_all(input.values()).await {
            Ok(()) => {}
            // Ordinary programs may intentionally stop reading before EOF.
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
            Err(error) => return Err(format!("cannot write external command stdin: {error}")),
        }
    }
    drop(stdin);
    Ok(())
}

async fn capture(
    mut stdout: ChildStdout,
    mut stderr: ChildStderr,
    limit: usize,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut output = (Vec::new(), Vec::new());
    let mut stdout_buffer = [0; 8192];
    let mut stderr_buffer = [0; 8192];
    let mut stdout_open = true;
    let mut stderr_open = true;
    while stdout_open || stderr_open {
        let (read, is_stdout) = tokio::select! {
            read = stdout.read(&mut stdout_buffer), if stdout_open => (read, true),
            read = stderr.read(&mut stderr_buffer), if stderr_open => (read, false),
        };
        let count =
            read.map_err(|error| format!("cannot read external command output: {error}"))?;
        if count == 0 {
            if is_stdout {
                stdout_open = false;
            } else {
                stderr_open = false;
            }
            continue;
        }
        if count > limit.saturating_sub(output.0.len() + output.1.len()) {
            return Err("external command output exceeds output_limit".into());
        }
        if is_stdout {
            output.0.extend_from_slice(&stdout_buffer[..count]);
        } else {
            output.1.extend_from_slice(&stderr_buffer[..count]);
        }
    }
    Ok(output)
}

fn output(code: i32, stdout: Vec<u8>, stderr: Vec<u8>) -> Result<Reply, String> {
    let mut encoder = crate::wire::Encoder::default();
    encoder.begin_object()?;
    encoder.key("output")?;
    encoder.begin_object()?;
    encoder.key("code")?;
    encoder.int(i64::from(code))?;
    encoder.key("stdout")?;
    encoder.bytes(&Bytes::new(stdout))?;
    encoder.key("stderr")?;
    encoder.bytes(&Bytes::new(stderr))?;
    encoder.end()?;
    encoder.end()?;
    Ok(Reply::Result(encoder.finish()?.as_str().to_owned()))
}
