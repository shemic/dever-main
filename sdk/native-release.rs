//! Private release-author entry point; never distributed as a user command.

use std::path::Path;

fn main() {
    if let Err(error) = run() {
        eprintln!("native-release: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let [author_root, flag, output] = arguments.as_slice() else {
        return Err("usage: native-release <author-root> --output <new-dir>".into());
    };
    if flag != "--output" {
        return Err("usage: native-release <author-root> --output <new-dir>".into());
    }
    let manifest =
        dever_cli::toolchain::packaging::create(Path::new(author_root), Path::new(output))?;
    println!(
        "created Dever {} for {}",
        manifest.version, manifest.platform
    );
    Ok(())
}
