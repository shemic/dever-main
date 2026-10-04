//! Explicit private fixture assets; never consult PATH or download a tool.

use std::fs;
use std::path::Path;

pub struct Asset {
    pub path: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}

pub fn assets(workspace: &Path) -> Vec<Asset> {
    let prepared = workspace.join("target/sandbox-inputs/assets");
    let mut assets = Vec::new();
    for directory in ["bin", "lib"] {
        for entry in fs::read_dir(prepared.join(directory))
            .expect("prepare target/sandbox-inputs assets explicitly")
        {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            let name = entry.file_name().into_string().unwrap();
            if directory == "bin" && name == "guard" {
                continue;
            }
            assets.push(Asset {
                path: format!("sandbox/{directory}/{name}"),
                bytes: fs::read(entry.path()).unwrap(),
                executable: directory == "bin" || name.starts_with("ld-linux"),
            });
        }
    }
    assets.push(Asset {
        path: "sandbox/bin/guard".into(),
        bytes: fs::read(workspace.join("target/debug/dever-sandbox-guard"))
            .expect("build the current dever-sandbox-guard fixture explicitly"),
        executable: true,
    });
    assets.sort_by(|left, right| left.path.cmp(&right.path));
    assets
}
