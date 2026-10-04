# Native Backend Options

## Required Properties

- Consume checked Dever HIR without changing `.dever` syntax or semantics.
- Generate x86_64 and ARM64 ELF, PE/COFF and Mach-O on every supported host.
- Run fully offline without Cargo, rustc, a target SDK or a remote build service.
- Preserve direct native calls, concrete value layouts and the existing native performance direction.
- Keep generated IR, objects and link details private, owned temporary artifacts.
- Avoid six target-specific compiler implementations.

## Options

| Option | Strengths | Blocking weaknesses | Decision |
| --- | --- | --- | --- |
| Pinned LLVM code generation plus LLD | Mature optimization; x86_64/ARM64; ELF, PE/COFF and Mach-O; supports in-process use | Large host binaries, native integration boundary, mandatory LLVM notices, target runtimes still required | Recommended |
| Cranelift plus LLD | Safe Rust-facing library, fast compilation, x86_64/ARM64 | Still needs a cross-format linker and target runtimes; optimization quality creates risk against the native performance contract | Keep as fallback only if measured LLVM integration cost is unacceptable |
| Custom machine-code and executable writers | Full control and small artifacts for the current subset | Reimplements two ISAs, three formats, relocation, unwind and linking; duplicates platform logic and does not scale to the language roadmap | Reject |
| WebAssembly or a bytecode VM inside native launchers | Simple portable execution model | Changes the AOT/runtime architecture, adds a production VM and jeopardizes native performance requirements | Reject for production build/run |
| Generated Rust plus bundled or renamed rustc | Reuses current implementation | Exposes the forbidden toolchain, produces Rust temporary source and makes offline target support depend on Rust sysroots/linkers | Replace completely |

LLVM's cross-compilation documentation still requires target-specific sysroots/libraries beyond machine-code generation: <https://clang.llvm.org/docs/CrossCompilation.html>. LLD supports the three required executable formats and can be embedded for compiler-owned trusted objects: <https://lld.llvm.org/> and <https://lld.llvm.org/NewLLD.html>.

## Proposed Ownership

```text
dever-core
  checked, target-neutral HIR and source diagnostics

dever-native
  fixed target catalog
  HIR -> LLVM lowering
  object emission
  LLD integration
  target-pack selection
  owned temporary artifacts

dever-runtime
  Dever-owned ABI and runtime behavior
  six release-built target packs

dever-cli
  --target parsing, host/default selection, output and process adaptation
```

`dever-native` is one justified private workspace boundary: it isolates the large native dependency and target mechanics from parsing/checking. It is not a public backend plugin API. Target variation uses one explicit catalog and small data/config adapters, not inheritance or a registry.

## Build Data Flow

```text
.dever -> check -> typed HIR -> specialize reachable program
       -> shared LLVM lowering -> target object in memory
       -> fixed target specification + versioned Dever runtime pack
       -> embedded LLD driver -> new executable output
```

- LLVM IR and object files are never public output formats.
- `dever build` creates a new output and keeps the current no-overwrite contract.
- `dever run` always builds the host target and runs it; cross-target execution/emulation is not added.
- A cross-target `build` only generates an artifact. Real target runners validate it in release CI.

## Target Catalog

The public catalog has exactly six initial targets:

```text
linux-x86_64
linux-arm64
windows-x86_64
windows-arm64
macos-x86_64
macos-arm64
```

Each target definition owns its LLVM triple, CPU baseline, object format, calling convention, executable suffix, runtime pack and deterministic linker settings. Shared OS and architecture helpers remove repetition; target entries remain explicit and reviewable.

## Runtime Packs

Generated code must use a Dever-owned ABI rather than Rust's unstable ABI or Rust `String` layout. Each target pack contains only Dever-owned startup/runtime objects and permitted platform import metadata. It must not contain Rust standard-library objects, Apple SDK files or target compiler binaries.

The current `dever-runtime` source is a behavior reference, not a stable native ABI. The first backend milestone defines the minimal Text slice, result/fault and `println` ABI once, then supplies six compiled implementations or target-specific adapters behind that contract. Future collections, numbers, HTTP, TLS and storage extend this ABI deliberately rather than reaching into host-language layouts.

## Determinism And Matrix Verification

- Pin one LLVM/LLD source revision and runtime-pack version for all six host builds.
- Remove timestamps, absolute build paths, random UUID inputs and host-dependent ordering.
- Each of six host compilers builds all six targets, producing 36 artifacts.
- Artifacts for the same target and source should be byte-identical across hosts. Hash equality reduces execution validation to one artifact per target; all six target artifacts still run on real matching systems.
- Offline checks deny network access and remove Cargo, rustc, Xcode, Visual Studio, GCC and target SDKs from PATH.

## Release And License Gates

- LLVM permits proprietary distribution but requires its copyright notice in binaries that include it: <https://llvm.org/docs/DeveloperPolicy.html#copyright-license-and-patents>.
- No Apple SDK file may enter a target pack. Apple's agreement prohibits separately using SDKs on non-Apple hardware and redistributing Apple Software in part: <https://www.apple.com/legal/sla/docs/xcode.pdf>.
- Before implementation commits to the macOS target, prove that both Mach-O targets can link the minimal Dever runtime without redistributed Apple SDK material and pass real-machine execution.
- The public-compiler Rust attribution decision in `distribution-licensing.md` remains a product gate.

## Rejected Shortcuts

- Do not rename or conceal rustc, Cargo, LLD or license files.
- Do not special-case the hello example or patch prebuilt executable templates.
- Do not implement one backend per OS.
- Do not upload source or HIR when local target support is missing.
- Do not silently fall back to a VM, host execution or another target.
