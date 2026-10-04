fn main() {
    // The private compiler carries LLVM beside itself; project compilation must
    // not require an author shell's library-path environment.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // RPATH also resolves the packaged LLVM library's transitive imports.
        for binary in ["dever", "dever-launcher", "deverd"] {
            println!("cargo:rustc-link-arg-bin={binary}=-Wl,--disable-new-dtags");
            println!("cargo:rustc-link-arg-bin={binary}=-Wl,-rpath,$ORIGIN/lib");
        }
    }
}
