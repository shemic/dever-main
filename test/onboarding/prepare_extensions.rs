//! Opt-in actual-asset acceptance: only the download transport is local.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use dever_cli::toolchain::{
    BuildTarget, ExtensionId, ExtensionSource, Layout, MachineManager, ReleaseManifest,
    SignedResources, Version, prepare_extension_with, validate_catalog,
};
use ring::signature::Ed25519KeyPair;

struct LocalAssets(PathBuf);

impl ExtensionSource for LocalAssets {
    fn open(&self, _: &Version, extension: &ExtensionId) -> Result<Box<dyn Read>, String> {
        let name = format!(
            "dever-{}.{}",
            BuildTarget::host()?.platform(),
            extension.asset_suffix()
        );
        File::open(self.0.join(name))
            .map(|file| Box::new(file) as Box<dyn Read>)
            .map_err(|error| format!("cannot open actual extension asset: {error}"))
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if let [operation, manifest, key] = arguments.as_slice()
        && operation == "--sign-catalog"
    {
        let bytes = fs::read(manifest).map_err(|error| error.to_string())?;
        let catalog: ReleaseManifest =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        validate_catalog(&catalog)?;
        let key = fs::read(key).map_err(|error| error.to_string())?;
        let pair = Ed25519KeyPair::from_pkcs8(&key).map_err(|error| error.to_string())?;
        let signature: String = pair
            .sign(&bytes)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        return fs::write(
            Path::new(manifest).with_file_name("manifest.sig"),
            signature,
        )
        .map_err(|error| error.to_string());
    }
    let [machine, assets, version, names @ ..] = arguments.as_slice() else {
        return Err(
            "usage: prepare-extensions <machine> <assets> <version> <extension-name>...".into(),
        );
    };
    if names.is_empty() {
        return Err("at least one signed extension must be selected".into());
    }
    let layout = Layout::new(machine);
    let version = Version::parse(version)?;
    let source = LocalAssets(PathBuf::from(assets));
    if names == ["--update"] {
        let installed = MachineManager::new(layout).update_with(&source)?;
        if installed != version {
            return Err(format!(
                "expected update to {version}, received {installed}"
            ));
        }
        println!("updated {installed}");
        return Ok(());
    }
    let resources = SignedResources::load(&layout, &version)?;
    for name in names {
        let extension = resources
            .manifest()
            .extensions
            .iter()
            .find(|extension| extension.id().name() == *name)
            .ok_or_else(|| format!("signed catalog does not declare extension '{name}'"))?;
        prepare_extension_with(&layout, &version, &extension.id(), &source)?;
        println!("prepared {}", extension.id().name());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("prepare-extensions: {error}");
        std::process::exit(1);
    }
}
