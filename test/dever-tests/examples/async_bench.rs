//! Same structured run/wait workload as the native Dever task fixture.

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Instant;

use dever_runtime::{bytes::Bytes, number, task};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("async_bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let [mode, workers, capacity, iterations] = arguments.as_slice() else {
        return Err("usage: async_bench root|worker WORKERS TASK_CAPACITY ITERATIONS".into());
    };
    let config = task::RuntimeConfig {
        worker_threads: bounded(workers, 64)?,
        max_blocking_threads: 2,
        task_capacity: bounded(capacity, task::MAX_TASKS)?,
    };
    let iterations = bounded(iterations, 1_048_576)?;
    match mode.as_str() {
        "root" => task::run_entry_with(config, batches(iterations)),
        // One outer structured task moves the batch onto a Tokio worker. It is
        // outside each timed sample and uses the unchanged production runtime.
        "worker" => task::run_entry_with(config, async move {
            let job = task::run(batches(iterations)).await?;
            task::wait(job).await
        }),
        _ => Err("mode must be root or worker".into()),
    }
}

fn bounded(value: &str, maximum: usize) -> Result<usize, String> {
    let number = value.parse::<usize>().map_err(|error| error.to_string())?;
    if !(1..=maximum).contains(&number) {
        return Err(format!("value must be between 1 and {maximum}"));
    }
    Ok(number)
}

async fn identity(value: i64) -> Result<i64, String> {
    Ok(value)
}

async fn task_sum(value: i64, state: i64) -> Result<i64, String> {
    let job = task::run(identity(value)).await?;
    number::int_add(state, task::wait(job).await?).map_err(str::to_owned)
}

fn print_line(line: &str) -> Result<(), String> {
    let mut output = io::stdout().lock();
    writeln!(output, "{line}").map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())
}

async fn batches(iterations: usize) -> Result<(), String> {
    let input = Bytes::new(vec![b'x'; iterations]);
    task::blocking(|| print_line("READY|0")).await?;
    for _ in 0..10 {
        let begin = Instant::now();
        let mut checksum = 0;
        for value in input.values() {
            checksum = task_sum(i64::from(*value), checksum).await?;
        }
        let elapsed = begin.elapsed().as_nanos();
        // As in the source fixture, all output and blocking dispatch are outside
        // the clock window. The caller independently verifies the checksum.
        task::blocking(move || {
            print_line(&format!("SAMPLE|tasks|{iterations}|{elapsed}|{checksum}"))
        })
        .await?;
    }
    Ok(())
}
