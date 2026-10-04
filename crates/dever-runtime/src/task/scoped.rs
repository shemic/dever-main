use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, oneshot};

use super::{
    CURRENT_SCOPE, CompleteOnDrop, Completion, Registration, Scope, TaskOutcome, start_task,
    supervise_with,
};

/// Completion of an externally supervised resource still belongs to the
/// requesting task: cancellation must drain it without aborting its cleanup.
#[cfg(feature = "wire")]
pub(crate) fn scoped_completion() -> Result<ScopedCompletion, String> {
    let scope = super::current_scope()?;
    let completion = Completion::new();
    let id = scope.register(None, completion.clone());
    Ok(ScopedCompletion {
        registration: Some(Registration { scope, id }),
        completion,
    })
}

#[cfg(feature = "wire")]
pub(crate) struct ScopedCompletion {
    registration: Option<Registration>,
    completion: Completion,
}

#[cfg(feature = "wire")]
impl Drop for ScopedCompletion {
    fn drop(&mut self) {
        if let Some(registration) = self.registration.take() {
            registration.finish();
        }
        self.completion.complete();
    }
}

pub(crate) fn scoped_owner<G>(guard: G) -> Result<(ScopedOwner, ScopedSpawner, ScopeFault), String>
where
    G: Send + 'static,
{
    let parent = super::current_scope()?;
    Ok(scoped_owner_in(parent, guard))
}

fn scoped_owner_in<G>(parent: Scope, guard: G) -> (ScopedOwner, ScopedSpawner, ScopeFault)
where
    G: Send + 'static,
{
    let scope = parent.child();
    let completion = Completion::new();
    let registration = Registration {
        id: parent.register(None, completion.clone()),
        scope: parent,
    };
    let admission = Arc::new(Mutex::new(true));
    let fault = ScopeFault::new();
    let task_slots = Arc::clone(&scope.limits.task_slots);
    (
        ScopedOwner {
            scope: scope.clone(),
            admission: Arc::clone(&admission),
            fault: fault.clone(),
            registration: Some(registration),
            completion: Some(completion),
            guard: Some(Box::new(guard)),
        },
        ScopedSpawner {
            scope,
            admission,
            fault: fault.clone(),
            task_slots,
        },
        fault,
    )
}

pub(crate) struct ScopedOwner {
    scope: Scope,
    admission: Arc<Mutex<bool>>,
    fault: ScopeFault,
    registration: Option<Registration>,
    completion: Option<Completion>,
    guard: Option<Box<dyn Send>>,
}

impl ScopedOwner {
    pub(crate) async fn enter<F>(&mut self, future: F) -> F::Output
    where
        F: Future,
    {
        CURRENT_SCOPE.scope(self.scope.clone(), future).await
    }

    pub(crate) async fn finish(mut self) -> Result<(), String> {
        self.start_cleanup(false, true)
            .expect("scoped owner cleaned once")
            .await
            .unwrap_or_else(|_| Err("scoped task cleanup stopped".into()))
    }

    pub(crate) async fn stop(mut self) -> Result<(), String> {
        self.start_cleanup(true, true)
            .expect("scoped owner cleaned once")
            .await
            .unwrap_or_else(|_| Err("scoped task cleanup stopped".into()))
    }

    pub(crate) fn finish_in_background(mut self) {
        let _ = self.start_cleanup(false, false);
    }

    pub(crate) fn stop_in_background(mut self) {
        let _ = self.start_cleanup(true, false);
    }

    fn close_admission(&self) {
        *self
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = false;
    }

    fn start_cleanup(
        &mut self,
        stopping: bool,
        cancel_if_unobserved: bool,
    ) -> Option<oneshot::Receiver<Result<(), String>>> {
        self.close_admission();
        let completion = self.completion.take()?;
        let (mut sender, receiver) = oneshot::channel();
        let scope = self.scope.clone();
        let registration = self.registration.take();
        let guard = self.guard.take();
        let fault = self.fault.clone();
        tokio::spawn(async move {
            let complete = CompleteOnDrop(completion);
            if stopping {
                scope.cancel_and_wait().await;
            } else if cancel_if_unobserved {
                tokio::select! {
                    _ = scope.wait() => {}
                    _ = sender.closed() => scope.cancel_and_wait().await,
                }
            } else {
                scope.wait().await;
            }
            drop(guard);
            if let Some(registration) = registration {
                registration.finish();
            }
            let result = fault.result();
            drop(complete);
            let _ = sender.send(result);
        });
        Some(receiver)
    }
}

impl Drop for ScopedOwner {
    fn drop(&mut self) {
        let _ = self.start_cleanup(true, true);
    }
}

#[derive(Clone)]
pub(crate) struct ScopedSpawner {
    scope: Scope,
    admission: Arc<Mutex<bool>>,
    fault: ScopeFault,
    task_slots: Arc<tokio::sync::Semaphore>,
}

impl ScopedSpawner {
    pub(crate) fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let accepting = self
            .admission
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !*accepting {
            return;
        }
        let permit = match Arc::clone(&self.task_slots)
            .try_acquire_owned()
            .map_err(|_| "async task capacity is exhausted".to_owned())
        {
            Ok(permit) => permit,
            Err(error) => {
                self.fault.report(error);
                return;
            }
        };
        let started = start_task(
            &self.scope,
            async move {
                future.await;
                Ok(())
            },
            permit,
        );
        let scope = self.scope.clone();
        let fault = self.fault.clone();
        tokio::spawn(supervise_with(started, move |id, outcome| {
            match outcome {
                TaskOutcome::Finished(Err(error)) | TaskOutcome::Panicked(error) => {
                    fault.report(error.clone());
                }
                TaskOutcome::Finished(Ok(())) | TaskOutcome::Stopped => {}
            }
            scope.unregister(id);
        }));
    }
}

#[derive(Clone)]
pub(crate) struct ScopeFault(Arc<FaultState>);

struct FaultState {
    first: Mutex<Option<String>>,
    changed: Notify,
}

impl ScopeFault {
    fn new() -> Self {
        Self(Arc::new(FaultState {
            first: Mutex::new(None),
            changed: Notify::new(),
        }))
    }

    pub(crate) fn report(&self, error: String) {
        let mut first = self
            .0
            .first
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if first.is_none() {
            *first = Some(error);
            drop(first);
            self.0.changed.notify_one();
        }
    }

    pub(crate) async fn wait(&self) -> String {
        loop {
            let changed = self.0.changed.notified();
            if let Some(error) = self.current() {
                return error;
            }
            changed.await;
        }
    }

    fn result(&self) -> Result<(), String> {
        self.current().map_or(Ok(()), Err)
    }

    fn current(&self) -> Option<String> {
        self.0
            .first
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}
