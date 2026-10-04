use super::*;

#[test]
#[ignore = "requires the explicit official native npm build fixture at target/npm-build"]
fn runs_official_npm_addon_from_offline_resources_and_threads() {
    let temp = TemporaryDirectory::new();
    let program = dever_core::check(&worker_sources(
        "npm",
        "worker/main.cjs",
        "lib \"bufferutil@4.0.9\"",
    ))
    .unwrap();
    let contract = program.external_worker_contracts().remove(0);
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let prepared = workspace.join("target/npm-build");
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(prepared.join("config/setting.json")).unwrap()).unwrap();
    let pack = fs::read(prepared.join(config["runtime_file"].as_str().unwrap())).unwrap();
    let mut lock =
        LockFile::decode(&fs::read(prepared.join("dependency/dever.lock")).unwrap()).unwrap();
    let runtime = lock.libs[0].runtime.clone();
    assert_eq!(sha(&pack), runtime.sha256);
    let rejected = lock.builds[0]
        .native_rejections
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        !rejected.is_empty(),
        "official multi-platform prebuilds must be classified"
    );
    let entry = temp.path().join(&contract.entry);
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    let adapter = r#"const addon = require('bufferutil/build/Release/bufferutil.node');
const {Worker} = require('node:worker_threads');
const path = require('node:path');
const rejected = __REJECTED__.map(name => path.join(__dirname, '..', name));
exports.send = async payload => {
  await new Promise((resolve, reject) => {
    const worker = new Worker(`const {parentPort,workerData}=require('node:worker_threads'); const fs=require('node:fs'); parentPort.postMessage(workerData.every(path=>!fs.existsSync(path)));`, {eval:true,workerData:rejected});
    worker.once('message', ok => ok ? resolve() : reject(new Error('unverified addon was installed')));
    worker.once('error', reject);
  });
  const input=Buffer.from([payload.value]), output=Buffer.alloc(1), mask=Buffer.from([1,2,3,4]);
  addon.mask(input,mask,output,0,1); addon.unmask(output,mask);
  return {receipt:output[0]+1};
};
"#.replace("__REJECTED__",&serde_json::to_string(&rejected).unwrap());
    fs::write(&entry, adapter).unwrap();
    let mut resources = vec![resource(
        format!(
            "lib/runtime/npm/{}/runtime.pack",
            dever_cli::toolchain::platform_identity()
        ),
        pack,
    )];
    for library in &mut lock.libs {
        library.schema = contract.schema.clone();
    }
    for artifact in lock
        .libs
        .iter()
        .flat_map(|lib| &lib.artifacts)
        .chain(lock.builds.iter().map(|receipt| &receipt.output))
    {
        let bytes = fs::read(prepared.join("dependency/artifacts").join(&artifact.sha256)).unwrap();
        assert_eq!(sha(&bytes), artifact.sha256);
        resources.push(resource(artifact.path.clone(), bytes));
    }
    for asset in sandbox::assets(&workspace) {
        let mut file = resource(asset.path, asset.bytes);
        file.executable = asset.executable;
        resources.push(file);
    }
    lock.workers.push(LockedWorker {
        port: contract.port.clone(),
        adapter: contract.adapter.clone(),
        ecosystem: "npm".into(),
        runtime: Some(runtime),
        entry: contract.entry.clone(),
        schema: contract.schema.clone(),
        capabilities: contract.capabilities.clone(),
        operations: contract.operations.clone(),
        libs: contract
            .libs
            .iter()
            .map(|spec| spec.parse().unwrap())
            .collect(),
    });
    lock.write_atomic(temp.path()).unwrap();
    let mut generated = workers::prepare(temp.path(), &program, &resources).unwrap();
    let again = workers::prepare(temp.path(), &program, &resources).unwrap();
    assert_eq!(
        generated
            .iter()
            .map(|file| (&file.path, &file.sha256))
            .collect::<Vec<_>>(),
        again
            .iter()
            .map(|file| (&file.path, &file.sha256))
            .collect::<Vec<_>>()
    );
    assert!(
        generated
            .iter()
            .any(|file| file.path.ends_with("/build/Release/bufferutil.node")
                && file.bytes.starts_with(b"\x7fELF"))
    );
    for rejected in &rejected {
        assert!(!generated.iter().any(|file| file.path.ends_with(rejected)));
    }
    generated.extend(
        resources
            .into_iter()
            .filter(|file| file.path.starts_with("sandbox/")),
    );
    fs::remove_file(entry).unwrap();
    run_packaged_protocol(&temp, &contract, "npm", &generated);
}
