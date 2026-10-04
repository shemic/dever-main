use std::io::{Cursor, Write};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

pub fn metadata(name: &str, version: &str, requires: &[&str], extras: &[&str]) -> String {
    let mut text = format!("Metadata-Version: 2.4\nName: {name}\nVersion: {version}\n");
    for requirement in requires {
        text.push_str(&format!("Requires-Dist: {requirement}\n"));
    }
    for extra in extras {
        text.push_str(&format!("Provides-Extra: {extra}\n"));
    }
    text
}

pub fn pack(
    name: &str,
    version: &str,
    wheel: &str,
    metadata: &str,
    extra: &[(&str, Vec<u8>)],
) -> Vec<u8> {
    let info = format!("{}-{version}.dist-info", name.replace('-', "_"));
    let mut files = vec![
        (format!("{info}/WHEEL"), wheel.as_bytes().to_vec()),
        (format!("{info}/METADATA"), metadata.as_bytes().to_vec()),
    ];
    files.extend(
        extra
            .iter()
            .map(|(path, bytes)| ((*path).to_owned(), bytes.clone())),
    );
    let mut record = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    for (path, bytes) in &files {
        record
            .write_record([
                path.clone(),
                format!("sha256={}", URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))),
                bytes.len().to_string(),
            ])
            .unwrap();
    }
    let path = format!("{info}/RECORD");
    record
        .write_record([path.clone(), String::new(), String::new()])
        .unwrap();
    files.push((path, record.into_inner().unwrap()));
    zip(&files)
}

pub fn zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in files {
        writer
            .start_file(path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

pub fn markers() -> pep508_rs::MarkerEnvironment {
    pep508_rs::MarkerEnvironmentBuilder {
        implementation_name: "cpython",
        implementation_version: "3.12.0",
        os_name: "posix",
        platform_machine: "x86_64",
        platform_python_implementation: "CPython",
        platform_release: "",
        platform_system: "Linux",
        platform_version: "",
        python_full_version: "3.12.0",
        python_version: "3.12",
        sys_platform: "linux",
    }
    .try_into()
    .unwrap()
}
