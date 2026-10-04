use std::io::{self, Write};
use std::time::Duration;

use tokio::runtime::Builder;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};

use crate::config::Arguments;
use crate::protocol::Session;

const SETTLE_TIME: Duration = Duration::from_millis(300);

#[derive(Default)]
struct Counters {
    attempted_connections: u64,
    connected: u64,
    disconnected: u64,
    messages: u64,
    errors: u64,
}

pub fn run(arguments: Arguments) -> Result<(), String> {
    let workers = crate::settings::Settings::load()?.workers()?;
    let runtime = Builder::new_multi_thread()
        .worker_threads(workers)
        .max_blocking_threads(1)
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| format!("cannot start live benchmark runtime: {error}"))?;
    let started = std::time::Instant::now();
    let result = runtime.block_on(execute(&arguments));
    let (counters, failure) = match result {
        Ok(counters) => (counters, None),
        Err((counters, error)) => (counters, Some(error)),
    };
    println!(
        "{{\"protocol\":\"{}\",\"connections\":{},\"cycles\":{},\"attempted_connections\":{},\"connected\":{},\"disconnected\":{},\"messages\":{},\"errors\":{},\"elapsed_ms\":{}}}",
        arguments.protocol.name(),
        arguments.connections,
        arguments.cycles,
        counters.attempted_connections,
        counters.connected,
        counters.disconnected,
        counters.messages,
        counters.errors,
        started.elapsed().as_millis()
    );
    io::stdout().flush().map_err(|error| error.to_string())?;
    match failure {
        Some(error) => Err(error),
        None if counters.errors != 0 => Err("live benchmark recorded connection errors".into()),
        None => Ok(()),
    }
}

async fn execute(arguments: &Arguments) -> Result<Counters, (Counters, String)> {
    let mut counters = Counters::default();
    for cycle in 0..arguments.cycles {
        if let Err(error) = execute_cycle(arguments, cycle, &mut counters).await {
            counters.errors += 1;
            return Err((counters, error));
        }
    }
    Ok(counters)
}

async fn execute_cycle(
    arguments: &Arguments,
    cycle: usize,
    counters: &mut Counters,
) -> Result<(), String> {
    phase(cycle, "connecting")?;
    counters.attempted_connections += arguments.connections as u64;

    let (ready_sender, mut ready_receiver) = mpsc::channel(arguments.connections);
    let (held_sender, mut held_receiver) = mpsc::channel(arguments.connections);
    let (failure_sender, mut failure_receiver) = mpsc::channel(arguments.connections);
    let (hold_sender, hold_receiver) = watch::channel(false);
    let (close_sender, close_receiver) = watch::channel(false);
    let mut sessions = JoinSet::new();
    for index in 0..arguments.connections {
        sessions.spawn(report_session_failure(
            session_task(
                arguments.clone(),
                cycle,
                index,
                ready_sender.clone(),
                held_sender.clone(),
                hold_receiver.clone(),
                close_receiver.clone(),
            ),
            failure_sender.clone(),
        ));
    }
    drop(ready_sender);
    drop(held_sender);
    drop(failure_sender);

    if let Err(error) = wait_for_phase(
        &mut ready_receiver,
        arguments.connections,
        Instant::now() + arguments.timeout,
        "connections",
        &mut failure_receiver,
    )
    .await
    {
        abort_sessions(&mut sessions).await;
        return Err(error);
    }
    counters.connected += arguments.connections as u64;
    if let Err(error) = phase(cycle, "connected") {
        abort_sessions(&mut sessions).await;
        return Err(error);
    }
    if hold_sender.send(true).is_err() {
        abort_sessions(&mut sessions).await;
        return Err("connection tasks ended before hold started".into());
    }

    let hold_deadline = Instant::now() + arguments.hold + arguments.timeout;
    if let Err(error) = wait_for_phase(
        &mut held_receiver,
        arguments.connections,
        hold_deadline,
        "held connections",
        &mut failure_receiver,
    )
    .await
    {
        let _ = close_sender.send(true);
        abort_sessions(&mut sessions).await;
        return Err(error);
    }
    if let Err(error) = phase(cycle, "closing") {
        abort_sessions(&mut sessions).await;
        return Err(error);
    }
    if close_sender.send(true).is_err() {
        abort_sessions(&mut sessions).await;
        return Err("connection tasks ended before close started".into());
    }

    let close_deadline = Instant::now() + arguments.timeout;
    while !sessions.is_empty() {
        let result = match timeout_at(close_deadline, sessions.join_next()).await {
            Err(_) => {
                abort_sessions(&mut sessions).await;
                return Err("connection shutdown timed out".into());
            }
            Ok(None) => {
                abort_sessions(&mut sessions).await;
                return Err("connection task set ended unexpectedly".into());
            }
            Ok(Some(Err(error))) => {
                abort_sessions(&mut sessions).await;
                return Err(format!("connection task failed: {error}"));
            }
            Ok(Some(Ok(result))) => result,
        };
        match result {
            Ok(messages) => {
                counters.disconnected += 1;
                counters.messages += messages;
            }
            Err(error) => {
                abort_sessions(&mut sessions).await;
                return Err(error);
            }
        }
    }
    phase(cycle, "disconnected")?;
    tokio::time::sleep(SETTLE_TIME).await;
    Ok(())
}

async fn session_task(
    arguments: Arguments,
    cycle: usize,
    index: usize,
    ready: mpsc::Sender<()>,
    held: mpsc::Sender<()>,
    mut hold: watch::Receiver<bool>,
    mut close: watch::Receiver<bool>,
) -> Result<u64, String> {
    let mut session = Session::connect(
        arguments.protocol,
        &arguments.target,
        cycle,
        index,
        arguments.timeout,
    )
    .await
    .map_err(|error| connection_error(cycle, index, error))?;
    ready.send(()).await.map_err(|_| "ready receiver closed")?;
    drop(ready);
    wait_for_signal(&mut hold, arguments.timeout, "hold").await?;
    session
        .hold(arguments.hold)
        .await
        .map_err(|error| connection_error(cycle, index, error))?;
    held.send(()).await.map_err(|_| "held receiver closed")?;
    drop(held);
    wait_for_signal(&mut close, arguments.timeout, "close").await?;
    session
        .close()
        .await
        .map_err(|error| connection_error(cycle, index, error))
}

async fn report_session_failure(
    session: impl std::future::Future<Output = Result<u64, String>>,
    failure: mpsc::Sender<String>,
) -> Result<u64, String> {
    let result = session.await;
    if let Err(error) = &result {
        let _ = failure.send(error.clone()).await;
    }
    result
}

async fn wait_for_phase(
    receiver: &mut mpsc::Receiver<()>,
    expected: usize,
    deadline: Instant,
    name: &str,
    failures: &mut mpsc::Receiver<String>,
) -> Result<(), String> {
    for completed in 0..expected {
        tokio::select! {
            biased;
            failure = failures.recv() => {
                return Err(failure.unwrap_or_else(|| format!(
                    "only {completed} of {expected} {name} reached the phase"
                )));
            }
            signal = timeout_at(deadline, receiver.recv()) => match signal {
                Ok(Some(())) => {}
                Ok(None) => {
                    return Err(format!(
                        "only {completed} of {expected} {name} reached the phase"
                    ));
                }
                Err(_) => return Err(format!("timed out waiting for {name}")),
            }
        }
    }
    Ok(())
}

async fn wait_for_signal(
    receiver: &mut watch::Receiver<bool>,
    operation_timeout: Duration,
    name: &str,
) -> Result<(), String> {
    if *receiver.borrow() {
        return Ok(());
    }
    tokio::time::timeout(operation_timeout, receiver.changed())
        .await
        .map_err(|_| format!("timed out waiting for {name} signal"))?
        .map_err(|_| format!("{name} signal closed"))?;
    if !*receiver.borrow() {
        return Err(format!("{name} signal did not start the phase"));
    }
    Ok(())
}

async fn abort_sessions(sessions: &mut JoinSet<Result<u64, String>>) {
    sessions.abort_all();
    while sessions.join_next().await.is_some() {}
}

fn phase(cycle: usize, name: &str) -> Result<(), String> {
    let mut output = io::stdout().lock();
    writeln!(output, "PHASE|{cycle}|{name}").map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())
}

fn connection_error(cycle: usize, index: usize, error: String) -> String {
    format!("cycle {cycle} connection {index}: {error}")
}
