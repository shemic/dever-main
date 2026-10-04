//! Application resources close after child drain, before the runtime is gone.

use dever_runtime::task::{self, RuntimeConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

struct DropWitness(Arc<AtomicBool>);

impl Drop for DropWitness {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn config() -> RuntimeConfig {
    RuntimeConfig {
        worker_threads: 1,
        max_blocking_threads: 2,
        task_capacity: 4,
    }
}

#[test]
fn cleanup_runs_after_pending_child_release_inside_the_same_runtime() {
    let dropped = Arc::new(AtomicBool::new(false));
    let witness = DropWitness(Arc::clone(&dropped));
    let result = task::run_entry_with_typed_cleanup(
        config(),
        async move {
            let child = task::run(async move {
                let _witness = witness;
                std::future::pending::<Result<(), String>>().await
            })
            .await?;
            tokio::task::yield_now().await;
            // Return the handle: the child is still live until root Scope drain.
            Ok::<_, String>(child)
        },
        |result| async {
            assert!(dropped.load(Ordering::Acquire));
            assert!(tokio::runtime::Handle::try_current().is_ok());
            tokio::time::sleep(Duration::from_millis(1)).await;
            result
        },
    );
    assert!(result.is_ok());
    drop(result);
}

#[test]
fn entry_and_cleanup_keep_borrowed_non_send_state_across_suspension() {
    let phase = std::rc::Rc::new(std::cell::Cell::new(0));
    let label = String::from("borrowed result");
    let result = task::run_entry_with_typed_cleanup(
        config(),
        async {
            let phase = std::rc::Rc::clone(&phase);
            tokio::task::yield_now().await;
            phase.set(1);
            Ok::<_, String>(label.as_str())
        },
        |result| async {
            let phase = std::rc::Rc::clone(&phase);
            tokio::task::yield_now().await;
            assert_eq!(phase.get(), 1);
            phase.set(2);
            result
        },
    );
    assert_eq!(result.unwrap(), label);
    assert_eq!(phase.get(), 2);
}

#[derive(Debug, Eq, PartialEq)]
enum Failure {
    Business { message: String },
    Runtime(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Runtime(message)
    }
}

#[test]
fn cleanup_receives_and_preserves_the_original_typed_failure() {
    let expected = Failure::Business {
        message: "original owned payload".into(),
    };
    let result = task::run_entry_with_typed_cleanup(
        config(),
        async { Err::<(), _>(expected) },
        |result| async {
            assert_eq!(
                result,
                Err(Failure::Business {
                    message: "original owned payload".into(),
                })
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
            result
        },
    );
    assert_eq!(
        result,
        Err(Failure::Business {
            message: "original owned payload".into(),
        })
    );
}

#[test]
fn rejected_nested_entry_does_not_run_body_or_cleanup() {
    let body_started = AtomicBool::new(false);
    let cleanup_started = AtomicBool::new(false);
    let result = task::run_entry(async {
        task::run_entry_with_typed_cleanup(
            config(),
            async {
                body_started.store(true, Ordering::Relaxed);
                Ok::<(), String>(())
            },
            |result| async {
                cleanup_started.store(true, Ordering::Relaxed);
                result
            },
        )
    });
    assert!(result.unwrap_err().contains("inside another runtime"));
    assert!(!body_started.load(Ordering::Relaxed));
    assert!(!cleanup_started.load(Ordering::Relaxed));
}
