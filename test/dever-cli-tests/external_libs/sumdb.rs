use super::*;
use dever_cli::libs::sumdb::{self, ChecksumDatabase, Evidence, Verifier};
use ring::signature::{Ed25519KeyPair, KeyPair};

fn key() -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(&[47; 32]).unwrap()
}

pub(super) fn verifier() -> Verifier {
    let mut encoded = vec![1];
    encoded.extend(key().public_key().as_ref());
    let mut hash = Sha256::new();
    hash.update(b"dever-sumdb-test\n");
    hash.update(&encoded);
    let id = hash.finalize();
    Verifier::new(&format!(
        "dever-sumdb-test+{}+{}",
        id[..4]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        base64::engine::general_purpose::STANDARD.encode(encoded)
    ))
    .unwrap()
}

fn signed(size: usize, root: [u8; 32]) -> String {
    let text = format!(
        "go.sum database tree\n{size}\n{}\n",
        base64::engine::general_purpose::STANDARD.encode(root)
    );
    let mut hash = Sha256::new();
    hash.update(b"dever-sumdb-test\n");
    hash.update([1]);
    hash.update(key().public_key().as_ref());
    let mut signature = hash.finalize()[..4].to_vec();
    signature.extend(key().sign(text.as_bytes()).as_ref());
    format!(
        "{text}\n— dever-sumdb-test {}\n",
        base64::engine::general_purpose::STANDARD.encode(signature)
    )
}

fn leaf(record: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update([0]);
    hash.update(record);
    hash.finalize().into()
}

fn root(hashes: &[[u8; 32]]) -> [u8; 32] {
    if hashes.is_empty() {
        return Sha256::digest([]).into();
    }
    if hashes.len() == 1 {
        return hashes[0];
    }
    let split = hashes.len().next_power_of_two() / 2;
    let mut hash = Sha256::new();
    hash.update([1]);
    hash.update(root(&hashes[..split]));
    hash.update(root(&hashes[split..]));
    hash.finalize().into()
}

fn unescape(path: &str) -> String {
    let mut chars = path.chars();
    let mut output = String::new();
    while let Some(ch) = chars.next() {
        output.push(if ch == '!' {
            chars.next().unwrap().to_ascii_uppercase()
        } else {
            ch
        });
    }
    output
}

pub(super) fn seal(registry: &mut LocalRegistry) {
    registry
        .0
        .retain(|(_, path), _| !path.starts_with("https://sum.golang.org/"));
    let records = registry
        .0
        .iter()
        .filter_map(|((ecosystem, path), bytes)| {
            if *ecosystem != Ecosystem::Go || !path.ends_with(".mod") {
                return None;
            }
            let (module, version) = path.strip_prefix('/')?.split_once("/@v/v")?;
            let version = version.strip_suffix(".mod")?;
            let module = unescape(module);
            let archive = registry.0.get(&(
                Ecosystem::Go,
                format!("{}zip", path.strip_suffix("mod").unwrap()),
            ))?;
            let record = format!(
                "{module} v{version} {}\n{module} v{version}/go.mod {}\n",
                sumdb::zip_hash(&module, version, archive).unwrap(),
                sumdb::mod_hash(bytes)
            );
            Some((
                format!(
                    "/lookup/{}@v{version}",
                    path.strip_prefix('/')
                        .unwrap()
                        .split("/@v/")
                        .next()
                        .unwrap()
                ),
                record,
            ))
        })
        .collect::<Vec<_>>();
    let hashes = records
        .iter()
        .map(|(_, record)| leaf(record.as_bytes()))
        .collect::<Vec<_>>();
    let checkpoint = signed(records.len(), root(&hashes));
    registry.0.insert(
        (Ecosystem::Go, "https://sum.golang.org/latest".into()),
        checkpoint.as_bytes().to_vec(),
    );
    for (id, (path, record)) in records.iter().enumerate() {
        registry.0.insert(
            (Ecosystem::Go, format!("https://sum.golang.org{path}")),
            format!("{id}\n{record}\n{checkpoint}").into_bytes(),
        );
    }
    // Fixtures deliberately remain below one tile. Larger trees use official
    // captured vectors, not a second production tile-address implementation.
    assert!(hashes.len() < 256);
    if !hashes.is_empty() {
        registry.0.insert(
            (
                Ecosystem::Go,
                format!("https://sum.golang.org/tile/8/0/000.p/{}", hashes.len()),
            ),
            hashes.iter().flatten().copied().collect(),
        );
    }
}

pub(super) struct Transport<'a> {
    pub registry: &'a dyn RegistryTransport,
    pub checksums: &'a LocalRegistry,
}
impl RegistryTransport for Transport<'_> {
    fn get(&self, ecosystem: Ecosystem, path: &str, limit: usize) -> Result<Vec<u8>, String> {
        if path.starts_with("https://sum.golang.org/") {
            self.checksums.get(ecosystem, path, limit)
        } else {
            self.registry.get(ecosystem, path, limit)
        }
    }
}

fn resolved(
    registry: &LocalRegistry,
    previous: Option<String>,
    roots: &[&str],
) -> Result<LockFile, String> {
    let cache = FixtureArtifactStore::default();
    let mut resolver = local_resolver(registry, &cache);
    resolver.go_sumdb = ChecksumDatabase::new(verifier(), previous)?;
    dever_cli::libs::LibResolver::resolve(
        &resolver,
        &roots
            .iter()
            .map(|root| root.parse().unwrap())
            .collect::<Vec<_>>(),
    )
}

#[test]
fn authenticated_modules_replay_offline_and_reject_content_or_proof_drift() {
    let registry = local_registry();
    let lock = resolved(&registry, None, &["go:example.com/a@1.0.0"]).unwrap();
    let evidence = &lock.go_sumdb[0];
    let checked = evidence.verify(&verifier()).unwrap();
    let archive = &registry.0[&(Ecosystem::Go, "/example.com/a/@v/v1.0.0.zip".into())];
    checked
        .verify_zip("example.com/a", "1.0.0", archive)
        .unwrap();
    assert!(
        evidence
            .verify(&Verifier::official())
            .unwrap_err()
            .contains("trusted signature")
    );
    let mut changed = evidence.clone();
    changed.records[0].go_mod.push_str("// changed\n");
    assert!(changed.verify(&verifier()).unwrap_err().contains("go.mod"));
    let mut changed = evidence.clone();
    changed.records[0].module = "wrong.example/a".into();
    assert!(
        changed
            .verify(&verifier())
            .unwrap_err()
            .contains("different module/version")
    );
    let mut changed = evidence.clone();
    changed.records[0].inclusion[0] = base64::engine::general_purpose::STANDARD.encode([0; 32]);
    assert!(
        changed
            .verify(&verifier())
            .unwrap_err()
            .contains("inclusion proof")
    );
    let mut changed = evidence.clone();
    changed.checkpoint = changed
        .checkpoint
        .replace("go.sum database tree", "go.sum database free");
    assert!(
        changed
            .verify(&verifier())
            .unwrap_err()
            .contains("signature verification")
    );
    assert!(
        checked
            .verify_zip(
                "example.com/a",
                "1.0.0",
                &zip_file("example.com/a@v1.0.0/changed.go")
            )
            .unwrap_err()
            .contains("sumdb h1")
    );
}

#[test]
fn checkpoint_updates_require_consistency_and_never_move_backwards() {
    let mut first = local_registry();
    first
        .0
        .remove(&(Ecosystem::Go, "/example.com/b/@v/v1.2.0.mod".into()));
    first.0.insert(
        (Ecosystem::Go, "/example.com/a/@v/v1.0.0.mod".into()),
        b"module example.com/a\n".to_vec(),
    );
    seal(&mut first);
    let old = resolved(&first, None, &["go:example.com/a@1.0.0"]).unwrap();
    let checkpoint = old.go_sumdb[0].verify(&verifier()).unwrap().checkpoint;
    let mut next = first;
    next.0.insert(
        (Ecosystem::Go, "/example.com/z/@v/v1.0.0.mod".into()),
        b"module example.com/z\n".to_vec(),
    );
    next.0.insert(
        (Ecosystem::Go, "/example.com/z/@v/v1.0.0.zip".into()),
        zip_file("example.com/z@v1.0.0/z.go"),
    );
    seal(&mut next);
    let new = resolved(&next, Some(checkpoint.clone()), &["go:example.com/z@1.0.0"]).unwrap();
    let current = new.go_sumdb[0].verify(&verifier()).unwrap().checkpoint;
    assert!(resolved(&next, Some(current.clone()), &["go:example.com/z@1.0.0"]).is_ok());
    next.0.insert(
        (Ecosystem::Go, "https://sum.golang.org/latest".into()),
        checkpoint.into_bytes(),
    );
    assert!(
        resolved(&next, Some(current), &["go:example.com/a@1.0.0"])
            .unwrap_err()
            .contains("rollback")
    );
    let mut fork = new.go_sumdb[0].clone();
    fork.consistency[0] = base64::engine::general_purpose::STANDARD.encode([0; 32]);
    assert!(
        fork.verify(&verifier())
            .unwrap_err()
            .contains("conflicting history")
    );
}

#[test]
fn sumdb_verifier_key_and_upstream_signed_note_vectors() {
    let note = "go.sum database tree\n63512409\nF7eHRPqN/cqPtqti14NKdGGaI1Giw8coWP3KxihFZbg=\n\n— sum.golang.org Az3gri1OBHKT87Xj8UXU8zjoHGy/X+7Nl94f8TZkELVaasolmGApUsWqwVG9vE6hgqYb/ph/w2vdkLIVRH7bMB/FPQ4=\n";
    let evidence = Evidence {
        previous: None,
        checkpoint: note.into(),
        consistency: vec![],
        records: vec![],
    };
    evidence.verify(&Verifier::official()).unwrap();
    assert!(evidence.verify(&verifier()).is_err());
    assert!(Verifier::new(&sumdb::OFFICIAL_VERIFIER.replace("033de0ae", "033de0af")).is_err());
}

#[test]
fn official_large_tree_inclusion_and_consistency_vectors_verify_offline() {
    // Captured from sum.golang.org/lookup/github.com/google/uuid@v1.6.0
    // and its immutable height-8 tiles on 2026-10-02. Hashes were assembled
    // independently of this Rust implementation. Both notes retain Google's
    // original Ed25519 signatures; no fixture signer can mint these vectors.
    let evidence: Evidence = serde_json::from_str(include_str!("sumdb_uuid.json")).unwrap();
    let trusted = Verifier::official();
    let verified = evidence.verify(&trusted).unwrap();
    assert!(verified.contains("github.com/google/uuid", "1.6.0"));
    assert_eq!(
        sumdb::mod_hash(b"module github.com/google/uuid\n"),
        "h1:TIyPZe4MgqvfeYDBFedMoGGpEw/LqOeaOT+nhxU+yHo="
    );
    for index in 0..evidence.records[0].inclusion.len() {
        let mut changed = evidence.clone();
        changed.records[0].inclusion[index] =
            base64::engine::general_purpose::STANDARD.encode([0; 32]);
        assert!(
            changed.verify(&trusted).is_err(),
            "accepted corrupt inclusion hash {index}"
        );
    }
    for index in 0..evidence.consistency.len() {
        let mut changed = evidence.clone();
        changed.consistency[index] = base64::engine::general_purpose::STANDARD.encode([0; 32]);
        assert!(
            changed.verify(&trusted).is_err(),
            "accepted corrupt consistency hash {index}"
        );
    }
}

#[test]
fn go_h1_ignores_zip_order_and_compression_and_preserves_module_case() {
    let archive = |reverse, compression| {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let mut files = vec![
            ("a.go", b"package a\n".as_slice()),
            ("LICENSE", b"fixture license\n".as_slice()),
        ];
        if reverse {
            files.reverse();
        }
        for (name, content) in files {
            writer
                .start_file(
                    format!("example.com/Mixed@v1.0.0/{name}"),
                    zip::write::SimpleFileOptions::default().compression_method(compression),
                )
                .unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    };
    let first = archive(false, zip::CompressionMethod::Stored);
    let second = archive(true, zip::CompressionMethod::Deflated);
    assert_ne!(digest(&first), digest(&second));
    assert_eq!(
        sumdb::zip_hash("example.com/Mixed", "1.0.0", &first).unwrap(),
        sumdb::zip_hash("example.com/Mixed", "1.0.0", &second).unwrap()
    );
    let mut registry = LocalRegistry(BTreeMap::from([
        (
            (Ecosystem::Go, "/example.com/!mixed/@v/v1.0.0.mod".into()),
            b"module example.com/Mixed\n".to_vec(),
        ),
        (
            (Ecosystem::Go, "/example.com/!mixed/@v/v1.0.0.zip".into()),
            first,
        ),
    ]));
    seal(&mut registry);
    let lock = resolved(&registry, None, &["go:example.com/Mixed@1.0.0"]).unwrap();
    assert_eq!(lock.libs[0].spec.name, "example.com/Mixed");
    assert!(
        lock.go_sumdb[0]
            .verify(&verifier())
            .unwrap()
            .contains("example.com/Mixed", "1.0.0")
    );
}

#[test]
fn every_go_mod_consulted_by_mvs_is_authenticated_even_when_not_selected() {
    let mut registry = LocalRegistry(BTreeMap::new());
    for (name, version, source) in [
        (
            "a",
            "1.0.0",
            "module example.com/a\nrequire (\nexample.com/b v1.0.0\nexample.com/c v1.0.0\n)\n",
        ),
        ("b", "1.0.0", "module example.com/b\n"),
        ("b", "2.0.0", "module example.com/b\n"),
        (
            "c",
            "1.0.0",
            "module example.com/c\nrequire example.com/b v2.0.0\n",
        ),
    ] {
        registry.0.insert(
            (
                Ecosystem::Go,
                format!("/example.com/{name}/@v/v{version}.mod"),
            ),
            source.as_bytes().to_vec(),
        );
        registry.0.insert(
            (
                Ecosystem::Go,
                format!("/example.com/{name}/@v/v{version}.zip"),
            ),
            zip_file(&format!("example.com/{name}@v{version}/{name}.go")),
        );
    }
    seal(&mut registry);
    let lock = resolved(&registry, None, &["go:example.com/a@1.0.0"]).unwrap();
    assert!(
        !lock
            .libs
            .iter()
            .any(|lib| lib.spec.key() == "go:example.com/b@1.0.0")
    );
    assert!(
        lock.go_sumdb[0]
            .records
            .iter()
            .any(|record| record.module == "example.com/b" && record.version == "1.0.0")
    );
    registry
        .0
        .get_mut(&(Ecosystem::Go, "/example.com/b/@v/v1.0.0.mod".into()))
        .unwrap()
        .extend(b"// modified discarded metadata\n");
    let cache = FixtureArtifactStore::default();
    let resolver = local_resolver(&registry, &cache);
    assert!(
        dever_cli::libs::LibResolver::resolve(
            &resolver,
            &["go:example.com/a@1.0.0".parse().unwrap()]
        )
        .unwrap_err()
        .contains("go.mod does not match")
    );
    for ((_, path), bytes) in &registry.0 {
        if path.ends_with(".zip") {
            assert!(!cache.has(&digest(bytes), "linux-x86_64").unwrap());
        }
    }
}

#[test]
fn go_hashzip_includes_empty_directory_entries() {
    let archive = |directory| {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "example.com/a@v1.0.0/a.go",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"package a\n").unwrap();
        if directory {
            writer
                .add_directory(
                    "example.com/a@v1.0.0/empty/",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
        }
        writer.finish().unwrap().into_inner()
    };
    // Independent Hash1 vectors: sorted "sha256(content)  filename\n" records,
    // including the directory's trailing slash and SHA-256 of empty content.
    assert_eq!(
        sumdb::zip_hash("example.com/a", "1.0.0", &archive(false)).unwrap(),
        "h1:VEUcYKYLbKBYq0eA/VEJ3mMQBzi4jjWpusjuduGJcBQ="
    );
    assert_eq!(
        sumdb::zip_hash("example.com/a", "1.0.0", &archive(true)).unwrap(),
        "h1:y9ZWKWCmVsKnLxdFkIVoxr4MyXyHgBUvp1hgBzZ+IfE="
    );
}

#[test]
fn project_updates_and_removing_last_lib_preserve_official_checkpoint_anchor() {
    let root = TemporaryDirectory::new();
    let evidence: Evidence = serde_json::from_str(include_str!("sumdb_uuid.json")).unwrap();
    let mut lock = LockFile::new(Vec::new()).unwrap();
    lock.go_sumdb.push(evidence.clone());
    lock.write_atomic(root.path()).unwrap();
    let spec = vec!["pip:fixture@1.0.0".into()];
    for command in ["add", "update", "remove"] {
        dever_cli::libs::execute(command, root.path(), &spec).unwrap();
        let lock = LockFile::decode(&fs::read(root.path().join("dever.lock")).unwrap()).unwrap();
        assert_eq!(lock.go_sumdb, vec![evidence.clone()]);
        let current = sumdb::verify_chain(&lock.go_sumdb, &Verifier::official()).unwrap();
        let previous = evidence.previous.clone().unwrap();
        // A subsequent online resolution cannot accept the older official tree.
        let mut registry = local_registry();
        registry.0.insert(
            (Ecosystem::Go, "https://sum.golang.org/latest".into()),
            previous.into_bytes(),
        );
        let cache = FixtureArtifactStore::default();
        let mut resolver = local_resolver(&registry, &cache);
        resolver.go_sumdb = ChecksumDatabase::official(Some(current.checkpoint)).unwrap();
        assert!(
            dever_cli::libs::LibResolver::resolve(
                &resolver,
                &["go:example.com/a@1.0.0".parse().unwrap()]
            )
            .unwrap_err()
            .contains("rollback")
        );
    }
}

#[test]
fn lock_publication_rejects_offline_byte_and_element_overflow_before_replacement() {
    let root = TemporaryDirectory::new();
    let original = LockFile::new(Vec::new()).unwrap();
    original.write_atomic(root.path()).unwrap();
    let path = root.path().join("dever.lock");
    let before = fs::read(&path).unwrap();
    let receipt = Evidence {
        previous: None,
        checkpoint: "x".into(),
        consistency: Vec::new(),
        records: Vec::new(),
    };
    for overflow in [true, false] {
        let mut lock = original.clone();
        let mut receipt = receipt.clone();
        if overflow {
            receipt.checkpoint = "x".repeat(dever_runtime::wire::MAX_BYTES);
        } else {
            receipt.consistency = vec!["x".into(); dever_runtime::wire::MAX_ELEMENTS];
        }
        lock.go_sumdb.push(receipt);
        assert!(lock.encode().unwrap_err().contains("invalid dever.lock"));
        assert!(lock.write_atomic(root.path()).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn truncated_surplus_and_out_of_bounds_sumdb_proofs_are_rejected() {
    let original: Evidence = serde_json::from_str(include_str!("sumdb_uuid.json")).unwrap();
    let trusted = Verifier::official();
    let mut changed = original.clone();
    changed.records[0].inclusion.pop();
    assert!(changed.verify(&trusted).is_err());
    let mut changed = original.clone();
    changed.records[0]
        .inclusion
        .push(base64::engine::general_purpose::STANDARD.encode([0; 32]));
    assert!(changed.verify(&trusted).is_err());
    for id in ["66321505", "09223372036854775807", "9223372036854775808"] {
        let mut changed = original.clone();
        let (_, suffix) = changed.records[0].lookup.split_once('\n').unwrap();
        changed.records[0].lookup = format!("{id}\n{suffix}");
        assert!(changed.verify(&trusted).is_err());
    }
    let mut changed = original.clone();
    changed.checkpoint.pop();
    assert!(changed.verify(&trusted).is_err());
    let mut disconnected = original.clone();
    disconnected.previous = None;
    disconnected.consistency.clear();
    assert!(
        sumdb::verify_chain(&[original, disconnected], &trusted)
            .unwrap_err()
            .contains("not connected")
    );
}

#[cfg(unix)]
#[test]
fn lib_and_package_mutations_share_a_stable_nonblocking_project_lock() {
    use std::os::unix::fs::OpenOptionsExt;
    let root = TemporaryDirectory::new();
    let requested = vec!["pip:fixture@1.0.0".into()];
    dever_cli::libs::execute("add", root.path(), &requested).unwrap();
    let lock_path = root.path().join("dever.lock");
    let config_path = root.path().join("config/setting.json");
    let before_lock = fs::read(&lock_path).unwrap();
    let before_config = fs::read(&config_path).unwrap();
    let guard = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .mode(0o600)
        .open(root.path().join(".dever-project.lock"))
        .unwrap();
    fs2::FileExt::try_lock_exclusive(&guard).unwrap();
    for command in ["add", "update", "remove"] {
        assert!(
            dever_cli::libs::execute(command, root.path(), &requested)
                .unwrap_err()
                .contains("mutation is busy")
        );
        assert!(
            dever_cli::packages::execute(command, root.path(), &[])
                .unwrap_err()
                .contains("mutation is busy")
        );
        struct UnusedTransport;
        impl dever_cli::packages::PackageTransport for UnusedTransport {
            fn get(&self, _: &str, _: usize) -> Result<Vec<u8>, String> {
                panic!("busy mutation must not contact a registry")
            }
        }
        assert!(
            dever_cli::packages::execute_with(
                command,
                root.path(),
                &[],
                &UnusedTransport,
                &FixtureArtifactStore::default()
            )
            .unwrap_err()
            .contains("mutation is busy")
        );
    }
    assert_eq!(fs::read(&lock_path).unwrap(), before_lock);
    assert_eq!(fs::read(&config_path).unwrap(), before_config);
    drop(guard);
    dever_cli::libs::execute("update", root.path(), &requested).unwrap();
    assert_eq!(fs::read(&lock_path).unwrap(), before_lock);
}

#[cfg(unix)]
#[test]
fn project_mutation_rejects_linked_or_unprotected_lock_paths() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = TemporaryDirectory::new();
    let outside = TemporaryDirectory::new();
    let target = outside.path().join("untouched");
    fs::write(&target, "keep").unwrap();
    let path = root.path().join(".dever-project.lock");
    symlink(&target, &path).unwrap();
    let requested = vec!["pip:fixture@1.0.0".into()];
    assert!(
        dever_cli::libs::execute("add", root.path(), &requested)
            .unwrap_err()
            .contains("without symlinks")
    );
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    fs::remove_file(&path).unwrap();
    fs::hard_link(&target, &path).unwrap();
    assert!(
        dever_cli::libs::execute("add", root.path(), &requested)
            .unwrap_err()
            .contains("private and unlinked")
    );
    fs::remove_file(&path).unwrap();
    fs::write(&path, "").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        dever_cli::libs::execute("add", root.path(), &requested)
            .unwrap_err()
            .contains("private and unlinked")
    );
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        dever_cli::libs::execute("add", root.path(), &requested)
            .unwrap_err()
            .contains("not group/other writable")
    );
}

#[test]
fn independent_go_workers_merge_connected_offline_evidence() {
    use dever_cli::libs::{LockedWorker, resolve_workers};
    let registry = local_registry();
    let cache = FixtureArtifactStore::default();
    let resolver = local_resolver(&registry, &cache);
    let worker = |adapter: &str, spec: &str| LockedWorker {
        port: "sample.worker".into(),
        adapter: adapter.into(),
        ecosystem: "go".into(),
        runtime: Some(registry_runtime(Ecosystem::Go).pack),
        entry: format!("worker/{adapter}.go"),
        schema: "checked-port".into(),
        capabilities: vec![],
        operations: vec!["read".into()],
        libs: vec![spec.parse().unwrap()],
    };
    let lock = resolve_workers(
        &resolver,
        &[],
        vec![
            worker("first", "go:example.com/a@1.0.0"),
            worker("second", "go:example.com/b@1.2.0"),
        ],
    )
    .unwrap();
    assert_eq!(lock.go_sumdb.len(), 2);
    assert_eq!(
        lock.go_sumdb[1].previous.as_ref(),
        Some(&lock.go_sumdb[0].verify(&verifier()).unwrap().checkpoint)
    );
    let decoded = LockFile::decode(&lock.encode().unwrap()).unwrap();
    let verified = sumdb::verify_chain(&decoded.go_sumdb, &verifier()).unwrap();
    assert!(verified.contains("example.com/a", "1.0.0"));
    assert!(verified.contains("example.com/b", "1.2.0"));
    verify_locked_artifacts(&decoded, &cache, "linux-x86_64").unwrap();
}

#[test]
#[ignore = "author operation: explicitly fetches official Go proxy/sumdb; requires target/go-managed/config/setting.json and runtime.pack"]
fn prepare_official_go_dependency_fixture() {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Settings {
        runtime: RegistryRuntime,
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/go-managed");
    let settings: Settings = serde_json::from_slice(
        &fs::read(root.join("config/setting.json"))
            .expect("prepare the explicit fixture settings first"),
    )
    .unwrap();
    let pack =
        fs::read(root.join("runtime.pack")).expect("prepare the explicit Go runtime pack first");
    assert_eq!(
        settings.runtime.pack.sha256,
        digest(&pack),
        "configured runtime does not match runtime.pack"
    );
    assert_eq!(
        settings.runtime.target,
        dever_cli::toolchain::platform_identity()
    );
    let output = root.join("dependency");
    let lock_path = output.join("dever.lock");
    let previous = if lock_path.exists() {
        let lock = LockFile::decode(&fs::read(&lock_path).unwrap()).unwrap();
        dever_cli::libs::doctor(&lock).unwrap();
        let verified = sumdb::verify_chain(&lock.go_sumdb, &Verifier::official()).unwrap();
        (!verified.checkpoint.is_empty()).then_some(verified.checkpoint)
    } else {
        None
    };
    let target = settings.runtime.target.clone();
    let store = FixtureArtifactStore::default();
    let transport = dever_cli::libs::HttpRegistry::official();
    let resolver = RegistryResolver {
        build: None,
        transport: &transport,
        store: &store,
        runtimes: BTreeMap::from([(Ecosystem::Go, settings.runtime)]),
        go_sumdb: ChecksumDatabase::official(previous).unwrap(),
    };
    let lock = dever_cli::libs::LibResolver::resolve(
        &resolver,
        &["go:github.com/google/uuid@1.6.0".parse().unwrap()],
    )
    .unwrap();
    // The same official verifier and offline artifact checks protect normal builds.
    dever_cli::libs::doctor(&lock).unwrap();
    dever_cli::libs::verify_locked_artifacts(&lock, &store, &target).unwrap();
    publish_fixture_artifacts(&output, &lock, &store);
}
