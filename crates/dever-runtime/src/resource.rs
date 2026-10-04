use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use crate::bytes::Bytes;

pub enum Pull<T> {
    Item(T),
    Last(T),
    End,
}

struct StreamState<T> {
    producer: Option<Box<dyn FnMut() -> Pull<T>>>,
}

pub struct Stream<T>(Arc<Mutex<StreamState<T>>>);

impl<T> Clone for Stream<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> std::fmt::Debug for Stream<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Stream(<resource>)")
    }
}

impl<T: 'static> Stream<T> {
    pub fn new(producer: impl FnMut() -> Pull<T> + 'static) -> Self {
        Self(Arc::new(Mutex::new(StreamState {
            producer: Some(Box::new(producer)),
        })))
    }

    fn pull_next(&self) -> Pull<T> {
        let mut state = self.0.lock().expect("stream lock poisoned");
        let Some(producer) = state.producer.as_mut() else {
            return Pull::End;
        };
        let result = producer.as_mut()();
        if matches!(&result, Pull::Last(_) | Pull::End) {
            state.producer = None;
        }
        result
    }

    pub fn pull(&self) -> Option<T> {
        match self.pull_next() {
            Pull::Item(value) | Pull::Last(value) => Some(value),
            Pull::End => None,
        }
    }

    pub fn close(&self) {
        self.0.lock().expect("stream lock poisoned").producer = None;
    }

    pub fn map<U: 'static>(self, mut transform: impl FnMut(T) -> U + 'static) -> Stream<U> {
        Stream::new(move || match self.pull_next() {
            Pull::Item(value) => Pull::Item(transform(value)),
            Pull::Last(value) => Pull::Last(transform(value)),
            Pull::End => Pull::End,
        })
    }
}

#[derive(Debug)]
pub struct Resource<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Resource<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Resource<T> {
    pub fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new(Some(value))))
    }

    pub fn close(&self) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "resource lock failed".to_owned())?
            .take()
            .map(|_| ())
            .ok_or_else(|| "resource is closed".to_owned())
    }

    pub fn is_open(&self) -> Result<bool, String> {
        self.0
            .lock()
            .map(|resource| resource.is_some())
            .map_err(|_| "resource lock failed".to_owned())
    }

    fn with<R>(&self, operation: impl FnOnce(&mut T) -> std::io::Result<R>) -> Result<R, String> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| "resource lock failed".to_owned())?;
        let resource = guard
            .as_mut()
            .ok_or_else(|| "resource is closed".to_owned())?;
        operation(resource).map_err(|error| error.to_string())
    }
}

pub type FileHandle = Resource<File>;

pub fn file_open(path: &str) -> Result<FileHandle, String> {
    File::open(path)
        .map(Resource::new)
        .map_err(|error| error.to_string())
}

pub fn file_create(path: &str) -> Result<FileHandle, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(Resource::new)
        .map_err(|error| error.to_string())
}

pub fn read<T: Read>(resource: &Resource<T>, limit: i64) -> Result<Option<Bytes>, String> {
    let limit = read_limit(limit)?;
    resource.with(|source| {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(limit)
            .map_err(std::io::Error::other)?;
        bytes.resize(limit, 0);
        let length = source.read(&mut bytes)?;
        bytes.truncate(length);
        Ok((length != 0).then(|| Bytes::new(bytes)))
    })
}

pub fn read_chunks<T: Read + 'static>(
    resource: Resource<T>,
    limit: i64,
) -> Result<Stream<Result<Bytes, String>>, String> {
    read_limit(limit)?;
    if !resource.is_open()? {
        return Err("resource is closed".to_owned());
    }
    Ok(Stream::new(move || match read(&resource, limit) {
        Ok(Some(bytes)) => Pull::Item(Ok(bytes)),
        Ok(None) => Pull::End,
        Err(message) => Pull::Last(Err(message)),
    }))
}

pub(crate) fn read_limit(limit: i64) -> Result<usize, String> {
    usize::try_from(limit)
        .ok()
        .filter(|limit| *limit > 0)
        .ok_or_else(|| "read size must be a positive Int".to_owned())
}

pub fn write<T: Write>(resource: &Resource<T>, bytes: &Bytes) -> Result<(), String> {
    resource.with(|output| {
        output.write_all(bytes.values())?;
        output.flush()
    })
}

pub fn stdout_write(text: &str) -> Result<(), String> {
    let mut output = std::io::stdout().lock();
    output
        .write_all(text.as_bytes())
        .and_then(|_| output.flush())
        .map_err(|error| format!("standard output failed: {error}"))
}
