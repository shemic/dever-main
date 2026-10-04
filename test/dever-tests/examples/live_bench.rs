#[path = "live_bench/config.rs"]
mod config;
#[path = "live_bench/protocol.rs"]
mod protocol;
#[path = "live_bench/run.rs"]
mod run;
#[path = "support/benchmark_settings.rs"]
mod settings;

use std::process::ExitCode;

fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("live_bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["check-config"] {
        return settings::Settings::check();
    }
    let arguments = config::Arguments::parse(arguments)?;
    run::run(arguments)
}

pub(crate) fn usage() -> String {
    "usage: live_bench PROTOCOL URL CONNECTIONS CYCLES HOLD_MS TIMEOUT_MS\n       live_bench check-config".into()
}
