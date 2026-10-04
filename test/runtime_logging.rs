use super::*;
use std::sync::mpsc;
use std::time::Duration;

fn record(level: Level, message: impl Into<String>) -> Record {
    Record {
        level,
        message: message.into(),
        fields: Vec::new(),
        dropped: 0,
    }
}

fn next_record(logger: &Logger) -> Record {
    match logger.receive() {
        Some(Command::Record(record)) => record,
        _ => panic!("expected a queued record"),
    }
}

#[test]
fn bounded_queue_keeps_fifo_and_carries_dropped_count_to_next_record() {
    let logger = Logger::new();
    for index in 0..QUEUE_CAPACITY {
        logger.submit(record(Level::Info, index.to_string()));
    }
    logger.submit(record(Level::Debug, "discard debug"));
    logger.submit(record(Level::Info, "discard info"));
    assert_eq!(logger.queue().commands.len(), QUEUE_CAPACITY);
    assert_eq!(logger.dropped.load(Ordering::Relaxed), 2);
    assert_eq!(next_record(&logger).message, "0");
    logger.submit(record(Level::Info, "after overflow"));
    for index in 1..QUEUE_CAPACITY {
        assert_eq!(next_record(&logger).message, index.to_string());
    }
    let recovered = next_record(&logger);
    assert_eq!(recovered.message, "after overflow");
    assert_eq!(recovered.dropped, 2);
    assert_eq!(logger.dropped.load(Ordering::Relaxed), 0);
    let mut output = Vec::new();
    write_record(&mut output, &recovered).unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "{\"level\":\"info\",\"message\":\"after overflow\",\"dropped_events\":2,\"fields\":{}}\n"
    );
}

#[test]
fn warn_and_error_wait_for_capacity_without_being_dropped() {
    for level in [Level::Warn, Level::Error] {
        let logger = Arc::new(Logger::new());
        for index in 0..QUEUE_CAPACITY {
            logger.submit(record(Level::Info, index.to_string()));
        }
        let writer = Arc::clone(&logger);
        let (started, starting) = mpsc::channel();
        let (finished, finishing) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            started.send(()).unwrap();
            writer.submit(record(level, "must survive"));
            finished.send(()).unwrap();
        });
        starting.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            finishing.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(next_record(&logger).message, "0");
        finishing.recv_timeout(Duration::from_secs(1)).unwrap();
        producer.join().unwrap();
        for index in 1..QUEUE_CAPACITY {
            assert_eq!(next_record(&logger).message, index.to_string());
        }
        let important = next_record(&logger);
        assert_eq!(important.message, "must survive");
        assert_eq!(important.level, level);
        assert_eq!(important.dropped, 0);
    }
}

#[derive(Clone, Default)]
struct CapturedOutput {
    bytes: Arc<Mutex<Vec<u8>>>,
    flushes: Arc<AtomicU64>,
}

impl Write for CapturedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[test]
fn concurrent_flushes_acknowledge_each_callers_prior_record() {
    let logger = Arc::new(Logger::new());
    let output = CapturedOutput::default();
    let writer = Arc::clone(&logger);
    let mut captured = output.clone();
    let writing = std::thread::spawn(move || writer.write(&mut captured));
    std::thread::scope(|scope| {
        for caller in 0..8 {
            let logger = &logger;
            let output = &output;
            scope.spawn(move || {
                for sequence in 0..8 {
                    let message = format!("caller-{caller}-record-{sequence}");
                    logger.submit(record(Level::Warn, message.clone()));
                    logger.flush();
                    let bytes = output.bytes.lock().unwrap();
                    let text = std::str::from_utf8(&bytes).unwrap();
                    assert!(text.contains(&format!("\"message\":\"{message}\"")));
                }
            });
        }
    });
    assert_eq!(output.flushes.load(Ordering::Relaxed), 64);
    logger.close();
    writing.join().unwrap();
}

struct PanickingOutput;

impl Write for PanickingOutput {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        panic!("fixture writer stopped")
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn writer_failure_releases_queue_and_wakes_producers_and_flushers() {
    let logger = Arc::new(Logger::new());
    for index in 0..QUEUE_CAPACITY {
        logger.submit(record(Level::Info, index.to_string()));
    }
    let (finished, finishing) = mpsc::channel();
    let mut waiting = Vec::new();
    for index in 0..3 {
        let logger = Arc::clone(&logger);
        let finished = finished.clone();
        waiting.push(std::thread::spawn(move || {
            if index == 0 {
                logger.submit(record(Level::Warn, "writer closed"));
            } else {
                logger.flush();
            }
            finished.send(()).unwrap();
        }));
    }
    let writer = Arc::clone(&logger);
    let writing = std::thread::spawn(move || writer.write(&mut PanickingOutput));
    assert!(writing.join().is_err());
    for _ in 0..3 {
        finishing.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    for thread in waiting {
        thread.join().unwrap();
    }
    let queue = logger.queue();
    assert!(queue.closed);
    assert!(queue.commands.is_empty());
}
