// Each independent test target imports only the helpers relevant to its layer.
#![allow(dead_code)]

use std::ffi::OsString;
use std::process::Command;

use dever_core::hir::Program;
use dever_core::source::SourceMap;

pub mod temp;

#[cfg(all(feature = "api", any(feature = "sqlite", feature = "postgres")))]
pub mod authorization;

#[cfg(feature = "postgres")]
pub mod postgres;

pub fn sources(source: &str) -> SourceMap {
    let mut sources = SourceMap::default();
    sources.add("main.dever", source);
    sources
}

pub fn checked(sources: &SourceMap) -> Program {
    dever_core::check(sources).unwrap_or_else(|errors| {
        panic!(
            "{}",
            errors
                .iter()
                .map(|error| error.render(sources))
                .collect::<String>()
        )
    })
}

pub fn rejected(source: &str, message: &str) {
    let sources = sources(source);
    let errors = match dever_core::check(&sources) {
        Err(errors) => errors,
        Ok(_) => panic!("source must fail semantic checking:\n{source}"),
    };
    assert!(
        errors.iter().any(|error| error.message.contains(message)),
        "expected {message:?}: {}",
        errors
            .iter()
            .map(|error| error.render(&sources))
            .collect::<String>()
    );
}

pub fn run(source: &str) -> std::process::Output {
    let sources = sources(source);
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let native =
        dever_core::native::compile(&checked(&sources), &sources, "main.main", &rustc).unwrap();
    Command::new(native.executable()).output().unwrap()
}

pub fn stdout(source: &str) -> String {
    let output = run(source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
