//! One bounded journal for the fixed bootstrap destinations. It contains
//! identities, never caller-selected recovery paths or arbitrary commands.
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    Layout, ReleaseManifest, create_directory, digest, read_bounded, release, service,
    trusted_directory, trusted_file, validate_bin, write_new,
};

const JOURNAL: &str = "bootstrap-install.json";
const JOURNAL_NEXT: &str = ".bootstrap-install.next";
const MAX_TREE_BYTES: u64 = 80 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Target {
    Bin,
    Sandbox,
    Apparmor,
    Service,
    Entry,
    Enable,
    Active,
}

const TARGETS: [Target; 7] = [
    Target::Sandbox,
    Target::Apparmor,
    Target::Bin,
    Target::Service,
    Target::Entry,
    Target::Enable,
    Target::Active,
];

impl Target {
    fn relative(self) -> &'static str {
        match self {
            Self::Bin => "opt/dever/bin",
            Self::Sandbox => "opt/dever/sandbox",
            Self::Apparmor => "etc/apparmor.d/dever-sandbox",
            Self::Service => "etc/systemd/system/deverd.service",
            Self::Entry => "usr/local/bin/dever",
            Self::Enable => "etc/systemd/system/multi-user.target.wants/deverd.service",
            Self::Active => "opt/dever/state/active-version",
        }
    }
    fn path(self, root: &Path) -> PathBuf {
        root.join(self.relative())
    }
    fn next(self, root: &Path) -> PathBuf {
        let path = self.path(root);
        path.with_file_name(format!(
            ".dever-bootstrap-{}.next",
            path.file_name().unwrap().to_string_lossy()
        ))
    }
    fn link(self) -> Option<&'static str> {
        match self {
            Self::Entry => Some("/opt/dever/bin/dever"),
            Self::Enable => Some("../deverd.service"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Preparing,
    Prepared,
    Committed,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Change {
    target: Target,
    before: Option<String>,
    after: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    format: String,
    phase: Phase,
    service_was_active: Option<bool>,
    service_start_requested: bool,
    changes: Vec<Change>,
}

pub(super) fn install(
    root: &Path,
    layout: &Layout,
    source: &Path,
    manifest: &ReleaseManifest,
    key: &[u8],
    activate_service: bool,
) -> Result<(), String> {
    let previous_service = if fs::read_dir(layout.bin())
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        Some(validate_bin(&layout.bin(), key)?)
    } else {
        None
    };
    for target in TARGETS {
        let parent = Path::new(target.relative())
            .parent()
            .unwrap()
            .to_str()
            .unwrap();
        create_directory(root, parent)?;
        if fs::symlink_metadata(target.next(root)).is_ok() {
            return Err("unrecorded bootstrap staging destination already exists".into());
        }
        if let Some(link) = target.link() {
            if let Ok(metadata) = fs::symlink_metadata(target.path(root))
                && (previous_service.is_none()
                    || metadata.uid() != 0
                    || !metadata.file_type().is_symlink()
                    || fs::read_link(target.path(root)).map_err(|error| error.to_string())?
                        != Path::new(link))
            {
                return Err(
                    "bootstrap cannot replace an unrelated public entry or service link".into(),
                );
            }
        } else if target == Target::Apparmor && target.path(root).exists() {
            trusted_file(&target.path(root))?;
            let installed_profile = layout.bin().join("dever-sandbox");
            if previous_service.is_none()
                || !installed_profile.exists()
                || read_bounded(&target.path(root), 8192)?
                    != read_bounded(&installed_profile, 8192)?
            {
                return Err("bootstrap cannot replace an unrelated AppArmor profile".into());
            }
        } else if target == Target::Service && target.path(root).exists() {
            trusted_file(&target.path(root))?;
            if previous_service.as_deref()
                != Some(read_bounded(&target.path(root), 8192)?.as_slice())
            {
                return Err(
                    "bootstrap cannot replace an unrelated system service definition".into(),
                );
            }
        }
    }
    let changes = TARGETS
        .into_iter()
        .map(|target| {
            Ok(Change {
                target,
                before: identity(&target.path(root), target.link())?,
                after: None,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut journal = Journal {
        format: "dever-bootstrap-transaction-v1".into(),
        phase: Phase::Preparing,
        service_was_active: if activate_service {
            Some(service::active()?)
        } else {
            None
        },
        service_start_requested: false,
        changes,
    };
    save(layout, &journal)?;
    let result = (|| {
        stage(root, source, manifest, key)?;
        for change in &mut journal.changes {
            change.after = identity(&change.target.next(root), change.target.link())?;
            if change.after.is_none() {
                return Err("bootstrap staging is incomplete".into());
            }
        }
        journal.phase = Phase::Prepared;
        save(layout, &journal)?;
        for index in 0..journal.changes.len() {
            if journal.changes[index].target == Target::Active && activate_service {
                journal.service_start_requested = true;
                save(layout, &journal)?;
                service::apply(true)?;
                service::ready(layout)?;
            }
            publish(root, &journal.changes[index])?;
        }
        journal.phase = Phase::Committed;
        save(layout, &journal)
    })();
    if let Err(primary) = result {
        // The journal is still Prepared unless its commit was persisted. Read
        // the durable phase rather than trusting the in-memory assignment.
        return match recover(root, layout) {
            Ok(()) => Err(primary),
            Err(rollback) => Err(format!("{primary}; bootstrap recovery failed: {rollback}")),
        };
    }
    if let Err(error) = recover(root, layout) {
        // Publication is already committed. Never roll it back because an old
        // staging copy could not be removed; preserve the journal for retry.
        return Err(format!(
            "bootstrap publication committed, but cleanup requires recovery: {error}"
        ));
    }
    Ok(())
}

fn stage(root: &Path, source: &Path, manifest: &ReleaseManifest, key: &[u8]) -> Result<(), String> {
    super::sandbox::stage(
        &Target::Sandbox.path(root),
        &Target::Sandbox.next(root),
        source,
        manifest,
        key,
    )?;
    write_new(
        &Target::Apparmor.next(root),
        dever_sandbox::APPARMOR_PROFILE.as_bytes(),
        0o644,
    )?;
    let bin = Target::Bin.next(root);
    fs::create_dir(&bin).map_err(|error| error.to_string())?;
    for artifact in &manifest.artifacts {
        let Some(relative) = artifact.path.strip_prefix("bootstrap/") else {
            continue;
        };
        release::copy_file(
            &source.join(&artifact.path),
            &bin.join(relative),
            matches!(relative, "dever" | "deverd"),
        )?;
    }
    for file in ["manifest.json", "manifest.sig"] {
        release::copy_file(&source.join(file), &bin.join(file), false)?;
    }
    validate_bin(&bin, key)?;
    for name in ["dever", "deverd"] {
        if !service::run(
            Command::new(bin.join(name)).arg("--dever-bootstrap-health"),
            "health-check bootstrap",
        )? {
            return Err(format!("bootstrap {name} failed its health check"));
        }
    }
    write_new(
        &Target::Service.next(root),
        super::SERVICE.as_bytes(),
        0o644,
    )?;
    for target in [Target::Entry, Target::Enable] {
        symlink(target.link().unwrap(), target.next(root)).map_err(|error| error.to_string())?;
        sync_parent(&target.next(root))?;
    }
    let active = Target::Active.path(root);
    let next = Target::Active.next(root);
    if active.exists() {
        release::copy_file(&active, &next, false)?;
    }
    release::append_active_version(&next, &manifest.version).map_err(|error| error.to_string())?;
    release::sync_tree(&bin)?;
    sync_parent(&bin)
}

fn publish(root: &Path, change: &Change) -> Result<(), String> {
    if change.target == Target::Apparmor && root == Path::new("/") {
        dever_sandbox::validate_host_policy()?;
    }
    let path = change.target.path(root);
    let next = change.target.next(root);
    if identity(&path, change.target.link())? != change.before
        || identity(&next, change.target.link())? != change.after
    {
        return Err("bootstrap destination changed during installation".into());
    }
    if change.before == change.after {
        return Ok(());
    }
    if change.before.is_some() {
        exchange(&path, &next)?;
    } else {
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            &next,
            rustix::fs::CWD,
            &path,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|error| error.to_string())?;
    }
    sync_parent(&path)
}

pub(super) fn recover(root: &Path, layout: &Layout) -> Result<(), String> {
    let temporary = layout.state().join(JOURNAL_NEXT);
    if fs::symlink_metadata(&temporary).is_ok() {
        trusted_file(&temporary)?;
        fs::remove_file(&temporary).map_err(|error| error.to_string())?;
    }
    let path = layout.state().join(JOURNAL);
    let bytes = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
        Ok(_) => {
            trusted_file(&path)?;
            read_bounded(&path, 64 * 1024)?
        }
    };
    let journal: Journal = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid bootstrap recovery journal: {error}"))?;
    if journal.format != "dever-bootstrap-transaction-v1"
        || journal
            .changes
            .iter()
            .map(|change| change.target)
            .collect::<Vec<_>>()
            != TARGETS
        || journal.service_was_active.is_some() && root != Path::new("/")
        || journal.service_start_requested
            && (journal.service_was_active.is_none() || journal.phase == Phase::Preparing)
        || journal.changes.iter().any(|change| {
            change.before.iter().chain(&change.after).any(|hash| {
                hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        })
        || journal.phase != Phase::Preparing
            && journal.changes.iter().any(|change| change.after.is_none())
    {
        return Err("bootstrap recovery journal has unsupported targets or identities".into());
    }
    // Validate all affected parents before touching any target. The journal can
    // never turn an image-root path into a write through a substituted symlink.
    for change in &journal.changes {
        trusted_directory(change.target.path(root).parent().unwrap())?;
    }
    if journal.phase == Phase::Committed {
        for change in &journal.changes {
            if identity(&change.target.path(root), change.target.link())? != change.after {
                return Err("committed bootstrap destination changed before cleanup".into());
            }
        }
    } else {
        let (before_files, after_files) = if let Some(was_active) = journal.service_was_active {
            service::recovery_actions(
                journal.service_start_requested,
                was_active,
                journal.service_start_requested && service::loaded()?,
            )
        } else {
            (vec![], vec![])
        };
        service::execute(&before_files)?;
        for change in journal.changes.iter().rev() {
            rollback(root, change, journal.phase)?;
        }
        service::execute(&after_files)?;
    }
    for change in &journal.changes {
        remove_owned(&change.target.next(root), change.target.link())?;
    }
    fs::remove_file(&path).map_err(|error| error.to_string())?;
    sync_parent(&path)
}

fn rollback(root: &Path, change: &Change, phase: Phase) -> Result<(), String> {
    let path = change.target.path(root);
    let next = change.target.next(root);
    let current = identity(&path, change.target.link())?;
    if current == change.before {
        return Ok(());
    }
    if phase != Phase::Prepared || current != change.after {
        return Err("bootstrap recovery refuses a destination outside this transaction".into());
    }
    if let Some(before) = &change.before {
        if identity(&next, change.target.link())?.as_ref() != Some(before) {
            return Err("bootstrap rollback copy is missing or changed".into());
        }
        exchange(&path, &next)?;
    } else {
        remove_owned(&path, change.target.link())?;
    }
    sync_parent(&path)
}

fn save(layout: &Layout, journal: &Journal) -> Result<(), String> {
    let next = layout.state().join(JOURNAL_NEXT);
    write_new(
        &next,
        &serde_json::to_vec(journal).map_err(|error| error.to_string())?,
        0o600,
    )?;
    fs::rename(&next, layout.state().join(JOURNAL)).map_err(|error| error.to_string())?;
    release::sync_directory(&layout.state()).map_err(|error| error.to_string())
}

fn exchange(path: &Path, next: &Path) -> Result<(), String> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        path,
        rustix::fs::CWD,
        next,
        rustix::fs::RenameFlags::EXCHANGE,
    )
    .map_err(|error| format!("atomic bootstrap exchange failed: {error}"))
}

fn sync_parent(path: &Path) -> Result<(), String> {
    release::sync_directory(path.parent().ok_or("bootstrap destination has no parent")?)
        .map_err(|error| error.to_string())
}

fn remove_owned(path: &Path, link: Option<&str>) -> Result<(), String> {
    let Some(_) = identity(path, link)? else {
        return Ok(());
    };
    if fs::symlink_metadata(path)
        .map_err(|error| error.to_string())?
        .is_dir()
    {
        fs::remove_dir_all(path).map_err(|error| error.to_string())?;
    } else {
        fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    sync_parent(path)
}

fn identity(path: &Path, link: Option<&str>) -> Result<Option<String>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if let Some(link) = link {
        if !metadata.file_type().is_symlink()
            || metadata.uid() != 0
            || fs::read_link(path).map_err(|error| error.to_string())? != Path::new(link)
        {
            return Err("bootstrap link has an unsupported owner or target".into());
        }
        return Ok(Some(digest(format!("link\0{link}").as_bytes())));
    }
    let mut tree = Tree::default();
    tree.visit(path, path, 0)?;
    Ok(Some(format!("{:x}", tree.hash.finalize())))
}

pub(super) fn files(root: &Path) -> Result<Vec<String>, String> {
    let mut tree = Tree::default();
    tree.visit(root, root, 0)?;
    Ok(tree.files.into_iter().collect())
}

#[derive(Default)]
struct Tree {
    hash: Sha256,
    bytes: u64,
    entries: usize,
    files: BTreeSet<String>,
}

impl Tree {
    fn visit(&mut self, root: &Path, path: &Path, depth: usize) -> Result<(), String> {
        self.entries += 1;
        if self.entries > 128 || depth > 4 {
            return Err("bootstrap tree exceeds its entry/depth bound".into());
        }
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        let name = path
            .strip_prefix(root)
            .unwrap()
            .to_str()
            .ok_or("bootstrap path is not UTF-8")?;
        self.hash.update((name.len() as u64).to_le_bytes());
        self.hash.update(name.as_bytes());
        self.hash.update((metadata.mode() & 0o777).to_le_bytes());
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            trusted_directory(path)?;
            self.hash.update(b"directory");
            let mut entries = fs::read_dir(path)
                .map_err(|error| error.to_string())?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            entries.sort();
            for entry in entries {
                self.visit(root, &entry, depth + 1)?;
            }
        } else {
            trusted_file(path)?;
            self.bytes = self
                .bytes
                .checked_add(metadata.len())
                .ok_or("bootstrap tree size overflow")?;
            if self.bytes > MAX_TREE_BYTES {
                return Err("bootstrap tree exceeds its byte bound".into());
            }
            let bytes = read_bounded(path, metadata.len())?;
            self.hash.update(b"file");
            self.hash.update((bytes.len() as u64).to_le_bytes());
            self.hash.update(bytes);
            self.files.insert(name.into());
        }
        Ok(())
    }
}
