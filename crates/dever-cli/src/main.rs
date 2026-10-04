use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use dever_cli::libs;
use dever_cli::toolchain::BuildTarget;
use dever_core::source::SourceMap;

mod api;
mod compile;
mod format;
mod test_runner;
mod worker;

#[cfg(test)]
#[path = "../../../test/dever-cli-tests/build_arguments.rs"]
mod build_arguments;

const USAGE: &str = "Usage:\n  dever check <project-root>\n  dever api <project-root> [--output <new-file>]\n  dever fmt <project-root> [--check]\n  dever test <project-root>\n  dever run <project-root> [-- <component>.<domain>.<cmd> '<json-object>']\n  dever run <project-root> --tenant <positive-tenant-id> -- <component>.<domain>.<cmd> '<json-object>'\n  dever build <project-root> --output <new-file> [--target linux-x86_64|linux-aarch64]\n  dever clean <project-root>\n  dever tenant migrate <project-root> <positive-tenant-id>\n  dever tenant owner <project-root> <positive-tenant-id> <site> <positive-user-id>\n  dever tenant component enable <project-root> <positive-tenant-id> <component>\n  dever tenant component disable <project-root> <positive-tenant-id> <component>\n  dever lib add|list|update|remove|doctor <project-root> [--target linux-x86_64|linux-aarch64] [spec ...]\n  dever package add|list|update|remove|doctor <project-root> [spec ...]\n\nApplications use API, Job, and CMD declarations under module/ and config/setting.json. Native builds and tests are offline.";

enum Action {
    Check,
    Api {
        output: Option<PathBuf>,
    },
    Format {
        check: bool,
    },
    Test,
    Run {
        tenant_id: Option<i64>,
        arguments: Vec<OsString>,
    },
    Build {
        output: PathBuf,
        target: BuildTarget,
    },
    Clean,
    TenantMigrate {
        tenant_id: i64,
    },
    TenantOwner {
        tenant_id: i64,
        site: String,
        user_id: i64,
    },
    TenantComponent {
        tenant_id: i64,
        component: String,
        enabled: bool,
    },
    Lib {
        command: String,
        specs: Vec<String>,
        target: BuildTarget,
    },
    Package {
        command: String,
        specs: Vec<String>,
    },
}

impl Action {
    fn loads_settings(&self) -> bool {
        matches!(self, Self::Check | Self::Api { .. }) || self.compiles_application()
    }

    fn compiles_application(&self) -> bool {
        matches!(
            self,
            Self::Run { .. }
                | Self::Build { .. }
                | Self::TenantMigrate { .. }
                | Self::TenantOwner { .. }
                | Self::TenantComponent { .. }
        )
    }
}

fn main() -> ExitCode {
    match execute() {
        Ok(code) => code,
        Err(error) => {
            // Preserve failure status even if stderr is unavailable.
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

fn execute() -> Result<ExitCode, String> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    if arguments.first().is_some_and(|argument| argument == "new") {
        let (destination, markdown) = match &arguments[1..] {
            [destination] if !destination.to_string_lossy().starts_with('-') => {
                (destination, false)
            }
            [destination, option]
                if !destination.to_string_lossy().starts_with('-') && option == "--markdown" =>
            {
                (destination, true)
            }
            _ => return Err("Usage: dever new <new-directory> [--markdown]".into()),
        };
        dever_cli::project::create(std::path::Path::new(destination), markdown)?;
        writeln!(
            io::stdout().lock(),
            "created Dever project {}",
            destination.to_string_lossy()
        )
        .map_err(|error| error.to_string())?;
        return Ok(ExitCode::SUCCESS);
    }
    if arguments.as_slice() == [OsString::from("--dever-compile-worker")] {
        worker::execute()?;
        return Ok(ExitCode::SUCCESS);
    }
    if arguments.as_slice() == [OsString::from("--dever-health")] {
        return Ok(ExitCode::SUCCESS);
    }
    if arguments.as_slice() == [OsString::from("--version")] {
        writeln!(io::stdout().lock(), "{}", env!("CARGO_PKG_VERSION"))
            .map_err(|error| error.to_string())?;
        return Ok(ExitCode::SUCCESS);
    }
    if arguments.as_slice() == [OsString::from("--help")] {
        writeln!(
            io::stdout().lock(),
            "Create a project: dever new <new-directory> [--markdown]"
        )
        .map_err(|error| error.to_string())?;
        writeln!(io::stdout().lock(), "{USAGE}").map_err(|error| error.to_string())?;
        return Ok(ExitCode::SUCCESS);
    }
    let (root, action) = arguments_for(&arguments)?;
    if let Action::Lib {
        command,
        specs,
        target,
    } = &action
    {
        let message = dever_cli::libs::execute_for_target(command, &root, specs, *target)?;
        if !message.is_empty() {
            writeln!(io::stdout().lock(), "{message}").map_err(|error| error.to_string())?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if let Action::Package { command, specs } = &action {
        let message = dever_cli::packages::execute(command, &root, specs)?;
        if !message.is_empty() {
            writeln!(io::stdout().lock(), "{message}").map_err(|error| error.to_string())?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if matches!(action, Action::Clean) {
        let summary = dever_core::native::clean_project_artifacts(&root)
            .map_err(|error| format!("cannot clean Dever artifacts: {error}"))?;
        writeln!(
            io::stdout().lock(),
            "cleaned {} directories, {} files, {} bytes",
            summary.directories,
            summary.files,
            summary.bytes,
        )
        .map_err(|error| error.to_string())?;
        return Ok(ExitCode::SUCCESS);
    }
    let source_root = root.join("module");
    let packages = if matches!(action, Action::Format { .. }) {
        Vec::new()
    } else {
        dever_cli::packages::sources(&root)?
    };
    let sources = SourceMap::load_with_packages(
        &source_root,
        matches!(action, Action::Test | Action::Format { .. })
            .then(|| root.join("test"))
            .as_deref(),
        &packages,
    )
    .map_err(|errors| {
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    if !sources.files().iter().any(|source| !source.is_test()) {
        return Err(format!(
            "{}: no .dever or .dever.md source files",
            source_root.display()
        ));
    }
    if action.compiles_application()
        && sources.files().iter().any(|source| {
            source
                .path()
                .file_name()
                .is_some_and(|name| name == "main.dever" || name == "main.dever.md")
        })
    {
        return Err("application entry is generated from API, CMD, and Job declarations; remove module/main.dever".into());
    }
    if let Action::Format { check } = action {
        return format::execute(&root, &sources, check);
    }
    let settings = action
        .loads_settings()
        .then(|| dever_runtime::config::Settings::load_project(&root))
        .transpose()?;
    let checked = match settings.as_ref() {
        Some(settings) => dever_core::check_with_settings(&sources, settings),
        None => dever_core::check(&sources),
    };
    let program = checked.map_err(|errors| {
        errors
            .iter()
            .map(|error| error.render(&sources))
            .collect::<String>()
    })?;
    for warning in program.warnings() {
        if SourceMap::is_standard(warning.primary.source) {
            continue;
        }
        write!(io::stderr().lock(), "{}", warning.render(&sources))
            .map_err(|error| error.to_string())?;
    }
    if let Action::Api { output } = action {
        return api::write(&program, output.as_deref());
    }
    api::check_baseline(&program, &root)?;
    if matches!(action, Action::Check) {
        return Ok(ExitCode::SUCCESS);
    }
    if matches!(action, Action::Test) {
        return test_runner::execute(&program, &sources);
    }
    let target = match &action {
        Action::Build { target, .. } => *target,
        _ => BuildTarget::host()?,
    };
    let external_report = if action.compiles_application() {
        libs::prepare_program(&root, &program, target)?
    } else {
        None
    };
    if action.compiles_application() {
        let errors = program.application_errors()?;
        if !errors.is_empty() {
            return Err(errors.iter().map(|error| error.render(&sources)).collect());
        }
    }
    match &action {
        Action::Check => unreachable!("check returned before target selection"),
        Action::Lib { .. } => unreachable!("lib returned before source checking"),
        Action::Package { .. } => unreachable!("package returned before source checking"),
        Action::Api { .. } => unreachable!("api returned before baseline checking"),
        Action::Format { .. } => unreachable!("fmt returned before checking"),
        Action::Test => unreachable!("test returned before application compilation"),
        Action::Run { .. }
        | Action::Build { .. }
        | Action::TenantMigrate { .. }
        | Action::TenantOwner { .. }
        | Action::TenantComponent { .. } => {}
        Action::Clean => unreachable!("clean returned before source loading"),
    }
    let settings = settings
        .as_ref()
        .expect("application action loaded settings before checking");
    program.validate_database_settings(settings)?;
    let resources = libs::prepare_resources(&root, &program, target)?;
    let native = compile::application(
        &program,
        &sources,
        &settings.compilation_bindings(),
        &resources,
        target,
    )?;
    match action {
        Action::Package { .. } => unreachable!("package returned before source checking"),
        Action::Build { output, .. } => {
            native.save(&output).map_err(|error| {
                format!("cannot save executable '{}': {error}", output.display())
            })?;
            if let Some(report) = external_report {
                eprintln!("{}", report.summary());
                libs::write_report(&output, &report)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Action::Run {
            tenant_id,
            arguments,
        } => {
            let arguments = match tenant_id {
                Some(tenant_id) => [
                    vec![
                        OsString::from("--dever-tenant-cmd"),
                        OsString::from(tenant_id.to_string()),
                    ],
                    arguments,
                ]
                .concat(),
                None => arguments,
            };
            run_native(&native, &root, &arguments)
        }
        Action::TenantMigrate { tenant_id } => run_native(
            &native,
            &root,
            &[
                OsString::from("--dever-tenant-migrate"),
                OsString::from(tenant_id.to_string()),
            ],
        ),
        Action::TenantOwner {
            tenant_id,
            site,
            user_id,
        } => run_native(
            &native,
            &root,
            &[
                OsString::from("--dever-tenant-owner"),
                OsString::from(tenant_id.to_string()),
                OsString::from(site),
                OsString::from(user_id.to_string()),
            ],
        ),
        Action::TenantComponent {
            tenant_id,
            component,
            enabled,
        } => run_native(
            &native,
            &root,
            &[
                OsString::from("--dever-tenant-component"),
                OsString::from(tenant_id.to_string()),
                OsString::from(if enabled { "enable" } else { "disable" }),
                OsString::from(component),
            ],
        ),
        Action::Check
        | Action::Api { .. }
        | Action::Format { .. }
        | Action::Test
        | Action::Lib { .. }
        | Action::Clean => {
            unreachable!("check and fmt returned before native compilation")
        }
    }
}

fn run_native(
    native: &dever_core::native::NativeProgram,
    project_root: &PathBuf,
    arguments: &[OsString],
) -> Result<ExitCode, String> {
    stage_runtime_config(native.executable(), project_root)?;
    let status = Command::new(native.executable())
        .current_dir(project_root)
        .args(arguments)
        .status()
        .map_err(|error| format!("cannot run native program: {error}"))?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn stage_runtime_config(
    executable: &std::path::Path,
    project_root: &std::path::Path,
) -> Result<(), String> {
    let source = project_root.join("config");
    let destination = executable
        .parent()
        .ok_or("native executable has no parent directory")?
        .join("config");
    copy_config_directory(&source, &destination)
}

fn copy_config_directory(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| format!("cannot inspect '{}': {error}", source.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "project config '{}' must be a real directory",
            source.display()
        ));
    }
    std::fs::create_dir(destination)
        .map_err(|error| format!("cannot stage config '{}': {error}", destination.display()))?;
    for entry in std::fs::read_dir(source)
        .map_err(|error| format!("cannot read '{}': {error}", source.display()))?
    {
        let entry =
            entry.map_err(|error| format!("cannot read '{}': {error}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&source_path)
            .map_err(|error| format!("cannot inspect '{}': {error}", source_path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "project config '{}' must not contain symbolic links",
                source_path.display()
            ));
        }
        if metadata.is_dir() {
            copy_config_directory(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&source_path, &destination_path).map_err(|error| {
                format!("cannot stage config '{}': {error}", source_path.display())
            })?;
        } else {
            return Err(format!(
                "project config '{}' must contain only files and directories",
                source_path.display()
            ));
        }
    }
    Ok(())
}

fn arguments_for(arguments: &[OsString]) -> Result<(PathBuf, Action), String> {
    match arguments {
        [command, operation, root, rest @ ..]
            if command == "lib"
                && matches!(
                    operation.to_str(),
                    Some("add" | "list" | "update" | "remove" | "doctor")
                ) =>
        {
            let command = operation.to_str().unwrap().to_owned();
            let (specs, target) = lib_arguments(rest)?;
            Ok((
                root.into(),
                Action::Lib {
                    command,
                    specs,
                    target,
                },
            ))
        }
        [command, operation, root, rest @ ..]
            if command == "package"
                && matches!(
                    operation.to_str(),
                    Some("add" | "list" | "update" | "remove" | "doctor")
                ) =>
        {
            let command = operation.to_str().unwrap().to_owned();
            let specs = rest
                .iter()
                .map(|value| {
                    value
                        .to_str()
                        .map(str::to_owned)
                        .ok_or("Package spec must be UTF-8".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((root.into(), Action::Package { command, specs }))
        }
        [command, root] if command == "check" => Ok((root.into(), Action::Check)),
        [command, root] if command == "api" => Ok((root.into(), Action::Api { output: None })),
        [command, root, flag, output] if command == "api" && flag == "--output" => Ok((
            root.into(),
            Action::Api {
                output: Some(output.into()),
            },
        )),
        [command, root] if command == "fmt" => Ok((root.into(), Action::Format { check: false })),
        [command, root, flag] if command == "fmt" && flag == "--check" => {
            Ok((root.into(), Action::Format { check: true }))
        }
        [command, root] if command == "run" => Ok((
            root.into(),
            Action::Run {
                tenant_id: None,
                arguments: Vec::new(),
            },
        )),
        [command, root, separator, name, input] if command == "run" && separator == "--" => Ok((
            root.into(),
            Action::Run {
                tenant_id: None,
                arguments: vec![name.clone(), input.clone()],
            },
        )),
        [command, root, tenant, tenant_id, separator, name, input]
            if command == "run" && tenant == "--tenant" && separator == "--" =>
        {
            let tenant_id = positive_tenant_id(tenant_id)?;
            Ok((
                root.into(),
                Action::Run {
                    tenant_id: Some(tenant_id),
                    arguments: vec![name.clone(), input.clone()],
                },
            ))
        }
        [command, root] if command == "test" => Ok((root.into(), Action::Test)),
        [command, root] if command == "clean" => Ok((root.into(), Action::Clean)),
        [command, root, rest @ ..] if command == "build" => {
            let mut output = None;
            let mut target = None;
            let mut options = rest.iter();
            while let Some(option) = options.next() {
                match option.to_str() {
                    Some("--output") if output.is_none() => {
                        output = Some(PathBuf::from(option_value(&mut options, "--output")?));
                    }
                    Some("--target") if target.is_none() => {
                        target = Some(parse_target(option_value(&mut options, "--target")?)?);
                    }
                    _ => return Err("build accepts --output and --target exactly once; unknown or duplicate option".into()),
                }
            }
            Ok((
                root.into(),
                Action::Build {
                    output: output.ok_or("build requires --output <new-file>")?,
                    target: target.map_or_else(BuildTarget::host, Ok)?,
                },
            ))
        }
        [tenant, migrate, root, tenant_id] if tenant == "tenant" && migrate == "migrate" => {
            let tenant_id = positive_tenant_id(tenant_id)?;
            Ok((root.into(), Action::TenantMigrate { tenant_id }))
        }
        [tenant, owner, root, tenant_id, site, user_id]
            if tenant == "tenant" && owner == "owner" =>
        {
            let tenant_id = positive_id(tenant_id, "tenant id")?;
            let site = site
                .to_str()
                .ok_or_else(|| "site must be UTF-8".to_owned())?
                .to_owned();
            let user_id = positive_id(user_id, "user id")?;
            Ok((
                root.into(),
                Action::TenantOwner {
                    tenant_id,
                    site,
                    user_id,
                },
            ))
        }
        [
            tenant,
            component_command,
            operation,
            root,
            tenant_id,
            component,
        ] if tenant == "tenant"
            && component_command == "component"
            && (operation == "enable" || operation == "disable") =>
        {
            let tenant_id = positive_tenant_id(tenant_id)?;
            let component = component
                .to_str()
                .ok_or_else(|| "component must be UTF-8".to_owned())?
                .to_owned();
            Ok((
                root.into(),
                Action::TenantComponent {
                    tenant_id,
                    component,
                    enabled: operation == "enable",
                },
            ))
        }
        _ => Err(USAGE.into()),
    }
}

fn option_value<'a>(
    options: &mut impl Iterator<Item = &'a OsString>,
    option: &str,
) -> Result<&'a OsString, String> {
    options
        .next()
        .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with("--"))
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_target(value: &OsString) -> Result<BuildTarget, String> {
    value.to_str().ok_or("build target must be UTF-8")?.parse()
}

fn lib_arguments(arguments: &[OsString]) -> Result<(Vec<String>, BuildTarget), String> {
    let mut specs = Vec::new();
    let mut target = None;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--target" && target.is_none() {
            target = Some(parse_target(option_value(&mut arguments, "--target")?)?);
        } else {
            let spec = argument.to_str().ok_or("lib spec must be UTF-8")?;
            if spec.starts_with('-') {
                return Err("unknown or duplicate lib option".into());
            }
            specs.push(spec.to_owned());
        }
    }
    Ok((specs, target.map_or_else(BuildTarget::host, Ok)?))
}

fn positive_tenant_id(value: &OsString) -> Result<i64, String> {
    positive_id(value, "tenant id")
}

fn positive_id(value: &OsString, label: &str) -> Result<i64, String> {
    let id = value
        .to_str()
        .ok_or_else(|| format!("{label} must be a positive integer"))?
        .parse::<i64>()
        .map_err(|_| format!("{label} must be a positive integer"))?;
    if id <= 0 {
        return Err(format!("{label} must be a positive integer"));
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::{Action, arguments_for};
    use std::ffi::OsString;

    #[test]
    fn run_accepts_only_one_named_json_command_after_separator() {
        let args = [
            "run",
            "project",
            "--",
            "user.account.greeting",
            "{\"name\":\"Ada\"}",
        ]
        .map(OsString::from);
        let (root, action) = arguments_for(&args).unwrap();
        assert_eq!(root.to_string_lossy(), "project");
        let Action::Run {
            tenant_id,
            arguments,
        } = action
        else {
            panic!("expected run action")
        };
        assert_eq!(tenant_id, None);
        assert_eq!(arguments, args[3..]);
        assert!(arguments_for(&args[..4]).is_err());
        assert!(arguments_for(&[args.as_slice(), &[OsString::from("extra")]].concat()).is_err());
    }

    #[test]
    fn tenant_command_uses_a_separate_positive_control_plane_id() {
        let args = [
            "run",
            "project",
            "--tenant",
            "42",
            "--",
            "content.article.rebuild",
            "{\"force\":true}",
        ]
        .map(OsString::from);
        let (root, action) = arguments_for(&args).unwrap();
        assert_eq!(root.to_string_lossy(), "project");
        let Action::Run {
            tenant_id,
            arguments,
        } = action
        else {
            panic!("expected run action")
        };
        assert_eq!(tenant_id, Some(42));
        assert_eq!(arguments, args[5..]);

        for invalid in ["0", "-1", "tenant-1", "9223372036854775808"] {
            let args = [
                "run",
                "project",
                "--tenant",
                invalid,
                "--",
                "content.article.rebuild",
                "{}",
            ]
            .map(OsString::from);
            let error = arguments_for(&args)
                .err()
                .expect("invalid tenant id must fail");
            assert_eq!(error, "tenant id must be a positive integer");
        }
    }

    #[test]
    fn tenant_migrate_requires_a_positive_integer_id() {
        let args = ["tenant", "migrate", "project", "42"].map(OsString::from);
        let (root, action) = arguments_for(&args).unwrap();
        assert_eq!(root.to_string_lossy(), "project");
        assert!(matches!(action, Action::TenantMigrate { tenant_id: 42 }));

        for invalid in ["0", "-1", "tenant-1", "9223372036854775808"] {
            let args = ["tenant", "migrate", "project", invalid].map(OsString::from);
            let error = arguments_for(&args)
                .err()
                .expect("invalid tenant id must fail");
            assert_eq!(error, "tenant id must be a positive integer");
        }
    }

    #[test]
    fn tenant_component_command_selects_one_manifest_component() {
        for (operation, expected) in [("enable", true), ("disable", false)] {
            let args =
                ["tenant", "component", operation, "project", "42", "news"].map(OsString::from);
            let (root, action) = arguments_for(&args).unwrap();
            assert_eq!(root.to_string_lossy(), "project");
            let Action::TenantComponent {
                tenant_id,
                component,
                enabled,
            } = action
            else {
                panic!("expected tenant component action");
            };
            assert_eq!(tenant_id, 42);
            assert_eq!(component, "news");
            assert_eq!(enabled, expected);
        }

        let invalid =
            ["tenant", "component", "toggle", "project", "42", "news"].map(OsString::from);
        assert!(arguments_for(&invalid).is_err());
    }

    #[test]
    fn clean_accepts_exactly_one_project_root() {
        let args = ["clean", "project"].map(OsString::from);
        let (root, action) = arguments_for(&args).unwrap();
        assert_eq!(root.to_string_lossy(), "project");
        assert!(matches!(action, Action::Clean));
        assert!(arguments_for(&[args.as_slice(), &[OsString::from("extra")]].concat()).is_err());
    }
}
