use std::io;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// Run only a test-owned process, and reap it on success, failure or timeout.
pub fn status(command: &mut Command, timeout: Duration) -> io::Result<ExitStatus> {
    let mut child = command.spawn()?;
    wait(&mut child, timeout)
}

/// Wait for a child whose caller owns concurrent interaction and panic cleanup.
pub fn wait(child: &mut Child, timeout: Duration) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                terminate(child)?;
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            terminate(child)?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "test-owned process exceeded its deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn terminate(child: &mut Child) -> io::Result<()> {
    let killed = child.kill();
    let reaped = child.wait();
    // Reap even if kill failed (for example, it raced with normal exit).
    reaped?;
    killed
}
