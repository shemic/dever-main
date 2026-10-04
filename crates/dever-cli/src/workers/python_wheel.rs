//! One wheel admission and installation contract for registry downloads,
//! offline Workers and isolated build dependencies.

use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use pep440_rs::Version;
use pep508_rs::Requirement;
use sha2::{Digest, Sha256, Sha384, Sha512};

use crate::libs::{LibSpec, archive_files};

pub(crate) mod native;
mod scripts;
pub use native::validate_native;

pub struct Wheel {
    files: Vec<(String, Vec<u8>)>,
    stem: String,
    pub requirements: Vec<String>,
    pub extras: Vec<String>,
    pub requires_python: Option<String>,
}

pub struct InstalledFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}

/// Filename tags can be compressed; runtime and WHEEL tags must be expanded.
pub fn validate_tags(tags: &[String]) -> Result<(), String> {
    let mut unique = BTreeSet::new();
    if tags.is_empty() || tags.len() > 4096 {
        return Err("Python wheel compatibility tags are missing or exceed their bound".into());
    }
    for tag in tags {
        if tag.split('-').count() != 3
            || tag.split('-').any(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            || !unique.insert(tag)
        {
            return Err("Python wheel compatibility tags are invalid or repeated".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_target(
    target: &str,
    tags: &[String],
    markers: &pep508_rs::MarkerEnvironment,
) -> Result<(), String> {
    validate_tags(tags)?;
    let architecture = match target.parse::<crate::toolchain::BuildTarget>()? {
        crate::toolchain::BuildTarget::LinuxX86_64 => "x86_64",
        crate::toolchain::BuildTarget::LinuxAarch64 => "aarch64",
    };
    if markers.sys_platform() != "linux" || markers.platform_machine() != architecture {
        return Err("Python marker environment differs from the selected target".into());
    }
    for tag in tags {
        let platform = tag.rsplit('-').next().expect("validated wheel tag");
        if platform != "any"
            && (!(platform.starts_with("linux_")
                || platform.starts_with("manylinux")
                || platform.starts_with("musllinux"))
                || !platform.ends_with(&format!("_{architecture}")))
        {
            return Err("Python wheel platform tag differs from the selected target".into());
        }
    }
    Ok(())
}

fn expanded_tags(python: &str, abi: &str, platform: &str) -> Result<BTreeSet<String>, String> {
    let mut tags = BTreeSet::new();
    for python in python.split('.') {
        for abi in abi.split('.') {
            for platform in platform.split('.') {
                if tags.len() >= 4096 || !tags.insert(format!("{python}-{abi}-{platform}")) {
                    return Err("wheel filename tags are repeated or exceed their bound".into());
                }
            }
        }
    }
    validate_tags(&tags.iter().cloned().collect::<Vec<_>>())?;
    Ok(tags)
}

fn filename(
    filename: &str,
    spec: Option<&LibSpec>,
) -> Result<(BTreeSet<String>, Option<String>), String> {
    super::valid_path(filename)?;
    if filename.contains('/') {
        return Err("wheel filename must be a leaf name".into());
    }
    let fields = filename
        .strip_suffix(".whl")
        .ok_or("wheel filename lacks .whl")?
        .split('-')
        .collect::<Vec<_>>();
    if !matches!(fields.len(), 5 | 6) {
        return Err("invalid wheel filename fields".into());
    }
    if let Some(spec) = spec {
        validate_identity(fields[0], fields[1], spec)?;
    }
    let build = if fields.len() == 6 {
        validate_build(fields[2])?;
        Some(fields[2].to_owned())
    } else {
        None
    };
    let tail = &fields[fields.len() - 3..];
    Ok((expanded_tags(tail[0], tail[1], tail[2])?, build))
}

pub fn matches(filename_value: &str, selected_tag: &str) -> bool {
    filename(filename_value, None).is_ok_and(|(tags, _)| tags.contains(selected_tag))
}

fn validate_build(value: &str) -> Result<(), String> {
    if !value.as_bytes().first().is_some_and(u8::is_ascii_digit)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("invalid wheel build tag".into());
    }
    Ok(())
}

fn validate_identity(name: &str, version: &str, spec: &LibSpec) -> Result<(), String> {
    name.parse::<pep508_rs::PackageName>()
        .map_err(|error| format!("invalid wheel distribution name: {error}"))?;
    let identity: LibSpec = format!("pip:{name}@{version}").parse()?;
    if identity.distribution_name() != spec.distribution_name()
        || version
            .parse::<Version>()
            .map_err(|error| error.to_string())?
            != spec
                .version
                .parse::<Version>()
                .map_err(|error| error.to_string())?
    {
        return Err("wheel distribution identity differs from its lock".into());
    }
    Ok(())
}

fn headers(bytes: &[u8]) -> Result<BTreeMap<String, Vec<String>>, String> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("wheel metadata exceeds 16 MiB".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "wheel metadata is not UTF-8")?;
    let mut result = BTreeMap::<String, Vec<String>>::new();
    let mut previous: Option<String> = None;
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        if line.starts_with([' ', '\t']) {
            let key = previous
                .as_ref()
                .ok_or("wheel metadata starts with a folded line")?;
            let value = result.get_mut(key).unwrap().last_mut().unwrap();
            value.push(' ');
            value.push_str(line.trim());
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or("invalid wheel metadata header")?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || value.contains('\0')
        {
            return Err("invalid wheel metadata header".into());
        }
        let key = key.to_ascii_lowercase();
        result
            .entry(key.clone())
            .or_default()
            .push(value.trim().into());
        previous = Some(key);
    }
    Ok(result)
}

fn optional_header<'a>(
    headers: &'a BTreeMap<String, Vec<String>>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match headers.get(key) {
        None => Ok(None),
        Some(values) if values.len() == 1 => Ok(Some(values[0].as_str())),
        _ => Err(format!("wheel metadata repeats {key}")),
    }
}

fn required_header<'a>(
    headers: &'a BTreeMap<String, Vec<String>>,
    key: &str,
) -> Result<&'a str, String> {
    optional_header(headers, key)?.ok_or_else(|| format!("wheel metadata lacks {key}"))
}

impl Wheel {
    pub(crate) fn validate_dependencies(
        &self,
        lib: &crate::libs::LockedLib,
        markers: &pep508_rs::MarkerEnvironment,
    ) -> Result<(), String> {
        let extras = lib
            .spec
            .name
            .split_once('[')
            .map(|(_, extras)| extras.trim_end_matches(']'))
            .unwrap_or("")
            .split(',')
            .filter(|extra| !extra.is_empty())
            .map(str::parse::<pep508_rs::ExtraName>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let supplied_extras = self
            .extras
            .iter()
            .map(|extra| extra.parse::<pep508_rs::ExtraName>())
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|error| error.to_string())?;
        if extras.iter().any(|extra| !supplied_extras.contains(extra)) {
            return Err("wheel does not provide a requested extra".into());
        }
        let document = serde_json::json!({"info":{"requires_dist":self.requirements}});
        let expected = crate::libs::python_dependencies(&document, markers, &extras)?;
        let mut names = BTreeSet::new();
        for (name, rule) in expected {
            if name == lib.spec.name
                && rule.contains(
                    &lib.spec
                        .version
                        .parse::<Version>()
                        .map_err(|error| error.to_string())?,
                )
            {
                continue;
            }
            let dependency = lib
                .dependencies
                .iter()
                .find(|dependency| dependency.spec.name == name)
                .ok_or("wheel dependency is missing from the exact lock")?;
            if !rule.contains(
                &dependency
                    .spec
                    .version
                    .parse::<Version>()
                    .map_err(|error| error.to_string())?,
            ) {
                return Err("locked wheel dependency does not satisfy METADATA".into());
            }
            names.insert(name);
        }
        if lib
            .dependencies
            .iter()
            .any(|dependency| !names.contains(&dependency.spec.name))
        {
            return Err("wheel lock has an undeclared dependency".into());
        }
        if let Some(rule) = &self.requires_python
            && !rule
                .parse::<pep440_rs::VersionSpecifiers>()
                .map_err(|error| error.to_string())?
                .contains(markers.python_full_version())
        {
            return Err("wheel Requires-Python excludes the signed runtime".into());
        }
        Ok(())
    }
    pub fn parse(
        spec: &LibSpec,
        filename_value: Option<&str>,
        runtime_tags: &[String],
        bytes: &[u8],
    ) -> Result<Self, String> {
        validate_tags(runtime_tags)?;
        let files = archive_files("whl", bytes)?;
        let mut info = BTreeSet::new();
        let mut data = BTreeSet::new();
        for (path, _) in &files {
            super::valid_path(path)?;
            if let Some((root, _)) = path.split_once('/') {
                if root.ends_with(".dist-info") {
                    info.insert(root);
                }
                if root.ends_with(".data") {
                    data.insert(root);
                }
            }
        }
        if info.len() != 1 {
            return Err("wheel must contain exactly one dist-info directory".into());
        }
        let info = info.into_iter().next().unwrap();
        let stem = info.strip_suffix(".dist-info").unwrap();
        let (name, version) = stem
            .rsplit_once('-')
            .ok_or("invalid wheel dist-info directory")?;
        validate_identity(name, version, spec)?;
        if data.iter().any(|root| *root != format!("{stem}.data")) {
            return Err("wheel data directory differs from its distribution identity".into());
        }
        let read = |name: &str| {
            files
                .iter()
                .find(|(path, _)| path == &format!("{info}/{name}"))
                .map(|(_, bytes)| bytes.as_slice())
                .ok_or_else(|| format!("wheel lacks {name}"))
        };
        let metadata = headers(read("METADATA")?)?;
        let metadata_version = required_header(&metadata, "metadata-version")?;
        if metadata_version
            .split_once('.')
            .is_none_or(|(major, minor)| {
                !matches!(major, "1" | "2") || minor.parse::<u32>().is_err()
            })
        {
            return Err("unsupported wheel core metadata version".into());
        }
        validate_identity(
            required_header(&metadata, "name")?,
            required_header(&metadata, "version")?,
            spec,
        )?;
        let wheel = headers(read("WHEEL")?)?;
        let wheel_version = required_header(&wheel, "wheel-version")?;
        if wheel_version
            .split_once('.')
            .is_none_or(|(major, minor)| major != "1" || minor.parse::<u32>().is_err())
        {
            return Err("unsupported Wheel-Version".into());
        }
        if !matches!(
            required_header(&wheel, "root-is-purelib")?,
            "true" | "false"
        ) {
            return Err("invalid wheel Root-Is-Purelib".into());
        }
        let tags = wheel.get("tag").ok_or("wheel has no compatibility Tag")?;
        validate_tags(tags)?;
        if !tags.iter().any(|tag| runtime_tags.contains(tag)) {
            return Err("wheel tags do not match the signed Python runtime".into());
        }
        if tags.iter().all(|tag| tag.ends_with("-any"))
            && files.iter().any(|(path, bytes)| {
                bytes.starts_with(b"\x7fELF")
                    || path.ends_with(".pyd")
                    || path.ends_with(".dll")
                    || path.ends_with(".dylib")
            })
        {
            return Err("platform-independent wheel contains a native binary".into());
        }
        let build = optional_header(&wheel, "build")?;
        if let Some(build) = build {
            validate_build(build)?;
        }
        if let Some(value) = filename_value {
            let (file_tags, file_build) = filename(value, Some(spec))?;
            if file_tags != tags.iter().cloned().collect() || file_build.as_deref() != build {
                return Err("wheel filename tags/build differ from WHEEL metadata".into());
            }
        }
        validate_record(&files, info, read("RECORD")?)?;
        let requirements = metadata.get("requires-dist").cloned().unwrap_or_default();
        for requirement in &requirements {
            requirement
                .parse::<Requirement>()
                .map_err(|error| format!("invalid wheel Requires-Dist: {error}"))?;
        }
        let extras = metadata.get("provides-extra").cloned().unwrap_or_default();
        for extra in &extras {
            extra
                .parse::<pep508_rs::ExtraName>()
                .map_err(|error| format!("invalid wheel Provides-Extra: {error}"))?;
        }
        let requires_python = optional_header(&metadata, "requires-python")?.map(str::to_owned);
        if let Some(rule) = &requires_python {
            rule.parse::<pep440_rs::VersionSpecifiers>()
                .map_err(|error| error.to_string())?;
        }
        let stem = stem.to_owned();
        Ok(Self {
            files,
            stem,
            requirements,
            extras,
            requires_python,
        })
    }

    /// Install into the interpreter's real prefix so sys.prefix, sysconfig,
    /// standalone scripts and the isolated runner agree on resource locations.
    pub fn install(
        self,
        interpreter: &str,
        python_version: &str,
    ) -> Result<Vec<InstalledFile>, String> {
        super::valid_path(interpreter)?;
        let prefix = interpreter
            .rsplit_once("/bin/")
            .map(|(prefix, _)| prefix)
            .ok_or("Python installation executable must live in its prefix/bin")?;
        super::valid_path(prefix)?;
        if python_version.split('.').count() != 2
            || python_version
                .split('.')
                .any(|part| part.parse::<u32>().is_err())
        {
            return Err("Python installation requires a major.minor version".into());
        }
        let library = format!("{prefix}/lib/python{python_version}/site-packages");
        let name = self.stem.rsplit_once('-').unwrap().0;
        let data_prefix = format!("{}.data/", self.stem);
        let mut installed = BTreeSet::new();
        let mut output = Vec::with_capacity(self.files.len());
        let entry_points = self
            .files
            .iter()
            .find(|(path, _)| path == &format!("{}.dist-info/entry_points.txt", self.stem))
            .map(|(_, bytes)| scripts::entry_points(bytes, prefix, interpreter))
            .transpose()?
            .unwrap_or_default();
        for (path, mut bytes) in self.files {
            let (destination, executable) = if let Some(relative) = path.strip_prefix(&data_prefix)
            {
                let (scheme, relative) = relative
                    .split_once('/')
                    .ok_or("wheel data scheme has no file")?;
                let destination = match scheme {
                    "purelib" | "platlib" => format!("{library}/{relative}"),
                    "headers" => {
                        format!("{prefix}/include/python{python_version}/{name}/{relative}")
                    }
                    "scripts" if !relative.contains('/') => format!("{prefix}/bin/{relative}"),
                    "data" => format!("{prefix}/{relative}"),
                    _ => return Err("invalid wheel data installation scheme".into()),
                };
                if scheme == "scripts"
                    && (bytes.starts_with(b"#!python\n")
                        || bytes.starts_with(b"#!pythonw\n")
                        || bytes.starts_with(b"#!python\r\n")
                        || bytes.starts_with(b"#!pythonw\r\n"))
                {
                    let newline = bytes.iter().position(|byte| *byte == b'\n').unwrap();
                    bytes = [
                        format!("#!/worker/{interpreter} -I\n").as_bytes(),
                        &bytes[newline + 1..],
                    ]
                    .concat();
                }
                (destination, scheme == "scripts")
            } else {
                (format!("{library}/{path}"), false)
            };
            super::valid_path(&destination)?;
            if !installed.insert(destination.clone()) {
                return Err("wheel installation schemes produce conflicting files".into());
            }
            output.push(InstalledFile {
                path: destination,
                bytes,
                executable,
            });
        }
        for file in entry_points {
            if !installed.insert(file.path.clone()) {
                return Err("wheel entry point conflicts with another installed script".into());
            }
            output.push(file);
        }
        let record_path = format!("{library}/{}.dist-info/RECORD", self.stem);
        let mut record = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(Vec::new());
        for file in &output {
            let base = library.split('/').collect::<Vec<_>>();
            let target = file.path.split('/').collect::<Vec<_>>();
            let common = base
                .iter()
                .zip(&target)
                .take_while(|(left, right)| left == right)
                .count();
            let relative = format!(
                "{}{}",
                "../".repeat(base.len() - common),
                target[common..].join("/")
            );
            let (hash, size) = if file.path == record_path {
                (String::new(), String::new())
            } else {
                (
                    format!(
                        "sha256={}",
                        URL_SAFE_NO_PAD.encode(Sha256::digest(&file.bytes))
                    ),
                    file.bytes.len().to_string(),
                )
            };
            record
                .write_record([relative, hash, size])
                .map_err(|error| error.to_string())?;
        }
        output
            .iter_mut()
            .find(|file| file.path == record_path)
            .expect("validated RECORD")
            .bytes = record.into_inner().map_err(|error| error.to_string())?;
        Ok(output)
    }
}

fn validate_record(files: &[(String, Vec<u8>)], info: &str, bytes: &[u8]) -> Result<(), String> {
    let entries = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes))
        .collect::<BTreeMap<_, _>>();
    let record_path = format!("{info}/RECORD");
    let mut seen = BTreeSet::new();
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(false)
        .from_reader(bytes);
    for row in reader.records() {
        let row = row.map_err(|error| format!("invalid wheel RECORD CSV: {error}"))?;
        if row.len() != 3 {
            return Err("wheel RECORD requires three fields".into());
        }
        let path = &row[0];
        super::valid_path(path)?;
        if !seen.insert(path.to_owned()) {
            return Err("wheel RECORD repeats a file".into());
        }
        let content = entries
            .get(path)
            .ok_or("wheel RECORD references an absent file")?;
        if path == record_path {
            if !row[1].is_empty() || !row[2].is_empty() {
                return Err("wheel RECORD cannot hash itself".into());
            }
            continue;
        }
        if path == format!("{info}/RECORD.jws") || path == format!("{info}/RECORD.p7s") {
            return Err("wheel signatures must not be listed in RECORD".into());
        }
        let (algorithm, expected) = row[1]
            .split_once('=')
            .ok_or("wheel RECORD lacks a secure hash")?;
        let actual = match algorithm {
            "sha256" => URL_SAFE_NO_PAD.encode(Sha256::digest(content)),
            "sha384" => URL_SAFE_NO_PAD.encode(Sha384::digest(content)),
            "sha512" => URL_SAFE_NO_PAD.encode(Sha512::digest(content)),
            _ => return Err("wheel RECORD hash must use SHA-256 or stronger SHA-2".into()),
        };
        if actual != expected || row[2].parse::<usize>().ok() != Some(content.len()) {
            return Err("wheel RECORD hash or size does not match its file".into());
        }
    }
    if !seen.contains(&record_path)
        || entries.keys().any(|path| {
            !seen.contains(*path)
                && **path != format!("{info}/RECORD.jws")
                && **path != format!("{info}/RECORD.p7s")
        })
    {
        return Err("wheel RECORD does not cover every file".into());
    }
    Ok(())
}
