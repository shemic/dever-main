fn main() {
    #[cfg(feature = "embedded")]
    build();
}

#[cfg(feature = "embedded")]
fn build() {
    use std::path::PathBuf;

    // Private authoring SDK, never a runtime PATH lookup or app setting.
    // Release builders prepare these headers/libraries before enabling embedded.
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/backend-sdk/usr/lib/llvm-18");
    let include = root.join("include");
    let lib = root.join("lib");
    for input in [include.join("lld/Common/Driver.h"), lib.join("liblldELF.a")] {
        assert!(
            input.is_file(),
            "private LLVM 18 SDK is missing: {}",
            input.display()
        );
    }
    println!("cargo:rerun-if-changed=src/bridge.cpp");
    println!("cargo:rerun-if-changed={}", root.display());
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        // Upstream headers are system headers; warnings remain enabled for ours.
        .flag("-isystem")
        .flag(include.to_str().expect("SDK include path must be UTF-8"))
        .file("src/bridge.cpp")
        .flag_if_supported("-fno-rtti")
        .compile("dever_backend_bridge");
    println!("cargo:rustc-link-search=native={}", lib.display());
    let multiarch = match std::env::var("TARGET").as_deref() {
        Ok("x86_64-unknown-linux-gnu") => "x86_64-linux-gnu",
        Ok("aarch64-unknown-linux-gnu") => "aarch64-linux-gnu",
        _ => panic!("the embedded author SDK currently requires a Linux GNU target"),
    };
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("..").join(multiarch).display()
    );
    for name in ["lldELF", "lldCOFF", "lldMachO", "lldCommon"] {
        println!("cargo:rustc-link-lib=static={name}");
    }
    println!("cargo:rustc-link-lib=dylib=LLVM-18");
    println!("cargo:rustc-link-lib=dylib=z");
    println!("cargo:rustc-link-lib=static=zstd");
}
