use super::super::release::Artifact;
use super::*;
use sha2::{Digest, Sha256};

fn manifest() -> ReleaseManifest {
    ReleaseManifest::new(
        Version::parse("1.2.3").unwrap(),
        vec![Artifact {
            path: "dever-core".into(),
            bytes: 4,
            sha256: super::super::release::hex(&Sha256::digest(b"core")),
        }],
    )
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = tar::Builder::new(Vec::new());
    for (name, contents) in entries {
        let mut header = tar::Header::new_ustar();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, name, *contents).unwrap();
    }
    archive.into_inner().unwrap()
}

#[test]
fn release_archive_accepts_only_the_exact_signed_regular_files() {
    let root = Download::new(&std::env::temp_dir()).unwrap();
    unpack(
        archive(&[("dever-core", b"core")]).as_slice(),
        &root.0,
        &manifest(),
    )
    .unwrap();
    assert_eq!(fs::read(root.0.join("dever-core")).unwrap(), b"core");
    for entries in [
        vec![
            ("dever-core", b"core".as_slice()),
            ("extra", b"x".as_slice()),
        ],
        vec![
            ("dever-core", b"core".as_slice()),
            ("dever-core", b"core".as_slice()),
        ],
        vec![("dever-core", b"bad".as_slice())],
        vec![],
    ] {
        let root = Download::new(&std::env::temp_dir()).unwrap();
        assert!(unpack(archive(&entries).as_slice(), &root.0, &manifest()).is_err());
    }
}

#[test]
fn release_archive_rejects_links_and_extension_headers() {
    for kind in [
        tar::EntryType::Symlink,
        tar::EntryType::Link,
        tar::EntryType::XGlobalHeader,
    ] {
        let root = Download::new(&std::env::temp_dir()).unwrap();
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_ustar();
        header.set_path("dever-core").unwrap();
        header.set_entry_type(kind);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_link_name("/outside").unwrap();
        header.set_cksum();
        archive.append(&header, io::empty()).unwrap();
        assert!(
            unpack(
                archive.into_inner().unwrap().as_slice(),
                &root.0,
                &manifest()
            )
            .is_err()
        );
        assert!(!root.0.join("dever-core").exists());
    }
}

#[test]
fn signed_catalog_paths_and_total_size_are_bounded_before_extraction() {
    for path in [
        "/absolute",
        "../outside",
        "a/../b",
        "./file",
        "a//b",
        "a\\b",
        "C:drive",
        "manifest.json",
        "manifest.sig",
    ] {
        let mut manifest = manifest();
        manifest.artifacts[0].path = path.into();
        assert!(validate_catalog(&manifest).is_err(), "{path}");
    }
    let mut excessive = manifest();
    excessive.artifacts[0].bytes = RELEASE_LIMIT + 1;
    assert!(validate_catalog(&excessive).is_err());
    let mut duplicate = manifest();
    duplicate.artifacts.push(duplicate.artifacts[0].clone());
    assert!(validate_catalog(&duplicate).is_err());
}

#[test]
fn official_redirects_reject_http_credentials_ports_and_foreign_hosts() {
    let current = "https://github.com/shemic/dever-main/releases/latest/download/a";
    assert!(
        redirect(
            current,
            "https://release-assets.githubusercontent.com/asset?token=opaque"
        )
        .is_ok()
    );
    assert!(redirect(current, "/shemic/dever-main/releases/download/v1.2.3/a").is_ok());
    for address in [
        "http://github.com/a",
        "https://evil.example/a",
        "https://github.com.evil.example/a",
        "https://user@github.com/a",
        "https://github.com:444/a",
        "https://github.com/a#fragment",
    ] {
        assert!(redirect(current, address).is_err(), "{address}");
    }
}

#[test]
fn official_asset_names_are_versioned_and_platform_specific() {
    let source = ReleaseSource::official();
    assert_eq!(
        source.asset("latest", "manifest.json"),
        format!(
            "{OFFICIAL_RELEASES}/latest/download/dever-{}.manifest.json",
            platform_identity()
        )
    );
    assert_eq!(
        source.asset("1.2.3", "tar.zst"),
        format!(
            "{OFFICIAL_RELEASES}/download/v1.2.3/dever-{}.tar.zst",
            platform_identity()
        )
    );
}

fn signed_release() -> (ReleaseManifest, Vec<u8>, Vec<u8>, Vec<u8>) {
    use ring::signature::{Ed25519KeyPair, KeyPair};
    let pair = Ed25519KeyPair::from_seed_unchecked(&[37; 32]).unwrap();
    let mut manifest = manifest();
    manifest.artifacts.push(Artifact {
        path: "skills/dever-language/SKILL.md".into(),
        bytes: 5,
        sha256: super::super::release::hex(&Sha256::digest(b"skill")),
    });
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = super::super::release::hex(pair.sign(&bytes).as_ref()).into_bytes();
    (
        manifest,
        bytes,
        signature,
        pair.public_key().as_ref().to_vec(),
    )
}

fn server(responses: Vec<Vec<u8>>) -> (ReleaseSource, std::thread::JoinHandle<()>) {
    use std::io::BufRead;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/releases", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        for response in responses {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "expected release request did not arrive"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = io::BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            assert!(request.starts_with("GET /releases/"));
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            )
            .unwrap();
            stream.write_all(&response).unwrap();
        }
    });
    (ReleaseSource::new(base), worker)
}

fn compressed_release(core: &[u8]) -> Vec<u8> {
    let bytes = archive(&[
        ("dever-core", core),
        ("skills/dever-language/SKILL.md", b"skill"),
    ]);
    compressed(&bytes)
}

fn compressed(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
    encoder.include_checksum(true).unwrap();
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn zstandard_frames_require_checksum_bounded_window_and_exact_eof() {
    let tar = archive(&[("dever-core", b"core")]);
    let good = compressed(&tar);
    let root = Download::new(&std::env::temp_dir()).unwrap();
    super::super::release_archive::extract(good.as_slice(), &root.0, &manifest().artifacts)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(root.0.join("dever-core"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755,
            "signed executable roles retain execution after extraction"
        );
    }
    let mut bad_checksum = good.clone();
    *bad_checksum.last_mut().unwrap() ^= 1;
    let mut concatenated = good.clone();
    concatenated.extend(&good);
    let mut garbage = good.clone();
    garbage.push(0);
    let oversized_window = vec![
        0x28,
        0xb5,
        0x2f,
        0xfd,
        4,
        (28 - 10) << 3,
        1,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    for bytes in [
        bad_checksum,
        concatenated,
        garbage,
        oversized_window,
        good[..good.len() - 1].to_vec(),
        zstd::stream::encode_all(tar.as_slice(), 1).unwrap(),
    ] {
        let root = Download::new(&std::env::temp_dir()).unwrap();
        assert!(
            super::super::release_archive::extract(
                bytes.as_slice(),
                &root.0,
                &manifest().artifacts
            )
            .is_err()
        );
    }
}

#[test]
fn tar_trailers_and_same_sized_corruption_are_rejected() {
    let tar = archive(&[("dever-core", b"core")]);
    let mut nonzero_tail = tar.clone();
    nonzero_tail.push(1);
    let mut nonzero_padding = tar.clone();
    nonzero_padding[516] = 1;
    for bytes in [
        nonzero_tail,
        nonzero_padding,
        tar[..tar.len() - 512].to_vec(),
        archive(&[("dever-core", b"evil")]),
    ] {
        let root = Download::new(&std::env::temp_dir()).unwrap();
        assert!(
            super::super::release_archive::extract(
                compressed(&bytes).as_slice(),
                &root.0,
                &manifest().artifacts
            )
            .is_err()
        );
    }
}

#[test]
fn signed_extensions_cannot_duplicate_or_escape_their_typed_namespace() {
    use super::super::{BuildTarget, Extension, ExtensionKind};
    use crate::libs::Ecosystem;
    let mut catalog = manifest();
    catalog.extensions.push(Extension {
        kind: ExtensionKind::Runtime(Ecosystem::Pip),
        target: BuildTarget::LinuxX86_64,
        artifacts: vec![Artifact {
            path: "runtime/pip/linux-x86_64/runtime.pack".into(),
            bytes: 4,
            sha256: catalog.artifacts[0].sha256.clone(),
        }],
    });
    validate_catalog(&catalog).unwrap();
    assert_eq!(
        catalog.extensions[0].id().asset_suffix(),
        "ext-runtime-pip-linux-x86_64.tar.zst"
    );
    let mut duplicate = catalog.clone();
    duplicate.extensions.push(duplicate.extensions[0].clone());
    assert!(validate_catalog(&duplicate).is_err());
    for path in [
        "dever-core",
        "runtime/npm/linux-x86_64/runtime.pack",
        "runtime/pip/linux-aarch64/runtime.pack",
        "runtime/pip/linux-x86_64/../escape",
    ] {
        catalog.extensions[0].artifacts[0].path = path.into();
        assert!(validate_catalog(&catalog).is_err(), "{path}");
    }
}

#[test]
fn explicit_download_verifies_the_complete_bundle_before_publishing_catalog() {
    let root = Download::new(&std::env::temp_dir()).unwrap();
    let layout = Layout::new(root.0.join("machine"));
    layout.initialize().unwrap();
    let (manifest, bytes, signature, key) = signed_release();
    fs::write(
        layout.state().join("trusted-release-key"),
        super::super::release::hex(&key),
    )
    .unwrap();
    let (source, worker) = server(vec![bytes, signature, compressed_release(b"core")]);
    let version = prepare_from(&layout, "latest", &source).unwrap();
    worker.join().unwrap();
    assert_eq!(version, manifest.version);
    let installed = layout.downloads().join(version.as_str());
    MachineManager::verify_package(&installed, Some(&version), &key).unwrap();
    assert_eq!(
        fs::read(installed.join("skills/dever-language/SKILL.md")).unwrap(),
        b"skill"
    );
    assert_eq!(
        fs::read_to_string(layout.downloads().join("latest")).unwrap(),
        "1.2.3\n"
    );
    assert_eq!(fs::read_dir(layout.staging()).unwrap().count(), 0);
}

#[test]
fn bad_signatures_and_corrupt_payloads_preserve_the_previous_catalog_and_active_version() {
    for bad_signature in [true, false] {
        let root = Download::new(&std::env::temp_dir()).unwrap();
        let layout = Layout::new(root.0.join("machine"));
        layout.initialize().unwrap();
        let (_, bytes, signature, key) = signed_release();
        fs::write(
            layout.state().join("trusted-release-key"),
            super::super::release::hex(&key),
        )
        .unwrap();
        fs::write(layout.downloads().join("latest"), "0.9.0\n").unwrap();
        let active = layout.state().join("active-version");
        super::super::release::append_active_version(&active, &Version::parse("0.9.0").unwrap())
            .unwrap();
        let previous = fs::read(&active).unwrap();
        let responses = if bad_signature {
            vec![bytes, vec![b'0'; 128]]
        } else {
            vec![bytes, signature, compressed_release(b"evil")]
        };
        let (source, worker) = server(responses);
        let error = prepare_from(&layout, "latest", &source).unwrap_err();
        worker.join().unwrap();
        assert!(error.contains("verification"), "{error}");
        assert_eq!(fs::read(&active).unwrap(), previous);
        assert_eq!(
            fs::read_to_string(layout.downloads().join("latest")).unwrap(),
            "0.9.0\n"
        );
        assert!(!layout.downloads().join("1.2.3").exists());
        assert_eq!(fs::read_dir(layout.staging()).unwrap().count(), 0);
    }
}
