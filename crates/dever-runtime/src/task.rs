use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::runtime::Builder;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{AbortHandle, JoinError, JoinHandle, JoinSet};

mod scoped;

pub(crate) use scoped::{ScopeFault, ScopedOwner, ScopedSpawner, scoped_owner};
#[cfg(feature = "wire")]
pub(crate) use scoped::{ScopedCompletion, scoped_completion};

tokio::task_local! {
    static CURRENT_SCOPE: Scope;
}

pub const MAX_ASYNC_WORKERS: usize = 64;
pub const MAX_BLOCKING_WORKERS: usize = 64;
pub const DEFAULT_MAX_BLOCKING_WORKERS: usize = 8;
pub const DEFAULT_TASKS: usize = 4_096;
pub const MAX_TASKS: usize = 65_536;
pub const MAX_GROUP_TASKS: usize = 65_536;

pub fn run_entry<F, T>(future: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    run_boxed_entry(RuntimeConfig::default(), Box::pin(future))
}

pub fn run_entry_typed<F, T, E>(future: F) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<String>,
{
    run_boxed_entry(RuntimeConfig::default(), Box::pin(future))
}

/// Source-configured programs establish one runtime, including when called from
/// synchronous parallel handlers. Rust embedding retains run_entry_with.
pub fn start_entry<F>(config: RuntimeConfig, future: F) -> Result<(), String>
where
    F: Future<Output = Result<(), String>>,
{
    static STARTED: AtomicBool = AtomicBool::new(false);
    validate_runtime_config(config)?;
    if STARTED.swap(true, Ordering::AcqRel) {
        return Err("the program runtime can only be started once".into());
    }
    run_boxed_entry(config, Box::pin(future))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeConfig {
    pub worker_threads: usize,
    pub max_blocking_threads: usize,
    pub task_capacity: usize,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            worker_threads: default_async_workers(),
            max_blocking_threads: default_blocking_workers(),
            task_capacity: DEFAULT_TASKS,
        }
    }
}

pub fn run_entry_with<F, T>(config: RuntimeConfig, future: F) -> Result<T, String>
where
    F: Future<Output = Result<T, String>>,
{
    run_boxed_entry(config, Box::pin(future))
}

pub fn run_entry_with_typed<F, T, E>(config: RuntimeConfig, future: F) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<String>,
{
    run_boxed_entry(config, Box::pin(future))
}

// Pin once at each public entry before forwarding a potentially large Future
// through synchronous cleanup adapters and task-local scopes.
fn run_boxed_entry<F, T, E>(config: RuntimeConfig, future: Pin<Box<F>>) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<String>,
{
    run_scoped_entry(config, future, |result, resources| {
        std::future::ready(merge_resource_result(result, resources))
    })
}

fn merge_resource_result<T, E>(result: Result<T, E>, resources: Result<(), String>) -> Result<T, E>
where
    E: From<String>,
{
    match result {
        Ok(value) => resources.map(|()| value).map_err(E::from),
        Err(primary) => Err(primary),
    }
}

/// Close entry-owned resources after descendants drain, while their runtime
/// is still alive. Cleanup must not start new business tasks in the closed scope.
pub fn run_entry_with_typed_cleanup<F, T, E, C, Cleanup>(
    config: RuntimeConfig,
    future: F,
    cleanup: C,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<String>,
    C: FnOnce(Result<T, E>) -> Cleanup,
    Cleanup: Future<Output = Result<T, E>>,
{
    run_scoped_entry(config, Box::pin(future), |result, resources| {
        cleanup(merge_resource_result(result, resources))
    })
}

/// Stop invocation Workers before cancelling descendants, then let the caller
/// append cleanup diagnostics using its concrete fault representation.
pub fn run_entry_with_typed_resource_cleanup<F, T, E, C, Cleanup>(
    config: RuntimeConfig,
    future: F,
    cleanup: C,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<String>,
    C: FnOnce(Result<T, E>, Result<(), String>) -> Cleanup,
    Cleanup: Future<Output = Result<T, E>>,
{
    run_scoped_entry(config, Box::pin(future), cleanup)
}

fn run_scoped_entry<F, T, E, C, Cleanup>(
    config: RuntimeConfig,
    future: Pin<Box<F>>,
    cleanup: C,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: From<String>,
    C: FnOnce(Result<T, E>, Result<(), String>) -> Cleanup,
    Cleanup: Future<Output = Result<T, E>>,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(E::from(
            "an async Dever entry cannot start inside another runtime".into(),
        ));
    }
    validate_runtime_config(config).map_err(E::from)?;
    let runtime = Builder::new_multi_thread()
        .worker_threads(config.worker_threads)
        .max_blocking_threads(config.max_blocking_threads)
        .enable_time()
        .enable_io()
        .build()
        .map_err(|error| E::from(format!("cannot start async runtime: {error}")))?;
    runtime.block_on(async {
        let scope = Scope::root(config);
        #[cfg(feature = "wire")]
        let components = crate::component::Session::new(scope.clone());
        let execution = CURRENT_SCOPE.scope(scope.clone(), async {
            let result = future.await;
            #[cfg(feature = "wire")]
            let resources = crate::component::close().await;
            #[cfg(not(feature = "wire"))]
            let resources = Ok(());
            scope.cancel_and_wait().await;
            cleanup(result, resources).await
        });
        #[cfg(feature = "wire")]
        let execution = crate::component::scope_resources(Some(components), execution);
        execution.await
    })
}

pub struct Task<T, E = String> {
    body: Option<AbortHandle>,
    supervisor: Option<JoinHandle<(usize, TaskOutcome<T, E>)>>,
    registration: Option<Registration>,
}

impl<T, E> Task<T, E> {
    fn request_stop(&mut self) {
        if let Some(body) = self.body.take() {
            body.abort();
        }
    }

    async fn finish(mut self, stopping: bool) -> Result<TaskOutcome<T, E>, E>
    where
        E: From<String>,
    {
        if stopping {
            self.request_stop();
        } else {
            self.body.take();
        }
        let supervisor = self.supervisor.take().expect("task consumed exactly once");
        let result = supervisor.await;
        self.registration
            .take()
            .expect("task registration")
            .finish();
        result
            .map(|(_, outcome)| outcome)
            .map_err(|error| E::from(join_error(error)))
    }
}

impl<T, E> Drop for Task<T, E> {
    fn drop(&mut self) {
        self.request_stop();
    }
}

pub async fn run<F, T>(future: F) -> Result<Task<T>, String>
where
    F: Future<Output = Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    run_typed(future).await
}

pub async fn run_typed<F, T, E>(future: F) -> Result<Task<T, E>, String>
where
    F: Future<Output = Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    let parent = current_scope()?;
    run_in(&parent, future)
}

pub(crate) fn run_in<F, T, E>(parent: &Scope, future: F) -> Result<Task<T, E>, String>
where
    F: Future<Output = Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    let permit = parent
        .limits
        .task_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| "async task capacity is exhausted".to_owned())?;
    let started = start_task(parent, future, permit);
    let body = started.body.abort_handle();
    let registration = Registration {
        scope: parent.clone(),
        id: started.id,
    };
    Ok(Task {
        body: Some(body),
        supervisor: Some(tokio::spawn(supervise(started))),
        registration: Some(registration),
    })
}

pub async fn wait<T, E>(task: Task<T, E>) -> Result<T, E>
where
    E: From<String>,
{
    waited_result(task.finish(false).await)
}

pub async fn stop<E>(task: Task<(), E>) -> Result<(), E>
where
    E: From<String>,
{
    stopped_result(task.finish(true).await)
}

// Resource close may itself be cancelled. Keep the supervisor in its owner until
// completion so another close caller can resume the same cleanup operation.
pub(crate) async fn stop_resource(slot: &mut Option<Task<()>>) -> Result<(), String> {
    let Some(task) = slot.as_mut() else {
        return Ok(());
    };
    task.request_stop();
    let result = task.supervisor.as_mut().expect("resource task").await;
    stopped_result(slot.take().expect("resource task").completed(result))
}

#[cfg(feature = "wire")]
pub(crate) async fn wait_resource(slot: &mut Option<Task<()>>) -> Result<(), String> {
    let Some(task) = slot.as_mut() else {
        return Ok(());
    };
    let result = task.supervisor.as_mut().expect("resource task").await;
    waited_result(slot.take().expect("resource task").completed(result))
}

/// The deadline ends execution, then cleanup completes before the caller resumes.
pub async fn wait_timeout<T, E>(mut task: Task<T, E>, millis: i64) -> Result<Option<T>, E>
where
    E: From<String>,
{
    let duration = positive_duration(millis).map_err(E::from)?;
    let completed = tokio::select! {
        biased;
        result = task.supervisor.as_mut().expect("owned task") => Some(result),
        _ = tokio::time::sleep(duration) => None,
    };
    match completed {
        Some(result) => waited_result(task.completed(result)).map(Some),
        None => discard_result(task.finish(true).await).map(|()| None),
    }
}

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub(crate) enum LeaseResult<T> {
    Completed(T),
    Timeout,
    Lost,
}

/// Retain the supervisor while renewing a lease; every exit joins cancelled descendants.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub(crate) async fn wait_lease<T, F, Fut>(
    mut task: Task<T>,
    timeout_ms: i64,
    period_ms: i64,
    mut renew: F,
) -> Result<LeaseResult<T>, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, String>>,
{
    let deadline = tokio::time::sleep(positive_duration(timeout_ms)?);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            result = task.supervisor.as_mut().expect("owned task") => return waited_result(task.completed(result)).map(LeaseResult::Completed),
            _ = &mut deadline => { discard_result(task.finish(true).await)?; return Ok(LeaseResult::Timeout); }
            _ = tokio::time::sleep(positive_duration(period_ms)?) => {}
        }
        let renewed = tokio::select! {
            biased;
            result = task.supervisor.as_mut().expect("owned task") => return waited_result(task.completed(result)).map(LeaseResult::Completed),
            _ = &mut deadline => { discard_result(task.finish(true).await)?; return Ok(LeaseResult::Timeout); }
            result = renew() => result,
        };
        if !matches!(renewed, Ok(true)) {
            discard_result(task.finish(true).await)?;
            return renewed.map(|_| LeaseResult::Lost);
        }
    }
}

/// The source root remains owned while a lifecycle requests cooperative shutdown.
pub async fn wait_shutdown<T, E, F>(
    mut task: Task<T, E>,
    shutdown: F,
    grace_ms: i64,
) -> Result<T, E>
where
    E: From<String>,
    F: Future<Output = Result<(), String>>,
{
    tokio::select! {
        biased;
        result = task.supervisor.as_mut().expect("owned task") => return waited_result(task.completed(result)),
        signal = shutdown => {
            if let Err(error) = signal {
                discard_result(task.finish(true).await)?;
                return Err(E::from(error));
            }
        }
    }
    wait_timeout(task, grace_ms)
        .await?
        .ok_or_else(|| E::from("application shutdown deadline exceeded".into()))
}

pub async fn race<T, E>(mut tasks: Vec<Task<T, E>>) -> Result<T, E>
where
    E: From<String>,
{
    if tasks.len() < 2 {
        return Err(E::from("race requires at least two tasks".into()));
    }
    let (result, winner, _) = futures_util::future::select_all(
        tasks
            .iter_mut()
            .map(|task| task.supervisor.as_mut().expect("owned task")),
    )
    .await;
    let result = waited_result(tasks.remove(winner).completed(result));
    for task in &mut tasks {
        task.request_stop();
    }
    let mut cleanup = Ok(());
    for task in tasks {
        if let Err(error) = discard_result(task.finish(true).await)
            && cleanup.is_ok()
        {
            cleanup = Err(error);
        }
    }
    let value = result?;
    cleanup?;
    Ok(value)
}

impl<T, E> Task<T, E> {
    fn completed(
        mut self,
        result: Result<(usize, TaskOutcome<T, E>), JoinError>,
    ) -> Result<TaskOutcome<T, E>, E>
    where
        E: From<String>,
    {
        self.body.take();
        self.supervisor.take();
        self.registration
            .take()
            .expect("task registration")
            .finish();
        result
            .map(|(_, outcome)| outcome)
            .map_err(|error| E::from(join_error(error)))
    }
}

fn discard_result<T, E>(result: Result<TaskOutcome<T, E>, E>) -> Result<(), E>
where
    E: From<String>,
{
    match result? {
        TaskOutcome::Finished(result) => result.map(|_| ()),
        TaskOutcome::Stopped => Ok(()),
        TaskOutcome::Panicked(error) => Err(E::from(error)),
    }
}

pub fn positive_duration(millis: i64) -> Result<std::time::Duration, String> {
    let millis = u64::try_from(millis)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "duration must be positive".to_owned())?;
    Ok(std::time::Duration::from_millis(millis))
}

pub fn ticks(millis: i64) -> Result<crate::async_stream::AsyncStream<i64>, String> {
    let period = positive_duration(millis)?;
    current_scope()?;
    let mut timer = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    Ok(crate::async_stream::AsyncStream::new(
        futures_util::stream::unfold((timer, 0_i64), |(mut timer, count)| async move {
            timer.tick().await;
            let next = count.checked_add(1)?;
            Some((crate::resource::Pull::Item(next), (timer, next)))
        }),
    ))
}

pub fn configured(workers: i64, blocking: i64, tasks: i64) -> Result<RuntimeConfig, String> {
    let number =
        |value| usize::try_from(value).map_err(|_| "runtime limits must be positive".to_owned());
    let config = RuntimeConfig {
        worker_threads: number(workers)?,
        max_blocking_threads: number(blocking)?,
        task_capacity: number(tasks)?,
    };
    validate_runtime_config(config)?;
    Ok(config)
}

pub async fn parallel<F, T>(apply: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    parallel_typed(apply).await
}

pub async fn parallel_typed<F, T, E>(apply: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: From<String> + Send + 'static,
{
    start_blocking(apply).await?.wait().await
}

pub async fn blocking<F, T>(apply: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    blocking_typed(apply).await
}

pub async fn blocking_typed<F, T, E>(apply: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: From<String> + Send + 'static,
{
    parallel_typed(apply).await
}

pub async fn parallel_each<I, T, F>(values: I, workers: i64, apply: F) -> Result<(), String>
where
    I: IntoIterator<Item = T>,
    I::IntoIter: Send,
    T: Send + 'static,
    F: Fn(T) -> Result<(), String> + Send + Sync + 'static,
{
    parallel_each_typed(values, workers, apply).await
}

pub async fn parallel_each_typed<I, T, F, E>(values: I, workers: i64, apply: F) -> Result<(), E>
where
    I: IntoIterator<Item = T>,
    I::IntoIter: Send,
    T: Send + 'static,
    F: Fn(T) -> Result<(), E> + Send + Sync + 'static,
    E: From<String> + Send + 'static,
{
    let workers =
        crate::concurrent::worker_count(workers).map_err(|error| E::from(error.to_owned()))?;
    let mut values = values.into_iter();
    let (minimum, maximum) = values.size_hint();
    let exact_items = maximum.filter(|maximum| *maximum == minimum);
    let concurrency = workers
        .min(
            current_runtime_limits()
                .map_err(E::from)?
                .max_blocking_workers,
        )
        .min(exact_items.unwrap_or(usize::MAX));
    if concurrency == 0 {
        return Ok(());
    }
    let batch_size = exact_items
        .map(|items| crate::concurrent::batch_size(items, concurrency))
        .unwrap_or(1);
    let apply = Arc::new(apply);
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut jobs: VecDeque<ScopedBlocking<(), E>> = VecDeque::new();
    let mut first_error = None;

    loop {
        if jobs.len() >= concurrency {
            let result = jobs
                .pop_front()
                .expect("non-empty parallel job queue")
                .wait()
                .await;
            record_parallel_result(result, &cancelled, &mut first_error);
        }
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        let permit = acquire_blocking_slot().await.map_err(E::from)?;
        let batch: Vec<_> = values.by_ref().take(batch_size).collect();
        if batch.is_empty() {
            break;
        }
        let apply = Arc::clone(&apply);
        let cancelled = Arc::clone(&cancelled);
        jobs.push_back(start_blocking_with_permit(permit, move || {
            for value in batch {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) = apply(value) {
                    cancelled.store(true, Ordering::Release);
                    return Err(error);
                }
            }
            Ok(())
        })?);
    }
    while let Some(job) = jobs.pop_front() {
        record_parallel_result(job.wait().await, &cancelled, &mut first_error);
    }
    first_error.map_or(Ok(()), Err)
}

pub async fn sleep(milliseconds: i64) -> Result<(), &'static str> {
    let milliseconds =
        u64::try_from(milliseconds).map_err(|_| "sleep duration must not be negative")?;
    tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
    Ok(())
}

pub async fn parallel_each_stream<T, F, Fut>(
    stream: crate::async_stream::AsyncStream<T>,
    limit: i64,
    apply: F,
) -> Result<(), String>
where
    T: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    parallel_each_stream_typed(stream, limit, apply).await
}

pub async fn parallel_each_stream_typed<T, F, Fut, E>(
    stream: crate::async_stream::AsyncStream<T>,
    limit: i64,
    apply: F,
) -> Result<(), E>
where
    T: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = Result<(), E>> + Send + 'static,
    E: From<String> + Send + 'static,
{
    let _owner = stream.close_on_drop();
    let mut group = Group::<E>::new_typed(limit).map_err(E::from)?;
    let result = async {
        loop {
            group.reap_completed().await?;
            if group.tasks.len() == group.limit {
                group.join_one().await?;
                continue;
            }
            // 拉取前预留容量；生产者不会在任务组之外积压下一项。
            let permit = task_slots()
                .map_err(E::from)?
                .try_acquire_owned()
                .map_err(|_| E::from("async task capacity is exhausted".to_owned()))?;
            let next = stream.pull();
            tokio::pin!(next);
            let value = loop {
                tokio::select! {
                    biased;
                    result = group.tasks.join_next(), if !group.tasks.is_empty() => {
                        group.complete(result.expect("non-empty group"), false)?;
                    }
                    value = &mut next => break value,
                }
            };
            let Some(value) = value else {
                return Ok(());
            };
            group.start(apply(value), permit);
        }
    }
    .await;
    match result {
        Ok(()) => group.wait().await,
        Err(error) => group.stop_and_drain(Some(error)).await,
    }
}

pub struct Group<E: 'static = String> {
    limit: usize,
    tasks: JoinSet<(usize, TaskOutcome<(), E>)>,
    bodies: HashMap<usize, AbortHandle>,
    scope: Scope,
}

impl<E> Group<E>
where
    E: From<String> + Send + 'static,
{
    pub fn new_typed(limit: i64) -> Result<Self, String> {
        let limit = usize::try_from(limit)
            .ok()
            .filter(|limit| (1..=MAX_GROUP_TASKS).contains(limit))
            .ok_or_else(|| "task group limit must be between 1 and 65536".to_owned())?;
        Ok(Self {
            limit,
            tasks: JoinSet::new(),
            bodies: HashMap::new(),
            scope: current_scope()?,
        })
    }

    pub async fn run<F>(&mut self, future: F) -> Result<(), E>
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
    {
        if let Err(error) = self.reap_completed().await {
            return self.stop_and_drain(Some(error)).await;
        }
        if self.tasks.len() >= self.limit
            && let Err(error) = self.join_one().await
        {
            return self.stop_and_drain(Some(error)).await;
        }
        let permit = task_slots()
            .map_err(E::from)?
            .try_acquire_owned()
            .map_err(|_| E::from("async task capacity is exhausted".to_owned()))?;
        self.start(future, permit);
        Ok(())
    }

    fn start<F>(&mut self, future: F, permit: OwnedSemaphorePermit)
    where
        F: Future<Output = Result<(), E>> + Send + 'static,
    {
        let started = start_task(&self.scope, future, permit);
        self.bodies.insert(started.id, started.body.abort_handle());
        self.tasks.spawn(supervise(started));
    }

    pub async fn wait(mut self) -> Result<(), E> {
        while !self.tasks.is_empty() {
            if let Err(error) = self.join_one().await {
                return self.stop_and_drain(Some(error)).await;
            }
        }
        Ok(())
    }

    pub async fn stop(mut self) -> Result<(), E> {
        self.stop_and_drain(None).await
    }

    async fn reap_completed(&mut self) -> Result<(), E> {
        while let Some(result) = self.tasks.try_join_next() {
            self.complete(result, false)?;
        }
        Ok(())
    }

    async fn join_one(&mut self) -> Result<(), E> {
        match self.tasks.join_next().await {
            Some(result) => self.complete(result, false),
            None => Ok(()),
        }
    }

    fn complete(
        &mut self,
        result: Result<(usize, TaskOutcome<(), E>), JoinError>,
        stopping: bool,
    ) -> Result<(), E> {
        let (id, outcome) = result.map_err(|error| E::from(join_error(error)))?;
        self.bodies.remove(&id);
        self.scope.unregister(id);
        if stopping {
            stopped_result(Ok(outcome))
        } else {
            waited_result(Ok(outcome))
        }
    }

    fn request_stop(&mut self) {
        for body in self.bodies.values() {
            body.abort();
        }
    }

    async fn stop_and_drain(&mut self, mut first_error: Option<E>) -> Result<(), E> {
        self.request_stop();
        while let Some(result) = self.tasks.join_next().await {
            if let Err(error) = self.complete(result, true) {
                first_error.get_or_insert(error);
            }
        }
        for id in self.bodies.keys().copied().collect::<Vec<_>>() {
            self.scope.unregister(id);
        }
        self.bodies.clear();
        first_error.map_or(Ok(()), Err)
    }
}

impl Group<String> {
    pub fn new(limit: i64) -> Result<Self, String> {
        Self::new_typed(limit)
    }
}

impl<E: 'static> Drop for Group<E> {
    fn drop(&mut self) {
        for body in self.bodies.values() {
            body.abort();
        }
        self.tasks.detach_all();
    }
}

struct StartedTask<T, E> {
    id: usize,
    body: JoinHandle<Result<T, E>>,
    scope: Scope,
    completion: Completion,
    _permit: OwnedSemaphorePermit,
}

fn start_task<F, T, E>(parent: &Scope, future: F, permit: OwnedSemaphorePermit) -> StartedTask<T, E>
where
    F: Future<Output = Result<T, E>> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    let scope = parent.child();
    #[cfg(feature = "wire")]
    let future = crate::component::scope_resources(crate::component::current(), future);
    #[cfg(feature = "api")]
    let future = crate::application::scope_resources(crate::application::current(), future);
    let body = tokio::spawn(CURRENT_SCOPE.scope(scope.clone(), future));
    let completion = Completion::new();
    let id = parent.register(Some(body.abort_handle()), completion.clone());
    StartedTask {
        id,
        body,
        scope,
        completion,
        _permit: permit,
    }
}

async fn supervise<T, E>(started: StartedTask<T, E>) -> (usize, TaskOutcome<T, E>)
where
    E: Send + 'static,
{
    supervise_with(started, |_, _| {}).await
}

async fn supervise_with<T, E>(
    started: StartedTask<T, E>,
    complete: impl FnOnce(usize, &TaskOutcome<T, E>),
) -> (usize, TaskOutcome<T, E>)
where
    E: Send + 'static,
{
    let _complete = CompleteOnDrop(started.completion);
    let outcome = match started.body.await {
        Ok(result) => TaskOutcome::Finished(result),
        Err(error) if error.is_cancelled() => TaskOutcome::Stopped,
        Err(error) => TaskOutcome::Panicked(join_error(error)),
    };
    started.scope.cancel_and_wait().await;
    complete(started.id, &outcome);
    (started.id, outcome)
}

enum TaskOutcome<T, E> {
    Finished(Result<T, E>),
    Stopped,
    Panicked(String),
}

fn waited_result<T, E>(result: Result<TaskOutcome<T, E>, E>) -> Result<T, E>
where
    E: From<String>,
{
    match result? {
        TaskOutcome::Finished(result) => result,
        TaskOutcome::Stopped => Err(E::from("async task was stopped".into())),
        TaskOutcome::Panicked(error) => Err(E::from(error)),
    }
}

fn stopped_result<E>(result: Result<TaskOutcome<(), E>, E>) -> Result<(), E>
where
    E: From<String>,
{
    match result? {
        TaskOutcome::Finished(result) => result,
        TaskOutcome::Stopped => Ok(()),
        TaskOutcome::Panicked(error) => Err(E::from(error)),
    }
}

struct ScopedBlocking<T, E> {
    id: usize,
    scope: Scope,
    handle: Option<JoinHandle<(usize, Result<T, E>)>>,
}

impl<T: Send + 'static, E: From<String> + Send + 'static> ScopedBlocking<T, E> {
    async fn wait(mut self) -> Result<T, E> {
        let handle = self.handle.take().expect("blocking task awaited once");
        let result = handle.await;
        self.scope.unregister(self.id);
        match result {
            Ok((_, result)) => result,
            Err(error) => Err(E::from(join_error(error))),
        }
    }
}

async fn start_blocking<F, T, E>(apply: F) -> Result<ScopedBlocking<T, E>, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: From<String> + Send + 'static,
{
    start_blocking_with_permit(acquire_blocking_slot().await.map_err(E::from)?, apply)
}

fn start_blocking_with_permit<F, T, E>(
    permit: OwnedSemaphorePermit,
    apply: F,
) -> Result<ScopedBlocking<T, E>, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: From<String> + Send + 'static,
{
    let scope = current_scope().map_err(E::from)?;
    let completion = Completion::new();
    let id = scope.register(None, completion.clone());
    #[cfg(feature = "api")]
    let session = crate::application::current();
    #[cfg(feature = "wire")]
    let components = crate::component::current();
    let handle = tokio::task::spawn_blocking(move || {
        #[cfg(feature = "api")]
        let apply = || crate::application::with_resources(session, apply);
        #[cfg(feature = "wire")]
        let apply = || crate::component::with_resources(components, apply);
        let _permit = permit;
        let _complete = CompleteOnDrop(completion);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(apply))
            .unwrap_or_else(|_| Err(E::from("blocking task panicked".to_owned())));
        (id, result)
    });
    Ok(ScopedBlocking {
        id,
        scope,
        handle: Some(handle),
    })
}

async fn acquire_blocking_slot() -> Result<OwnedSemaphorePermit, String> {
    blocking_slots()?
        .acquire_owned()
        .await
        .map_err(|_| "blocking task capacity is closed".to_owned())
}

#[derive(Clone)]
pub(crate) struct Scope {
    state: Arc<ScopeState>,
    limits: RuntimeLimits,
}

struct ScopeState {
    registry: Mutex<ScopeRegistry>,
}

#[derive(Default)]
struct ScopeRegistry {
    next_id: usize,
    work: HashMap<usize, ScopedWork>,
}

struct ScopedWork {
    abort: Option<AbortHandle>,
    completion: Completion,
}

impl Scope {
    fn root(config: RuntimeConfig) -> Self {
        Self::new(RuntimeLimits::new(config))
    }

    fn child(&self) -> Self {
        Self::new(self.limits.clone())
    }

    fn new(limits: RuntimeLimits) -> Self {
        Self {
            state: Arc::new(ScopeState {
                registry: Mutex::new(ScopeRegistry::default()),
            }),
            limits,
        }
    }

    fn register(&self, abort: Option<AbortHandle>, completion: Completion) -> usize {
        let mut registry = self
            .state
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let id = registry.next_id;
        registry.next_id = registry.next_id.wrapping_add(1);
        registry.work.insert(id, ScopedWork { abort, completion });
        id
    }

    fn unregister(&self, id: usize) {
        self.state
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .work
            .remove(&id);
    }

    async fn cancel_and_wait(&self) {
        loop {
            let work = {
                let mut registry = self
                    .state
                    .registry
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                std::mem::take(&mut registry.work)
            };
            if work.is_empty() {
                return;
            }
            for child in work.values() {
                if let Some(abort) = &child.abort {
                    abort.abort();
                }
            }
            for child in work.into_values() {
                child.completion.wait().await;
            }
        }
    }

    async fn wait(&self) {
        loop {
            let completions = self
                .state
                .registry
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .work
                .values()
                .map(|work| work.completion.clone())
                .collect::<Vec<_>>();
            if completions.is_empty() {
                return;
            }
            for completion in completions {
                completion.wait().await;
            }
        }
    }
}

struct Registration {
    scope: Scope,
    id: usize,
}

impl Registration {
    fn finish(self) {
        self.scope.unregister(self.id);
    }
}

#[derive(Clone)]
struct Completion(Arc<Semaphore>);

impl Completion {
    fn new() -> Self {
        Self(Arc::new(Semaphore::new(0)))
    }

    fn complete(&self) {
        self.0.close();
    }

    async fn wait(self) {
        let _ = self.0.acquire().await;
    }
}

struct CompleteOnDrop(Completion);

impl Drop for CompleteOnDrop {
    fn drop(&mut self) {
        self.0.complete();
    }
}

fn current_scope() -> Result<Scope, String> {
    CURRENT_SCOPE
        .try_with(Clone::clone)
        .map_err(|_| "async operation requires a Dever task scope".to_owned())
}

#[cold]
fn join_error(error: JoinError) -> String {
    if error.is_cancelled() {
        "async task was stopped".into()
    } else if error.is_panic() {
        "async task panicked".into()
    } else {
        error.to_string()
    }
}

#[derive(Clone)]
struct RuntimeLimits {
    task_slots: Arc<Semaphore>,
    blocking_slots: Arc<Semaphore>,
    max_blocking_workers: usize,
}

impl RuntimeLimits {
    fn new(config: RuntimeConfig) -> Self {
        Self {
            task_slots: Arc::new(Semaphore::new(config.task_capacity)),
            blocking_slots: Arc::new(Semaphore::new(config.max_blocking_threads)),
            max_blocking_workers: config.max_blocking_threads,
        }
    }
}

fn record_parallel_result<E>(
    result: Result<(), E>,
    cancelled: &AtomicBool,
    first_error: &mut Option<E>,
) {
    if let Err(error) = result {
        cancelled.store(true, Ordering::Release);
        first_error.get_or_insert(error);
    }
}

fn task_slots() -> Result<Arc<Semaphore>, String> {
    Ok(current_runtime_limits()?.task_slots)
}

fn blocking_slots() -> Result<Arc<Semaphore>, String> {
    Ok(current_runtime_limits()?.blocking_slots)
}

fn current_runtime_limits() -> Result<RuntimeLimits, String> {
    Ok(current_scope()?.limits)
}

fn default_async_workers() -> usize {
    available_parallelism().min(MAX_ASYNC_WORKERS)
}

fn default_blocking_workers() -> usize {
    available_parallelism()
        .min(DEFAULT_MAX_BLOCKING_WORKERS)
        .min(MAX_BLOCKING_WORKERS)
}

fn available_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

fn validate_runtime_config(config: RuntimeConfig) -> Result<(), String> {
    if !(1..=MAX_ASYNC_WORKERS).contains(&config.worker_threads) {
        return Err(format!(
            "async worker count must be between 1 and {MAX_ASYNC_WORKERS}"
        ));
    }
    if !(1..=MAX_BLOCKING_WORKERS).contains(&config.max_blocking_threads) {
        return Err(format!(
            "blocking worker count must be between 1 and {MAX_BLOCKING_WORKERS}"
        ));
    }
    if !(1..=MAX_TASKS).contains(&config.task_capacity) {
        return Err(format!(
            "async task capacity must be between 1 and {MAX_TASKS}"
        ));
    }
    Ok(())
}
