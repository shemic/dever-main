//! One application owner broadcasts stop-admission and joins its structured root.
use crate::config::{RuntimeMode, RuntimeSettings};
use std::future::Future;
use std::sync::{Arc, OnceLock};

pub(crate) struct Lifecycle {
    settings: RuntimeSettings,
    stop: tokio::sync::watch::Sender<bool>,
}

static LIFECYCLE: OnceLock<Arc<Lifecycle>> = OnceLock::new();

fn current() -> Option<Arc<Lifecycle>> {
    #[cfg(feature = "api")]
    if let Some(session) = crate::application::current() {
        return session.lifecycle();
    }
    LIFECYCLE.get().cloned()
}

pub fn configure(api: bool, worker: bool) -> Result<(), String> {
    let settings = crate::config::current_settings().services(api, worker)?;
    let (stop, _) = tokio::sync::watch::channel(false);
    let state = Arc::new(Lifecycle { settings, stop });
    #[cfg(feature = "api")]
    if let Some(session) = crate::application::current() {
        return session.set_lifecycle(state);
    }
    LIFECYCLE
        .set(state)
        .map_err(|_| "application lifecycle initialized twice".into())
}

pub fn api_enabled() -> bool {
    current().is_none_or(|state| matches!(state.settings.mode, RuntimeMode::Api | RuntimeMode::All))
}

pub fn worker_enabled() -> bool {
    current()
        .is_some_and(|state| matches!(state.settings.mode, RuntimeMode::Worker | RuntimeMode::All))
}

pub fn stopping() -> bool {
    current().is_some_and(|state| *state.stop.borrow())
}

pub async fn shutdown_requested() {
    let Some(state) = current() else {
        return std::future::pending().await;
    };
    let mut receiver = state.stop.subscribe();
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

pub async fn run<F, T, E>(future: F) -> Result<T, E>
where
    F: Future<Output = Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: From<String> + Send + 'static,
{
    let state =
        current().ok_or_else(|| E::from("application lifecycle is not configured".into()))?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| E::from(format!("cannot register SIGTERM: {error}")))?;
    #[cfg(unix)]
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| E::from(format!("cannot register SIGINT: {error}")))?;
    let signal = async {
        #[cfg(unix)]
        tokio::select! { _ = terminate.recv() => {}, _ = interrupt.recv() => {} }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c()
            .await
            .map_err(|error| format!("cannot receive Ctrl-C: {error}"))?;
        state.stop.send_replace(true);
        Ok(())
    };
    let root = crate::task::run_typed(future).await.map_err(E::from)?;
    crate::task::wait_shutdown(root, signal, state.settings.shutdown_ms).await
}

pub async fn serve_both<A, W>(api: A, worker: W) -> Result<(), String>
where
    A: Future<Output = Result<(), String>>,
    W: Future<Output = Result<(), String>>,
{
    tokio::try_join!(api, worker)?;
    Ok(())
}
