//! Wheel console/gui entry points share the installed interpreter prefix.

use super::InstalledFile;
use std::collections::{BTreeMap, BTreeSet};

fn identifier(path: &str) -> bool {
    !path.is_empty()
        && path.split('.').all(|part| {
            let mut bytes = part.bytes();
            bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

pub(super) fn entry_points(
    bytes: &[u8],
    prefix: &str,
    interpreter: &str,
) -> Result<Vec<InstalledFile>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "wheel entry points must be UTF-8")?;
    let mut active = false;
    let mut sections = BTreeSet::new();
    let mut entries = BTreeMap::<String, String>::new();
    let mut previous: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        if let Some(section) = trimmed
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            active = matches!(section, "console_scripts" | "gui_scripts");
            if active && !sections.insert(section.to_owned()) {
                return Err("wheel repeats an entry-point group".into());
            }
            previous = None;
            continue;
        }
        if !active {
            continue;
        }
        if line.starts_with(char::is_whitespace) && !trimmed.contains('=') {
            let name = previous
                .as_ref()
                .ok_or("wheel entry point has an orphan continuation")?;
            let value = entries.get_mut(name).expect("previous entry");
            value.push(' ');
            value.push_str(trimmed);
            continue;
        }
        let (name, value) = trimmed
            .split_once('=')
            .ok_or("invalid wheel script entry point")?;
        let name = name.trim();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            || matches!(name, "." | "..")
        {
            return Err("wheel script name must be a portable file name".into());
        }
        if entries
            .insert(name.to_owned(), value.trim().to_owned())
            .is_some()
        {
            return Err("wheel repeats a script name".into());
        }
        previous = Some(name.to_owned());
    }
    entries.into_iter().map(|(name,value)| {
        let value = if let Some((target,extras))=value.split_once('[') {
            let extras=extras.strip_suffix(']').ok_or("invalid entry-point extras")?;
            for extra in extras.split(',') {
                extra.trim().parse::<pep508_rs::ExtraName>().map_err(|error|format!("invalid entry-point extra: {error}"))?;
            }
            target.trim()
        }else{value.trim()};
        let (module,function)=value.split_once(':').ok_or("wheel console script must name a callable")?;
        if !identifier(module) || !identifier(function) { return Err("wheel script has an invalid Python entry point".into()); }
        let source=format!("#!/worker/{interpreter} -I\nimport importlib\nimport re\nimport sys\nentry = importlib.import_module({module:?})\nfor part in {function:?}.split('.'):\n    entry = getattr(entry, part)\nsys.argv[0] = re.sub(r'(-script\\.pyw|\\.exe)?$', '', sys.argv[0])\nsys.exit(entry())\n");
        Ok(InstalledFile {path:format!("{prefix}/bin/{name}"),bytes:source.into_bytes(),executable:true})
    }).collect()
}
