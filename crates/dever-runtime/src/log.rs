use std::collections::VecDeque;
use std::io::{self, BufWriter, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};

const QUEUE_CAPACITY: usize = 128;
const WRITER_STACK_BYTES: usize = 128 * 1024;
const MAX_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_FIELDS: usize = 16;
const MAX_FIELD_NAME_BYTES: usize = 128;
const MAX_FIELD_VALUE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    fn drops_when_full(self) -> bool {
        matches!(self, Self::Debug | Self::Info)
    }

    #[cfg(feature = "wire")]
    fn enabled(self, configured: crate::config::LogLevel) -> bool {
        let actual = match self {
            Self::Debug => 0,
            Self::Info => 1,
            Self::Warn => 2,
            Self::Error => 3,
        };
        let minimum = match configured {
            crate::config::LogLevel::Debug => 0,
            crate::config::LogLevel::Info => 1,
            crate::config::LogLevel::Warn => 2,
            crate::config::LogLevel::Error => 3,
        };
        actual >= minimum
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Field {
    pub name: String,
    pub value: String,
}

struct Record {
    level: Level,
    message: String,
    fields: Vec<Field>,
    dropped: u64,
}

enum Command {
    Record(Record),
    Flush,
}

struct Logger {
    queue: Mutex<Queue>,
    available: Condvar,
    space: Condvar,
    flushed: Condvar,
    flush_call: Mutex<()>,
    dropped: AtomicU64,
}

struct Queue {
    commands: VecDeque<Command>,
    closed: bool,
    flushed: bool,
}

struct WriterGuard<'a>(&'a Logger);

impl Drop for WriterGuard<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

static LOGGER: OnceLock<Result<Arc<Logger>, String>> = OnceLock::new();

pub fn debug(message: impl Into<String>, fields: Vec<Field>) {
    emit(Level::Debug, message, fields);
}

pub fn info(message: impl Into<String>, fields: Vec<Field>) {
    emit(Level::Info, message, fields);
}

pub fn warn(message: impl Into<String>, fields: Vec<Field>) {
    emit(Level::Warn, message, fields);
}

pub fn error(message: impl Into<String>, fields: Vec<Field>) {
    emit(Level::Error, message, fields);
}

pub fn emit(level: Level, message: impl Into<String>, fields: Vec<Field>) {
    #[cfg(feature = "wire")]
    if crate::config::try_current_settings()
        .is_some_and(|settings| !level.enabled(settings.log().level))
    {
        return;
    }
    #[cfg(feature = "api")]
    let fields = {
        let mut fields = fields;
        for field in crate::auth::log_fields() {
            if !fields.iter().any(|existing| existing.name == field.name) {
                fields.push(field);
            }
        }
        fields
    };
    let record = Record {
        level,
        message: truncate(message.into(), MAX_MESSAGE_BYTES),
        fields: fields
            .into_iter()
            .take(MAX_FIELDS)
            .map(|field| Field {
                name: truncate(field.name, MAX_FIELD_NAME_BYTES),
                value: truncate(field.value, MAX_FIELD_VALUE_BYTES),
            })
            .collect(),
        dropped: 0,
    };
    match logger() {
        Ok(logger) => logger.submit(record),
        Err(error) => emergency(&Record {
            level: Level::Error,
            message: error.clone(),
            fields: Vec::new(),
            dropped: 0,
        }),
    }
}

pub fn flush() {
    if let Ok(logger) = logger() {
        logger.flush();
    }
}

pub fn dropped_events() -> u64 {
    logger()
        .as_ref()
        .map(|logger| logger.dropped.load(Ordering::Relaxed))
        .unwrap_or_default()
}

fn logger() -> &'static Result<Arc<Logger>, String> {
    LOGGER.get_or_init(|| {
        let logger = Arc::new(Logger::new());
        let writer = Arc::clone(&logger);
        std::thread::Builder::new()
            .name("dever-log".into())
            .stack_size(WRITER_STACK_BYTES)
            .spawn(move || {
                let mut output = BufWriter::new(io::stderr().lock());
                writer.write(&mut output);
            })
            .map_err(|error| format!("cannot start Dever log writer: {error}"))?;
        Ok(logger)
    })
}

impl Logger {
    fn new() -> Self {
        Self {
            queue: Mutex::new(Queue {
                commands: VecDeque::with_capacity(QUEUE_CAPACITY),
                closed: false,
                flushed: false,
            }),
            available: Condvar::new(),
            space: Condvar::new(),
            flushed: Condvar::new(),
            flush_call: Mutex::new(()),
            dropped: AtomicU64::new(0),
        }
    }

    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn wait_for_space<'a>(&self, mut queue: MutexGuard<'a, Queue>) -> MutexGuard<'a, Queue> {
        while !queue.closed && queue.commands.len() == QUEUE_CAPACITY {
            queue = self
                .space
                .wait(queue)
                .unwrap_or_else(|error| error.into_inner());
        }
        queue
    }

    fn submit(&self, mut record: Record) {
        record.dropped = self.dropped.swap(0, Ordering::Relaxed);
        let mut queue = self.queue();
        if !queue.closed && queue.commands.len() == QUEUE_CAPACITY && record.level.drops_when_full()
        {
            self.dropped
                .fetch_add(record.dropped.saturating_add(1), Ordering::Relaxed);
            return;
        }
        queue = self.wait_for_space(queue);
        if queue.closed {
            drop(queue);
            emergency(&record);
            return;
        }
        queue.commands.push_back(Command::Record(record));
        self.available.notify_one();
    }

    fn flush(&self) {
        // A single pending flush pairs the fixed acknowledgement with its
        // caller. Queue storage and waiting state never allocate after startup.
        let _caller = self
            .flush_call
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut queue = self.wait_for_space(self.queue());
        if queue.closed {
            return;
        }
        queue.flushed = false;
        queue.commands.push_back(Command::Flush);
        self.available.notify_one();
        while !queue.flushed && !queue.closed {
            queue = self
                .flushed
                .wait(queue)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn receive(&self) -> Option<Command> {
        let mut queue = self.queue();
        while queue.commands.is_empty() && !queue.closed {
            queue = self
                .available
                .wait(queue)
                .unwrap_or_else(|error| error.into_inner());
        }
        let command = queue.commands.pop_front();
        self.space.notify_one();
        command
    }

    fn write(&self, output: &mut impl Write) {
        let _guard = WriterGuard(self);
        while let Some(command) = self.receive() {
            match command {
                Command::Record(record) => {
                    if write_record(output, &record).is_err() {
                        emergency(&record);
                    }
                }
                Command::Flush => {
                    let _ = output.flush();
                    let mut queue = self.queue();
                    queue.flushed = true;
                    self.flushed.notify_one();
                }
            }
        }
        let _ = output.flush();
    }

    fn close(&self) {
        let mut queue = self.queue();
        queue.closed = true;
        queue.commands.clear();
        self.available.notify_all();
        self.space.notify_all();
        self.flushed.notify_all();
    }
}

fn write_record(output: &mut impl Write, record: &Record) -> io::Result<()> {
    write!(
        output,
        "{{\"level\":\"{}\",\"message\":",
        record.level.name()
    )?;
    write_json_string(output, &record.message)?;
    if record.dropped > 0 {
        write!(output, ",\"dropped_events\":{}", record.dropped)?;
    }
    write!(output, ",\"fields\":{{")?;
    for (index, field) in record.fields.iter().enumerate() {
        if index > 0 {
            output.write_all(b",")?;
        }
        write_json_string(output, &field.name)?;
        output.write_all(b":")?;
        write_json_string(output, &field.value)?;
    }
    output.write_all(b"}}\n")?;
    if record.level == Level::Error {
        output.flush()?;
    }
    Ok(())
}

fn write_json_string(output: &mut impl Write, value: &str) -> io::Result<()> {
    output.write_all(b"\"")?;
    for character in value.chars() {
        match character {
            '"' => output.write_all(b"\\\"")?,
            '\\' => output.write_all(b"\\\\")?,
            '\n' => output.write_all(b"\\n")?,
            '\r' => output.write_all(b"\\r")?,
            '\t' => output.write_all(b"\\t")?,
            character if character.is_control() => write!(output, "\\u{:04x}", character as u32)?,
            character => write!(output, "{character}")?,
        }
    }
    output.write_all(b"\"")
}

fn emergency(record: &Record) {
    let mut output = io::stderr().lock();
    let _ = write_record(&mut output, record);
}

fn truncate(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let end = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or(0);
    value.truncate(end);
    value
}

#[cfg(test)]
#[path = "../../../test/runtime_logging.rs"]
mod tests;
