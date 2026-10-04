//! Shared ELF architecture, loader search and versioned dependency closure.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use goblin::elf::{Elf, header};

pub(crate) fn object<'a>(path: &str, bytes: &'a [u8], machine: u16) -> Result<Elf<'a>, String> {
    let elf = Elf::parse(bytes).map_err(|error| format!("invalid native ELF '{path}': {error}"))?;
    if !elf.is_64
        || !elf.little_endian
        || elf.header.e_machine != machine
        || !matches!(elf.header.e_type, header::ET_DYN | header::ET_EXEC)
    {
        return Err(format!(
            "native ELF '{path}' has an incompatible target or object type"
        ));
    }
    Ok(elf)
}

pub(crate) fn origin_directories(path: &str, fields: &[&str]) -> Result<Vec<String>, String> {
    let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
    let mut result = Vec::new();
    for field in fields {
        for directory in field.split(':') {
            let relative = directory
                .strip_prefix("$ORIGIN")
                .or_else(|| directory.strip_prefix("${ORIGIN}"))
                .ok_or("native library search path must be relative to $ORIGIN")?;
            if !relative.is_empty() && !relative.starts_with('/')
                || relative.contains(['$', '\\', '\0'])
            {
                return Err("native library search path has an unsupported token".into());
            }
            let combined = format!("{parent}{relative}");
            let mut normalized = Vec::new();
            for part in combined.split('/') {
                match part {
                    "" | "." => {}
                    ".." => {
                        normalized
                            .pop()
                            .ok_or("native library path escapes the installation tree")?;
                    }
                    part => normalized.push(part),
                }
            }
            let normalized = normalized.join("/");
            if normalized.is_empty() || normalized.starts_with("sandbox/") {
                return Err("native library path escapes its Worker installation".into());
            }
            if !result.contains(&normalized) {
                result.push(normalized);
            }
        }
    }
    Ok(result)
}

fn plain_library(name: &str) -> Result<(), String> {
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err("native DT_NEEDED must be a plain library name".into());
    }
    Ok(())
}

fn validate_versions(consumer: &Elf<'_>, library: &str, provider: &Elf<'_>) -> Result<(), String> {
    let mut provided = BTreeSet::new();
    if let Some(definitions) = &provider.verdef {
        for definition in definitions.iter() {
            for name in definition.iter() {
                provided.insert(
                    provider
                        .dynstrtab
                        .get_at(name.vda_name)
                        .ok_or("invalid ELF version definition")?
                        .to_owned(),
                );
            }
        }
    }
    if let Some(requirements) = &consumer.verneed {
        for requirement in requirements.iter() {
            if consumer.dynstrtab.get_at(requirement.vn_file) != Some(library) {
                continue;
            }
            for version in requirement.iter() {
                let name = consumer
                    .dynstrtab
                    .get_at(version.vna_name)
                    .ok_or("invalid ELF version requirement")?;
                if version.vna_flags & 2 == 0 && !provided.contains(name) {
                    return Err(format!(
                        "packaged library '{library}' does not provide native ABI version '{name}'"
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(crate) struct Closure<'a> {
    pub(crate) files: &'a BTreeMap<String, &'a [u8]>,
    pub(crate) machine: u16,
    pub(crate) directories: Vec<String>,
}

impl Closure<'_> {
    pub(crate) fn follow(
        &self,
        start: &str,
        inherited: Vec<String>,
        globals: &BTreeMap<String, String>,
    ) -> Result<(BTreeMap<String, String>, BTreeSet<String>), String> {
        let mut loaded = globals.clone();
        let mut pending = VecDeque::from([(start.to_owned(), inherited)]);
        let mut seen = BTreeSet::new();
        while let Some((path, inherited)) = pending.pop_front() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let bytes = self
                .files
                .get(&path)
                .ok_or_else(|| format!("native dependency '{path}' is absent"))?;
            let elf = object(&path, bytes, self.machine)?;
            let rpaths = if elf.runpaths.is_empty() {
                origin_directories(&path, &elf.rpaths)?
            } else {
                Vec::new()
            };
            let runpaths = origin_directories(&path, &elf.runpaths)?;
            let mut inherited_next = rpaths.clone();
            inherited_next.extend(inherited.clone());
            let inherited_search = if elf.runpaths.is_empty() {
                inherited.as_slice()
            } else {
                &[]
            };
            for name in &elf.libraries {
                plain_library(name)?;
                let supplied = loaded
                    .get(*name)
                    .cloned()
                    .or_else(|| {
                        rpaths
                            .iter()
                            .chain(inherited_search.iter())
                            .chain(runpaths.iter())
                            .map(|directory| format!("{directory}/{name}"))
                            .chain(
                                self.directories
                                    .iter()
                                    .map(|directory| format!("{directory}/{name}")),
                            )
                            .find(|candidate| self.files.contains_key(candidate))
                    })
                    .ok_or_else(|| {
                        format!("native library '{name}' is not in the packaged dependency closure")
                    })?;
                let provider = object(&supplied, self.files[&supplied], self.machine)?;
                validate_versions(&elf, name, &provider)?;
                loaded.insert((*name).to_owned(), supplied.clone());
                if let Some(soname) = provider.soname {
                    plain_library(soname)?;
                    if soname != *name {
                        return Err("native library SONAME differs from DT_NEEDED".into());
                    }
                }
                pending.push_back((supplied, inherited_next.clone()));
            }
        }
        Ok((loaded, seen))
    }
}
