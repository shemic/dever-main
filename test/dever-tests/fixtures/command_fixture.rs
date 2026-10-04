//! Ordinary argv/stdin/stdout fixture: deliberately has no Dever protocol.
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            if matches!(
                std::env::args().nth(1).as_deref(),
                Some("tree" | "wait" | "heartbeat")
            ) && let Some(directory) = std::env::args().nth(2)
            {
                let _ = std::fs::write(
                    Path::new(&directory).join("fixture-error"),
                    error.to_string(),
                );
            }
            std::process::exit(91);
        }
    }
}

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("echo") => {
            for argument in arguments {
                std::io::stdout().write_all(argument.as_bytes())?;
                std::io::stdout().write_all(&[0])?;
            }
        }
        Some("stdin") => {
            std::io::copy(&mut std::io::stdin(), &mut std::io::stdout())?;
            std::io::stderr().write_all(&[0, 255, 128, 10])?;
        }
        Some("exit") => return Ok(arguments.next().ok_or("missing exit code")?.parse()?),
        Some("fds") => {
            let descriptors = std::fs::read_dir("/proc/self/fd")?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()?;
            for descriptor in descriptors {
                let Some(number) = descriptor
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                if number > 2 && std::fs::read_link(&descriptor).is_ok() {
                    writeln!(std::io::stdout(), "{number}")?;
                }
            }
        }
        Some("duplex") => {
            let count: usize = arguments.next().ok_or("missing output length")?.parse()?;
            let stdout = std::thread::spawn(move || std::io::stdout().write_all(&vec![254; count]));
            let stderr = std::thread::spawn(move || std::io::stderr().write_all(&vec![253; count]));
            let mut input = Vec::new();
            std::io::stdin().read_to_end(&mut input)?;
            stdout.join().map_err(|_| "stdout thread panicked")??;
            stderr.join().map_err(|_| "stderr thread panicked")??;
        }
        Some("wait") | Some("tree") => {
            let directory = arguments.next().ok_or("missing marker directory")?;
            let directory = Path::new(&directory);
            let mut starts = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(directory.join("starts"))?;
            starts.write_all(b"started\n")?;
            // Every lifecycle case gets a real descendant. PID namespace exit
            // must stop its heartbeat before the runtime returns cancellation.
            let mut child = std::process::Command::new(std::env::current_exe()?)
                .arg("heartbeat")
                .arg(directory)
                .spawn()?;
            std::fs::write(directory.join("ready"), b"ready")?;
            let status = child.wait()?;
            return Ok(status.code().unwrap_or(92));
        }
        Some("heartbeat") => {
            let directory = arguments.next().ok_or("missing marker directory")?;
            let lifetime = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(Path::new(&directory).join("lifetime.lock"))?;
            lifetime.lock()?;
            let marker = Path::new(&directory).join("heartbeat");
            for count in 0..3000 {
                std::fs::write(&marker, count.to_string())?;
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        _ => return Err("unknown ordinary command mode".into()),
    }
    Ok(0)
}
