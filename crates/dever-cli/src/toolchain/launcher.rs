use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::release::{Layout, MachineManager, Version, platform_contract};

const USAGE: &str = "Usage:\n  dever install <version|latest>\n  dever update\n  dever use <version>\n  dever uninstall <version>\n  dever version [project-root]\n  dever cache status\n  dever cache clean\n  dever lib add|list|update|remove|doctor <project-root> [--target linux-x86_64|linux-aarch64] [spec ...]\n  dever package add|list|update|remove|doctor <project-root> [spec ...]\n  dever check <project-root>\n  dever api <project-root> [--output <new-file>]\n  dever fmt <project-root> [--check]\n  dever test <project-root>\n  dever run <project-root> [-- <component>.<domain>.<cmd> '<json-object>']\n  dever build <project-root> --output <new-file> [--target linux-x86_64|linux-aarch64]\n  dever clean <project-root>\n  dever tenant ...";

#[derive(Debug)]
pub enum CommandResult {
    Exited(i32),
    Message(String),
}

pub fn execute(executable: &Path, arguments: &[OsString]) -> Result<CommandResult, String> {
    if platform_contract(std::env::consts::OS).is_none() {
        return Err(format!(
            "Dever does not support platform '{}'",
            std::env::consts::OS
        ));
    }
    let layout = Layout::from_launcher(executable)?;
    execute_in(&layout, arguments)
}

pub fn execute_in(layout: &Layout, arguments: &[OsString]) -> Result<CommandResult, String> {
    layout.validate_machine_permissions()?;
    #[cfg(target_os = "linux")]
    super::bootstrap::require_no_pending(layout)?;
    let manager = MachineManager::new(layout.clone());
    match arguments {
        [command, requested] if command == "install" => {
            let requested = utf8(requested, "version")?;
            super::release_source::prepare(layout, requested)?;
            let version = manager.install_requested(requested)?;
            Ok(CommandResult::Message(format!("installed Dever {version}")))
        }
        [command] if command == "update" => {
            super::release_source::prepare(layout, "latest")?;
            let version = manager.update()?;
            Ok(CommandResult::Message(format!(
                "updated Dever and dever-language skill to {version}"
            )))
        }
        [command, version] if command == "use" => {
            let version = Version::parse(utf8(version, "version")?)?;
            manager.activate(&version)?;
            Ok(CommandResult::Message(format!("active Dever {version}")))
        }
        [command, version] if command == "uninstall" => {
            let version = Version::parse(utf8(version, "version")?)?;
            manager.uninstall(&version)?;
            Ok(CommandResult::Message(format!(
                "uninstalled Dever {version}"
            )))
        }
        [command] if command == "version" => {
            let version = manager
                .active_version()?
                .ok_or("no active Dever version; run 'dever install latest'")?;
            manager.resolve_core(&version)?;
            Ok(CommandResult::Message(version.to_string()))
        }
        [command, project] if command == "version" => {
            let root = PathBuf::from(project);
            let version = select_version(&manager, &root)?;
            Ok(CommandResult::Message(version.to_string()))
        }
        [cache, status] if cache == "cache" && status == "status" => {
            let status = crate::toolchain::cache_status(layout)?;
            Ok(CommandResult::Message(
                serde_json::to_string(&status).map_err(|error| error.to_string())?,
            ))
        }
        [cache, clean] if cache == "cache" && clean == "clean" => {
            let status = crate::toolchain::cache_clean(
                layout,
                &manager.installed_versions()?.into_iter().collect(),
            )?;
            Ok(CommandResult::Message(
                serde_json::to_string(&status).map_err(|error| error.to_string())?,
            ))
        }
        [skill, path] if skill == "skill" && path == "path" => Ok(CommandResult::Message(
            super::skill::path(&manager)?.display().to_string(),
        )),
        [skill, path, project] if skill == "skill" && path == "path" => {
            let version = select_version(&manager, Path::new(project))?;
            Ok(CommandResult::Message(
                manager.resolve_skill(&version)?.display().to_string(),
            ))
        }
        [skill, install, directory] if skill == "skill" && install == "install" => {
            super::skill::install(&manager, Path::new(directory))?;
            Ok(CommandResult::Message(format!(
                "installed dever-language skill entry in '{}'",
                Path::new(directory).display()
            )))
        }
        [command, ..] if command == "new" => {
            let version = manager
                .active_version()?
                .ok_or("no active Dever version; run 'dever install latest'")?;
            dispatch_core(&manager, &version, arguments)
        }
        [help] if help == "--help" => Ok(CommandResult::Message(format!(
            "{USAGE}\n  dever new <new-directory> [--markdown]\n  dever skill path [project-root]\n  dever skill install <new-directory>"
        ))),
        [] => Err(USAGE.into()),
        _ => dispatch_project_command(&manager, arguments),
    }
}

pub fn project_version(project_root: &Path) -> Result<Option<Version>, String> {
    let path = project_root.join("config/setting.json");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read '{}': {error}", path.display()))?;
    let root = dever_runtime::wire::parse(&text)
        .map_err(|error| format!("invalid '{}': {error}", path.display()))?;
    let fields = root
        .object()
        .map_err(|_| format!("invalid '{}': expected a JSON object", path.display()))?;
    let Some(dever) = fields.get("dever") else {
        return Ok(None);
    };
    let fields = dever
        .fields(&["version"])
        .map_err(|error| format!("invalid '{}': dever: {error}", path.display()))?;
    let version = fields
        .get("version")
        .ok_or_else(|| format!("invalid '{}': dever.version is required", path.display()))?
        .text()
        .map_err(|error| format!("invalid '{}': dever.version: {error}", path.display()))?;
    Version::parse(version)
        .map(Some)
        .map_err(|error| format!("invalid '{}': {error}", path.display()))
}

fn dispatch_project_command(
    manager: &MachineManager,
    arguments: &[OsString],
) -> Result<CommandResult, String> {
    let root = project_root(arguments).ok_or_else(|| USAGE.to_owned())?;
    let version = select_version(manager, &root)?;
    dispatch_core(manager, &version, arguments)
}

fn dispatch_core(
    manager: &MachineManager,
    version: &Version,
    arguments: &[OsString],
) -> Result<CommandResult, String> {
    let core = manager.resolve_core(version)?;
    let status = Command::new(&core)
        .args(arguments)
        .env_clear()
        .status()
        .map_err(|error| format!("cannot run Dever {version} '{}': {error}", core.display()))?;
    Ok(CommandResult::Exited(status.code().unwrap_or(1)))
}

fn select_version(manager: &MachineManager, project_root: &Path) -> Result<Version, String> {
    let version = match project_version(project_root)? {
        Some(version) => version,
        None => manager
            .active_version()?
            .ok_or("no active Dever version; run 'dever install latest'")?,
    };
    manager.resolve_core(&version)?;
    Ok(version)
}

fn project_root(arguments: &[OsString]) -> Option<PathBuf> {
    match arguments {
        [command, _operation, root, ..] if command == "lib" || command == "package" => {
            Some(root.into())
        }
        [command, root, ..]
            if matches!(
                command.to_str(),
                Some("check" | "api" | "fmt" | "test" | "run" | "build" | "clean")
            ) =>
        {
            Some(root.into())
        }
        [tenant, operation, root, ..]
            if tenant == "tenant" && matches!(operation.to_str(), Some("migrate" | "owner")) =>
        {
            Some(root.into())
        }
        [tenant, component, _operation, root, ..]
            if tenant == "tenant" && component == "component" =>
        {
            Some(root.into())
        }
        _ => None,
    }
}

fn utf8<'a>(value: &'a OsStr, label: &str) -> Result<&'a str, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))
}
