use super::*;
use dever_cli::workers::python_wheel::Wheel;

fn validate_native(
    target: &str,
    suffixes: &[String],
    interpreter: &str,
    files: &BTreeMap<String, &[u8]>,
) -> Result<(), String> {
    let wheel_files = files
        .keys()
        .filter(|path| !path.starts_with("runtime/") && !path.starts_with("sandbox/"))
        .cloned()
        .collect();
    dever_cli::workers::python_wheel::validate_native(
        target,
        suffixes,
        interpreter,
        files,
        &wheel_files,
    )
}

fn spec() -> LibSpec {
    "pip:sample@1.0.0".parse().unwrap()
}

#[test]
fn wheel_console_scripts_use_the_same_private_prefix_and_record_generated_files() {
    let bytes=wheel_fixture::pack("sample","1.0.0","Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        &wheel_fixture::metadata("sample","1.0.0",&[],&[]),&[("sample-1.0.0.dist-info/entry_points.txt",b"[console_scripts]\nsample-tool = sample.cli:main\n[gui_scripts]\nsample-gui = sample.gui:App.run [desktop]\n".to_vec())]);
    let installed = Wheel::parse(&spec(), None, &tags(), &bytes)
        .unwrap()
        .install("runtime/bin/python3", "3.12")
        .unwrap();
    let script = installed
        .iter()
        .find(|file| file.path == "runtime/bin/sample-tool")
        .unwrap();
    assert!(script.executable);
    assert!(
        script
            .bytes
            .starts_with(b"#!/worker/runtime/bin/python3 -I\n")
    );
    assert!(String::from_utf8_lossy(&script.bytes).contains("sample.cli"));
    let record = installed
        .iter()
        .find(|file| file.path.ends_with(".dist-info/RECORD"))
        .unwrap();
    assert!(String::from_utf8_lossy(&record.bytes).contains("../../../bin/sample-tool,sha256="));
    let collision = wheel_fixture::pack(
        "sample",
        "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        &wheel_fixture::metadata("sample", "1.0.0", &[], &[]),
        &[
            (
                "sample-1.0.0.dist-info/entry_points.txt",
                b"[console_scripts]\ntool = sample:main\n".to_vec(),
            ),
            ("sample-1.0.0.data/scripts/tool", b"#!python\n".to_vec()),
        ],
    );
    assert!(
        Wheel::parse(&spec(), None, &tags(), &collision)
            .unwrap()
            .install("runtime/bin/python3", "3.12")
            .is_err()
    );
}
fn tags() -> Vec<String> {
    vec!["py3-none-any".into()]
}
fn valid() -> Vec<u8> {
    pure_wheel("sample", "1.0.0", &[], &[])
}
fn unpack(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut file = archive.by_index(index).unwrap();
            let name = file.name().to_owned();
            let mut bytes = vec![];
            file.read_to_end(&mut bytes).unwrap();
            (name, bytes)
        })
        .collect()
}

#[test]
fn wheel_identity_and_all_compressed_filename_tags_are_bound_to_metadata() {
    Wheel::parse(
        &spec(),
        Some("sample-1.0.0-py3-none-any.whl"),
        &tags(),
        &valid(),
    )
    .unwrap();
    for filename in [
        "other-1.0.0-py3-none-any.whl",
        "sample-2.0.0-py3-none-any.whl",
        "sample-1.0.0-py2.py3-none-any.whl",
        "sample-1.0.0-1-py3-none-any.whl",
    ] {
        assert!(
            Wheel::parse(&spec(), Some(filename), &tags(), &valid()).is_err(),
            "{filename}"
        );
    }
    let compressed = wheel_fixture::pack(
        "sample",
        "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: false\nTag: py2-none-any\nTag: py3-none-any\n",
        &wheel_fixture::metadata("sample", "1.0.0", &[], &[]),
        &[],
    );
    Wheel::parse(
        &spec(),
        Some("sample-1.0.0-py2.py3-none-any.whl"),
        &tags(),
        &compressed,
    )
    .unwrap();
    assert!(
        Wheel::parse(
            &spec(),
            None,
            &["cp312-cp312-linux_x86_64".into()],
            &valid()
        )
        .is_err()
    );
    let forged_metadata = wheel_fixture::pack(
        "sample",
        "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        &wheel_fixture::metadata("other", "1.0.0", &[], &[]),
        &[],
    );
    assert!(Wheel::parse(&spec(), None, &tags(), &forged_metadata).is_err());
    let hidden_binary = wheel_fixture::pack(
        "sample",
        "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        &wheel_fixture::metadata("sample", "1.0.0", &[], &[]),
        &[("sample/native.so", elf(62, None, None))],
    );
    assert!(Wheel::parse(&spec(), None, &tags(), &hidden_binary).is_err());
}

#[test]
#[ignore = "author operation: explicitly fetches PyPI simplejson; requires target/python-managed/config/setting.json and runtime.pack"]
fn prepare_official_native_python_fixture() {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Settings {
        runtime: RegistryRuntime,
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/python-managed");
    let settings: Settings =
        serde_json::from_slice(&fs::read(root.join("config/setting.json")).unwrap()).unwrap();
    let pack = fs::read(root.join("runtime.pack")).unwrap();
    assert_eq!(digest(&pack), settings.runtime.pack.sha256);
    assert_eq!(
        settings.runtime.target,
        dever_cli::toolchain::platform_identity()
    );
    let target = settings.runtime.target.clone();
    let store = FixtureArtifactStore::default();
    let transport = dever_cli::libs::HttpRegistry::official();
    let resolver = RegistryResolver {
        build: None,
        transport: &transport,
        store: &store,
        runtimes: BTreeMap::from([(Ecosystem::Pip, settings.runtime)]),
        go_sumdb: dever_cli::libs::sumdb::ChecksumDatabase::official(None).unwrap(),
    };
    let lock = dever_cli::libs::LibResolver::resolve(
        &resolver,
        &["pip:simplejson@3.20.1".parse().unwrap()],
    )
    .unwrap();
    dever_cli::libs::doctor(&lock).unwrap();
    dever_cli::libs::verify_locked_artifacts(&lock, &store, &target).unwrap();
    let artifact = &lock.libs[0].artifacts[0];
    let wheel = store
        .verify_exact(&artifact.sha256, artifact.bytes, &target)
        .unwrap();
    assert!(
        unpack(&wheel)
            .iter()
            .any(|(path, bytes)| path.contains("_speedups.")
                && path.ends_with(".so")
                && bytes.starts_with(b"\x7fELF")),
        "fixture must use native speedups, not the optional pure-Python fallback"
    );
    publish_fixture_artifacts(&root.join("dependency"), &lock, &store);
}

#[test]
fn wheel_record_requires_exact_files_hashes_sizes_and_unique_csv_paths() {
    for mutation in 0..6 {
        let mut files = unpack(&valid());
        match mutation {
            0 => files
                .iter_mut()
                .find(|(path, _)| path.ends_with("__init__.py"))
                .unwrap()
                .1
                .push(b'!'),
            1 => files.push(("sample/unlisted.py".into(), vec![b'x'])),
            2 => {
                files.retain(|(path, _)| !path.ends_with("__init__.py"));
            }
            _ => {
                let record = &mut files
                    .iter_mut()
                    .find(|(path, _)| path.ends_with("/RECORD"))
                    .unwrap()
                    .1;
                let mut content = String::from_utf8(record.clone()).unwrap();
                match mutation {
                    3 => content = content.replace("sha256=", "sha1="),
                    4 => content.push_str("sample-1.0.0.dist-info/RECORD,,\n"),
                    5 => content.push_str("../outside.py,sha256=abc,1\n"),
                    _ => unreachable!(),
                }
                *record = content.into_bytes();
            }
        }
        assert!(
            Wheel::parse(&spec(), None, &tags(), &wheel_fixture::zip(&files)).is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn wheel_installs_all_standard_schemes_and_rewrites_installed_record() {
    let bytes = wheel_fixture::pack(
        "sample",
        "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: false\nTag: py3-none-any\n",
        &wheel_fixture::metadata("sample", "1.0.0", &[], &[]),
        &[
            ("sample-1.0.0.data/purelib/plain.py", b"plain".to_vec()),
            (
                "sample-1.0.0.data/platlib/platform.py",
                b"platform".to_vec(),
            ),
            (
                "sample-1.0.0.data/scripts/tool",
                b"#!python\nprint('ok')\n".to_vec(),
            ),
            ("sample-1.0.0.data/headers/public.h", b"header".to_vec()),
            (
                "sample-1.0.0.data/data/share/sample/schema.json",
                b"{}".to_vec(),
            ),
            ("sample/name,with\"quotes.py", b"quoted".to_vec()),
        ],
    );
    let files = Wheel::parse(&spec(), None, &tags(), &bytes)
        .unwrap()
        .install("runtime/bin/python3", "3.12")
        .unwrap();
    for path in [
        "runtime/lib/python3.12/site-packages/plain.py",
        "runtime/lib/python3.12/site-packages/platform.py",
        "runtime/include/python3.12/sample/public.h",
        "runtime/share/sample/schema.json",
        "runtime/lib/python3.12/site-packages/sample/name,with\"quotes.py",
    ] {
        assert!(files.iter().any(|file| file.path == path), "{path}");
    }
    let script = files
        .iter()
        .find(|file| file.path == "runtime/bin/tool")
        .unwrap();
    assert!(script.executable);
    assert!(
        script
            .bytes
            .starts_with(b"#!/worker/runtime/bin/python3 -I\n")
    );
    let record = files
        .iter()
        .find(|file| file.path.ends_with("/RECORD"))
        .unwrap();
    let rows = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(record.bytes.as_slice())
        .records()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let installed_script = rows
        .iter()
        .find(|row| &row[0] == "../../../bin/tool")
        .unwrap();
    assert_eq!(
        installed_script[1].to_owned(),
        format!(
            "sha256={}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(&script.bytes))
        )
    );
}

#[test]
fn wheel_rejects_scheme_collisions_and_uses_artifact_dependencies() {
    let bytes = wheel_fixture::pack(
        "sample",
        "1.0.0",
        "Wheel-Version: 1.0\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        &wheel_fixture::metadata("sample", "1.0.0", &[], &[]),
        &[
            ("same.py", vec![b'a']),
            ("sample-1.0.0.data/platlib/same.py", vec![b'b']),
        ],
    );
    assert!(
        Wheel::parse(&spec(), None, &tags(), &bytes)
            .unwrap()
            .install("runtime/bin/python3", "3.12")
            .is_err()
    );
    let mut registry = local_registry();
    let release = registry
        .0
        .get_mut(&(Ecosystem::Pip, "/pypi/a/1.0.0/json".into()))
        .unwrap();
    let mut document: serde_json::Value = serde_json::from_slice(release).unwrap();
    document["info"]["requires_dist"] = serde_json::json!([]);
    *release = serde_json::to_vec(&document).unwrap();
    let lock = python_lock(&registry, &["pip:a@1.0.0"]).unwrap();
    assert_eq!(dependency_keys(&lock, "pip:a@1.0.0"), ["pip:b@1.1.0"]);
}

#[test]
fn solver_backtracks_on_selected_wheel_metadata_and_its_actual_extras() {
    let mut registry = LocalRegistry(BTreeMap::new());
    python_release(
        &mut registry,
        "root",
        "1.0.0",
        &[],
        &["a[x] >=1", "b ==1.0.0"],
    );
    python_release(
        &mut registry,
        "a",
        "2.0.0",
        &["x"],
        &["b ==2.0.0; extra == 'x'"],
    );
    python_release(
        &mut registry,
        "a",
        "1.0.0",
        &["x"],
        &["b ==1.0.0; extra == 'x'"],
    );
    python_release(&mut registry, "b", "2.0.0", &[], &[]);
    python_release(&mut registry, "b", "1.0.0", &[], &[]);
    for version in ["1.0.0", "2.0.0"] {
        let json = registry
            .0
            .get_mut(&(Ecosystem::Pip, format!("/pypi/a/{version}/json")))
            .unwrap();
        let mut release: serde_json::Value = serde_json::from_slice(json).unwrap();
        release["info"] = serde_json::json!({"requires_dist":null,"provides_extra":null});
        *json = serde_json::to_vec(&release).unwrap();
    }
    let lock = python_lock(&registry, &["pip:root@1.0.0"]).unwrap();
    assert!(
        lock.libs
            .iter()
            .any(|lib| lib.spec.key() == "pip:a[x]@1.0.0")
    );
    assert!(!lock.libs.iter().any(|lib| lib.spec.version == "2.0.0"));
    assert_eq!(dependency_keys(&lock, "pip:a[x]@1.0.0"), ["pip:b@1.0.0"]);
}

/// ELF64 protocol fixture with PT_LOAD and optional PT_DYNAMIC/DT_STRTAB.
/// It is parsed only; no executable bytes are launched by this test.
fn elf(machine: u16, needed: Option<&str>, rpath: Option<&str>) -> Vec<u8> {
    let mut bytes = vec![0u8; 1024];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&2u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
    bytes[96..104].copy_from_slice(&1024u64.to_le_bytes());
    bytes[104..112].copy_from_slice(&1024u64.to_le_bytes());
    bytes[120..124].copy_from_slice(&2u32.to_le_bytes());
    bytes[128..136].copy_from_slice(&176u64.to_le_bytes());
    bytes[136..144].copy_from_slice(&176u64.to_le_bytes());
    let mut strings = vec![0];
    let mut dynamic = vec![];
    for (tag, value) in [(1u64, needed), (15u64, rpath)] {
        if let Some(value) = value {
            dynamic.push((tag, strings.len() as u64));
            strings.extend(value.bytes());
            strings.push(0);
        }
    }
    dynamic.extend([(5, 512), (10, strings.len() as u64), (0, 0)]);
    bytes[152..160].copy_from_slice(&((dynamic.len() * 16) as u64).to_le_bytes());
    bytes[160..168].copy_from_slice(&((dynamic.len() * 16) as u64).to_le_bytes());
    for (index, (tag, value)) in dynamic.into_iter().enumerate() {
        bytes[176 + index * 16..184 + index * 16].copy_from_slice(&tag.to_le_bytes());
        bytes[184 + index * 16..192 + index * 16].copy_from_slice(&value.to_le_bytes());
    }
    bytes[512..512 + strings.len()].copy_from_slice(&strings);
    bytes
}

#[test]
fn native_wheel_checks_target_suffix_and_closed_library_search() {
    let runtime = elf(62, None, None);
    let library = elf(62, None, None);
    let native = elf(62, Some("libsample.so.1"), Some("$ORIGIN/../sample.libs"));
    let suffixes = vec![
        ".cpython-312-x86_64-linux-gnu.so".into(),
        ".abi3.so".into(),
        ".so".into(),
    ];
    let mut files = BTreeMap::from([
        ("runtime/bin/python3".into(), runtime.as_slice()),
        (
            "site-packages/sample/_native.cpython-312-x86_64-linux-gnu.so".into(),
            native.as_slice(),
        ),
        (
            "site-packages/sample.libs/libsample.so.1".into(),
            library.as_slice(),
        ),
    ]);
    validate_native("linux-x86_64", &suffixes, "runtime/bin/python3", &files).unwrap();
    files.remove("site-packages/sample.libs/libsample.so.1");
    assert!(
        validate_native("linux-x86_64", &suffixes, "runtime/bin/python3", &files)
            .unwrap_err()
            .contains("packaged dependency closure")
    );
    files.insert("sandbox/lib/libsample.so.1".into(), library.as_slice());
    validate_native("linux-x86_64", &suffixes, "runtime/bin/python3", &files).unwrap();
    let old = files
        .remove("site-packages/sample/_native.cpython-312-x86_64-linux-gnu.so")
        .unwrap();
    files.insert(
        "site-packages/sample/_native.cpython-311-x86_64-linux-gnu.so".into(),
        old,
    );
    assert!(
        validate_native("linux-x86_64", &suffixes, "runtime/bin/python3", &files)
            .unwrap_err()
            .contains("suffix")
    );
    for (machine, rpath, message) in [
        (183, "$ORIGIN", "target"),
        (62, "/usr/lib", "$ORIGIN"),
        (62, "$ORIGIN/../../../..", "escapes"),
    ] {
        let rejected = elf(machine, None, Some(rpath));
        let files = BTreeMap::from([
            ("runtime/bin/python3".into(), runtime.as_slice()),
            ("site-packages/sample/native.so".into(), rejected.as_slice()),
        ]);
        assert!(
            validate_native("linux-x86_64", &suffixes, "runtime/bin/python3", &files)
                .unwrap_err()
                .contains(message)
        );
    }
}

#[test]
fn native_runpath_outranks_ancestor_rpath_and_helpers_keep_import_context() {
    let runtime = elf(62, None, Some("$ORIGIN/../lib"));
    let shared = elf(62, None, None);
    let wrong_target = elf(183, None, None);
    let mut extension = elf(
        62,
        Some("libsample.so.1"),
        Some("$ORIGIN/../../runtime/alternate"),
    );
    // Replace DT_RPATH with DT_RUNPATH in the independently encoded ELF vector.
    for offset in (176..256).step_by(16) {
        if u64::from_le_bytes(extension[offset..offset + 8].try_into().unwrap()) == 15 {
            extension[offset..offset + 8].copy_from_slice(&29u64.to_le_bytes());
        }
    }
    let suffixes = vec![".cpython-312-x86_64-linux-gnu.so".into()];
    let files = BTreeMap::from([
        ("runtime/bin/python3".into(), runtime.as_slice()),
        ("runtime/lib/libsample.so.1".into(), shared.as_slice()),
        (
            "runtime/alternate/libsample.so.1".into(),
            wrong_target.as_slice(),
        ),
        (
            "site-packages/pkg/mod.cpython-312-x86_64-linux-gnu.so".into(),
            extension.as_slice(),
        ),
    ]);
    assert!(
        validate_native("linux-x86_64", &suffixes, "runtime/bin/python3", &files)
            .unwrap_err()
            .contains("target")
    );

    let extension = elf(62, Some("liba.so.1"), Some("$ORIGIN/../vendor"));
    let helper = elf(62, Some("libb.so.1"), None);
    let files = BTreeMap::from([
        ("runtime/bin/python3".into(), runtime.as_slice()),
        (
            "site-packages/pkg/mod.cpython-312-x86_64-linux-gnu.so".into(),
            extension.as_slice(),
        ),
        ("site-packages/vendor/liba.so.1".into(), helper.as_slice()),
        ("site-packages/vendor/libb.so.1".into(), shared.as_slice()),
    ]);
    validate_native("linux-x86_64", &suffixes, "runtime/bin/python3", &files).unwrap();
}
