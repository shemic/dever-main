# Linux Host Cross-Compilation

## Confirmed Toolchain Facts

- LLVM is designed as a cross-compiler, but target code generation alone is insufficient. A complete build also needs target-specific headers, libraries, linker behavior and usually a sysroot: <https://clang.llvm.org/docs/CrossCompilation.html>.
- LLD supports ELF, Windows PE/COFF and macOS Mach-O. Its PE/COFF port is described as complete; Mach-O support exists but is listed after ELF and PE/COFF in completeness: <https://lld.llvm.org/>.
- Microsoft publishes the PE/COFF executable and object format specification, so a Linux-hosted backend can produce Windows artifacts without invoking a Windows compiler: <https://learn.microsoft.com/en-us/windows/win32/debug/pe-format>.
- Apple's documented universal-binary workflow requires Xcode on a Mac and uses Apple target tooling. Distributed macOS software also has Developer ID signing and notarization requirements: <https://developer.apple.com/documentation/apple-silicon/building-a-universal-macos-binary> and <https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution>.
- Apple exposes a Notary REST API, so submission need not use `notarytool`, but this does not remove SDK, linking, signing or real-device validation concerns: <https://developer.apple.com/documentation/NotaryAPI>.

## Product Consequences

### Linux to Windows

This is a practical offline target. A target-neutral Dever frontend can feed an embedded native backend, then a PE/COFF linker can combine generated code with a versioned Dever Windows runtime and required import libraries. The resulting `.exe` still needs validation on real x86_64 and ARM64 Windows systems.

### Linux to macOS

Generating x86_64 or ARM64 instructions and a Mach-O file on Linux is technically possible. Treating that file as a supported macOS product is harder: platform libraries, minimum OS versions, Mach-O signing details, Developer ID distribution, notarization and testing remain Apple-platform concerns. Redistributing an Apple SDK inside the Linux compiler also introduces a licensing boundary and must not be assumed acceptable.

For the current Text/println subset, a self-contained Dever runtime and purpose-built Mach-O path could avoid most SDK use. That shortcut does not establish a maintainable path for the planned HTTP, TLS, files, database and other system integrations.

Local compilation and public distribution are separate contracts. Once the target backend and versioned Dever macOS runtime are installed, `dever build` can remain offline. Apple Silicon requires every executable to be signed, so the linker must create an identity-free ad-hoc signature as part of offline output: <https://developer.apple.com/documentation/macos-release-notes/macos-big-sur-11_0_1-universal-apps-release-notes>. Developer ID distribution signing requires the developer's identity and an online secure timestamp: <https://developer.apple.com/documentation/technotes/tn3161-inside-code-signing-certificates>. Apple's notarization service then requires uploading the signed artifact. Apple exposes a REST API for non-macOS notarization clients, but the service is necessarily online: <https://developer.apple.com/documentation/NotaryAPI>.

## Recommended Boundary

- Keep parsing, checking, HIR and target selection shared.
- Support offline Linux-to-Windows cross-build as a normal target once the embedded backend exists.
- Keep all first-release compilation local and offline; do not transfer source or intermediate representations to a build service.
- Include macOS as an offline compilation target without introducing controlled Mac build infrastructure. Treat Developer ID signing and Apple notarization as separate optional publishing stages, and require real Intel and Apple Silicon validation for supported Mach-O output.
- Implement each target through small ABI/runtime/object-format adapters rather than duplicating compiler flows.

## Open Product Decision

Resolved: the first release uses a fully symmetric Linux, Windows and macOS x86_64/ARM64 host-to-target matrix. Six host distributions must each produce all six targets locally and offline.
