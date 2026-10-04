#[path = "network_bench/config.rs"]
mod config;
#[path = "network_bench/load.rs"]
mod load;
#[path = "network_bench/metrics.rs"]
mod metrics;
#[path = "network_bench/response.rs"]
mod response;
#[path = "network_bench/service.rs"]
mod service;
#[path = "support/benchmark_settings.rs"]
mod settings;
#[path = "network_bench/tls.rs"]
mod tls;

use std::process::ExitCode;

use config::ServiceKind;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("network_bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    let Some(command) = arguments.next() else {
        return Err(usage());
    };
    if command == "check-config" && arguments.next().is_none() {
        return settings::Settings::check();
    }
    if command == "load" {
        let arguments = load::Arguments::parse(arguments.collect())?;
        return load::run(arguments);
    }
    let kind = ServiceKind::parse(&command)?;
    if arguments.next().is_some() {
        return Err(usage());
    }
    service::run(kind)
}

fn usage() -> String {
    concat!(
        "usage: network_bench runtime-http|runtime-https|runtime-http2|runtime-https2|",
        "hyper-http|hyper-https|hyper-http2|hyper-https2\n",
        "       network_bench check-config\n",
        "       network_bench load h1|h2 URL PATH RATE SECONDS PHYSICAL_CONNECTIONS ",
        "STREAMS_PER_CONNECTION TIMEOUT_MS REUSE CA"
    )
    .into()
}
