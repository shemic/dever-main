use std::env;
use std::path::PathBuf;

fn main() {
    if env::args_os().skip(1).collect::<Vec<_>>().as_slice() == ["--dever-bootstrap-health"] {
        return;
    }
    let mut arguments = env::args_os().skip(1);
    let Some(flag) = arguments.next() else {
        eprintln!("Usage: deverd --root <machine-toolchain-root>");
        std::process::exit(2);
    };
    if flag != "--root" {
        eprintln!("Usage: deverd --root <machine-toolchain-root>");
        std::process::exit(2);
    }
    let Some(root) = arguments.next() else {
        eprintln!("deverd: --root requires a machine toolchain root");
        std::process::exit(2);
    };
    if arguments.next().is_some() {
        eprintln!("deverd: unexpected arguments");
        std::process::exit(2);
    }
    let layout = dever_cli::toolchain::Layout::new(PathBuf::from(root));
    let result = layout
        .initialize()
        .and_then(|()| layout.validate_machine_permissions())
        .and_then(|()| dever_cli::toolchain::serve(&layout));
    if let Err(error) = result {
        eprintln!("deverd: {error}");
        std::process::exit(1);
    }
}
