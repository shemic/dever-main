use std::future::Future;

use hyper::rt::Executor;

use crate::task::ScopedSpawner;

#[derive(Clone)]
pub(super) struct Http2Executor(pub(super) ScopedSpawner);

impl<F> Executor<F> for Http2Executor
where
    F: Future<Output = ()> + Send + 'static,
{
    fn execute(&self, future: F) {
        self.0.spawn(future);
    }
}
