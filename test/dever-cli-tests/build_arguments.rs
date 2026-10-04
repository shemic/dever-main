use super::{Action, BuildTarget, OsString, arguments_for};

#[test]
fn build_target_is_explicit_or_defaults_to_host() {
    for options in [
        vec!["--output", "program", "--target", "linux-aarch64"],
        vec!["--target", "linux-aarch64", "--output", "program"],
        vec!["--output", "program"],
    ] {
        let explicit = options.contains(&"--target");
        let arguments: Vec<_> = [vec!["build", "project"], options]
            .concat()
            .into_iter()
            .map(OsString::from)
            .collect();
        let (root, action) = arguments_for(&arguments).unwrap();
        assert_eq!(root.to_str(), Some("project"));
        let Action::Build { output, target } = action else {
            panic!("expected build");
        };
        assert_eq!(output.to_str(), Some("program"));
        assert_eq!(
            target,
            if explicit {
                BuildTarget::LinuxAarch64
            } else {
                BuildTarget::host().unwrap()
            }
        );
    }
}

#[test]
fn build_rejects_missing_unknown_duplicate_and_misplaced_options() {
    for options in [
        vec![],
        vec!["--output"],
        vec!["--target"],
        vec!["--target", "linux-aarch64"],
        vec!["--output", "program", "--target", "arm64"],
        vec![
            "--output",
            "program",
            "--target",
            "linux-aarch64",
            "--target",
            "linux-aarch64",
        ],
        vec!["--output", "program", "--output", "second"],
        vec!["--output", "--target", "linux-aarch64"],
        vec!["--output", "program", "--unknown"],
        vec!["--output", "program", "extra"],
        vec!["--output", ""],
    ] {
        let arguments: Vec<_> = [vec!["build", "project"], options]
            .concat()
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(arguments_for(&arguments).is_err(), "accepted {arguments:?}");
    }
    for command in ["run", "test", "check"] {
        let arguments = [command, "project", "--target", "linux-aarch64"].map(OsString::from);
        assert!(arguments_for(&arguments).is_err());
    }
}

#[test]
fn lib_target_keeps_specs_and_rejects_option_drift() {
    let arguments = [
        "lib",
        "add",
        "project",
        "pip:demo",
        "--target",
        "linux-aarch64",
    ]
    .map(OsString::from);
    let (_, action) = arguments_for(&arguments).unwrap();
    let Action::Lib { specs, target, .. } = action else {
        panic!("expected lib");
    };
    assert_eq!(specs, ["pip:demo"]);
    assert_eq!(target, BuildTarget::LinuxAarch64);
    for options in [
        vec!["--target"],
        vec!["--target", "arm64"],
        vec!["--unknown"],
        vec!["--target", "linux-aarch64", "--target", "linux-aarch64"],
    ] {
        let arguments: Vec<_> = [vec!["lib", "update", "project"], options]
            .concat()
            .into_iter()
            .map(OsString::from)
            .collect();
        assert!(arguments_for(&arguments).is_err());
    }
}
