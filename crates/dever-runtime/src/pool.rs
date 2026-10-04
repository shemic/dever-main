#[cfg(feature = "sqlite")]
pub(crate) const WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub(crate) const CREATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub(crate) const RECYCLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) struct PoolObjectGuard<T> {
    object: Option<T>,
    detach: fn(T),
}

impl<T> PoolObjectGuard<T> {
    pub(crate) fn new(object: T, detach: fn(T)) -> Self {
        Self {
            object: Some(object),
            detach,
        }
    }

    pub(crate) fn object(&self) -> &T {
        self.object.as_ref().expect("active pooled object guard")
    }

    pub(crate) fn release(mut self) -> T {
        self.object.take().expect("active pooled object guard")
    }
}

impl<T> Drop for PoolObjectGuard<T> {
    fn drop(&mut self) {
        if let Some(object) = self.object.take() {
            (self.detach)(object);
        }
    }
}
