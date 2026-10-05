use std::ffi::OsString;
use std::io::{self, Write};

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments.as_slice() == ["--dever-bootstrap-health"] {
        return 0;
    }
    if let [flag, config] = arguments.as_slice()
        && flag == "--dever-extract"
    {
        return match dever_cli::toolchain::extract_config(std::path::Path::new(config)) {
            Ok(()) => 0,
            Err(error) => fail(error),
        };
    }
    #[cfg(target_os = "linux")]
    if let [flag, root] = arguments.as_slice()
        && flag == "--dever-install"
    {
        return match dever_cli::toolchain::bootstrap::install(std::path::Path::new(root)) {
            Ok(version) => {
                println!("installed Dever {version}");
                0
            }
            Err(error) => fail(error),
        };
    }
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => return fail(format!("cannot locate dever launcher: {error}")),
    };
    match dever_cli::toolchain::execute(&executable, &arguments) {
        Ok(dever_cli::toolchain::CommandResult::Exited(code)) => code,
        Ok(dever_cli::toolchain::CommandResult::Message(message)) => {
            match writeln!(io::stdout().lock(), "{message}") {
                Ok(()) => 0,
                Err(error) => fail(format!("cannot write command output: {error}")),
            }
        }
        Err(error) => fail(error),
    }
}

fn fail(error: String) -> i32 {
    let _ = writeln!(io::stderr().lock(), "{error}");
    1
}
