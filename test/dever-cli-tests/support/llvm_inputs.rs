//! Explicit Linux author inputs shared by opt-in native fixtures.
use std::path::{Path, PathBuf};

pub const LLVM_LIBRARY: &str = "/usr/lib/llvm-18/lib/libLLVM.so.1";

pub fn core_libraries() -> Vec<(&'static str, PathBuf)> {
    let mut inputs = vec![("libLLVM.so.18.1", PathBuf::from(LLVM_LIBRARY))];
    for name in [
        "libstdc++.so.6",
        "libgcc_s.so.1",
        "libz.so.1",
        "libffi.so.8",
        "libedit.so.2",
        "libzstd.so.1",
        "libtinfo.so.6",
        "libxml2.so.2",
        "libbsd.so.0",
        "libmd.so.0",
        "libicuuc.so.74",
        "libicudata.so.74",
        "liblzma.so.5",
    ] {
        inputs.push((name, Path::new("/usr/lib/x86_64-linux-gnu").join(name)));
    }
    inputs
        .into_iter()
        .map(|(name, path)| {
            (
                name,
                path.canonicalize()
                    .expect("prepare the explicit GNU compiler-library inputs"),
            )
        })
        .collect()
}

pub fn native_inputs() -> Vec<(&'static str, PathBuf)> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    vec![
        (
            "runtime.a",
            workspace.join("target/native-runtime-abi/debug/libdever_backend_bridge.a"),
        ),
        ("crt1.o", "/usr/lib/x86_64-linux-gnu/crt1.o".into()),
        ("crti.o", "/usr/lib/x86_64-linux-gnu/crti.o".into()),
        (
            "crtbeginT.o",
            "/usr/lib/gcc/x86_64-linux-gnu/13/crtbeginT.o".into(),
        ),
        (
            "crtend.o",
            "/usr/lib/gcc/x86_64-linux-gnu/13/crtend.o".into(),
        ),
        ("crtn.o", "/usr/lib/x86_64-linux-gnu/crtn.o".into()),
        ("libc.a", "/usr/lib/x86_64-linux-gnu/libc.a".into()),
        ("libm.a", "/usr/lib/x86_64-linux-gnu/libm-2.39.a".into()),
        ("libmvec.a", "/usr/lib/x86_64-linux-gnu/libmvec.a".into()),
        (
            "libgcc.a",
            "/usr/lib/gcc/x86_64-linux-gnu/13/libgcc.a".into(),
        ),
        (
            "libgcc_eh.a",
            "/usr/lib/gcc/x86_64-linux-gnu/13/libgcc_eh.a".into(),
        ),
    ]
}
