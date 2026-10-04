# Native release authoring

This is the private compiler-author workflow. Application projects continue to use `dever run/build`; they do not need Cargo, a signing key or author settings. A Linux x86_64 compiler release can contain both native x86_64 and cross-compiled ARM64 application packs. The compiler and its LLVM libraries remain host binaries.

Build the four runtime archives separately, then assemble and sign a new directory. The maker never builds source, downloads dependencies, searches `PATH`, reads configuration from environment variables, or modifies its inputs. It reuses the runtime manifest and signed release formats consumed by `deverd` and `MachineManager`.

## Build inputs

Build the compiler with `cargo build --offline --locked -p dever-cli --bin dever`; its author input becomes the installed `dever-core`. Build the stable machine launcher with `--bin dever-launcher` and the daemon with `--bin deverd`. Use `target/debug/dever-launcher` for the bootstrap launcher input, which the maker publishes as `bootstrap/dever`. The installed public command remains `dever`; `dever-launcher` is only an author build name. Preserve this distinction when preparing release inputs.

Prepare the pinned Rust dependencies and compiler LLVM 18/LLD SDK explicitly. Use an explicitly selected Cargo/Rust toolchain for author builds. The root `runtime-pack` Cargo profile inherits the release optimization settings, uses one codegen unit and removes debug information while retaining archive symbols needed by LLD. It must not enable the bridge's `embedded` feature.

All profiles include `runtime-api,runtime-external`; their dependencies enable `runtime-abi`. Database drivers vary:

| Profile | Additional features | Saved archive |
| --- | --- | --- |
| base | none | `base.a` |
| sqlite | `runtime-sqlite` | `sqlite.a` |
| postgres | `runtime-postgres` | `postgres.a` |
| both | `runtime-sqlite,runtime-postgres` | `both.a` |

For example, build base for Linux x86_64:

```sh
cargo build --offline --locked -p dever-backend-bridge --lib \
  --profile runtime-pack --no-default-features \
  --features runtime-api,runtime-external \
  --target x86_64-unknown-linux-gnu --target-dir target/native-runtime-abi
```

Copy `target/native-runtime-abi/x86_64-unknown-linux-gnu/runtime-pack/libdever_backend_bridge.a` to a new `base.a` input, then repeat with each row's features and save its archive before the next build. Share this one Cargo target directory; do not create four dependency caches. Do not copy the both archive into all four inputs or strip a hard-linked fixture in place. Do not use `strip --strip-all` on link inputs.

Prepare a matching compiler core, `libLLVM.so.18.1`, target CRT objects and genuine static libraries. Linux GNU link order is explicit: start objects, runtime, static libraries, end objects. Linker scripts and thin archives are rejected. Preserve the compiler version and the exact compiler/CRT/SDK versions and input hashes in build records; the maker does not infer feature content or compiler versions from filenames.

## Author settings

Application authors select `dever build <project-root> --target linux-aarch64 --output <new-file>`; omitted targets, `run`, and `test` use the host. This selects application output, not an ARM compiler installation. Missing target packs are errors. Product commands do not discover cross tools or download packs while building.

For an additional application target, add `native_targets.linux-aarch64` to the author settings. Its value has the same `profiles`, `start`, `libraries`, and `end` fields as the host inputs below. Build all four archives with Rust target `aarch64-unknown-linux-gnu` and an explicitly prepared cross linker/sysroot. Use ARM CRT and static libraries, never host files or linker scripts. Both standalone object and archive member ELF headers are checked against the declared target before publishing and on cache access.

The parallel maps `ecosystem_targets.linux-aarch64`, `sandbox_targets.linux-aarch64`, and `build_targets.linux-aarch64` use the corresponding host `runtimes`, `sandbox`, and `builds` shapes. Python/Node interpreters, native wheel/addon files, the sandbox guard, static bwrap, and its OS libraries must match the application target. Go is different: `build.host` identifies the host executable tools; `build.goos` and `build.goarch` identify the output and standard-library archives. The compiler/linker must produce that target with a cleared environment. The private fixture maker supports `go run test/go-managed/make_pack.go <go-binary> <new-pack> linux-aarch64`; it uses an author-only Go overlay to bake ARM defaults into x86 tools without changing the installed Go SDK.

Prepare project Libs explicitly with `dever lib update <project-root> --target linux-aarch64` (or `lib add` with the same option). The resulting lock and cached resources bind the selected target. Offline `build` verifies that binding. Pure Python wheels/npm packages and Go cross compilation use this path; Python source distributions or npm lifecycle builds require already prepared matching target outputs and complete build receipts. An x86 tool must never silently produce a native extension labeled ARM.

The opt-in ARM acceptance uses explicit inputs under `target/arm64-cross/`: four archives/CRT libraries, target ecosystem and sandbox packs, locked upstream dependencies, and private QEMU/Alpine kernel/initramfs files. `native_release::acceptance::cross` exports independent applications under `guest/apps/` after deleting its source and machine store. Run `python3 -B test/ecosystem-release/arm64/simulate.py` to rebuild only the owned guest images and execute Python, Node, Go and HTTP under the complete ARM kernel. It requires all success/failure/cleanup markers and writes serial logs plus a result JSON under `logs/`. The VM has no external network or system language installations. Its initramfs switches into a normal root mount so bubblewrap can use `pivot_root`. This is functional emulation, not physical ARM performance or a universal kernel compatibility result.

Use a separate author root, not a Dever application's configuration. Place its explicit input files and private Ed25519 PKCS#8 key below that root. `config/setting.json` has this shape; replace every illustrative digest with the file's actual lowercase SHA-256:

```json
{
  "format": "dever-native-release-input-v1",
  "version": "0.1.0",
  "target": "x86_64-unknown-linux-gnu",
  "signing_key": "private/release.pk8",
  "core": {"source": "inputs/dever-core", "sha256": "<sha256>"},
  "skill": [
    {"source": "inputs/skills/SKILL.md", "path": "SKILL.md", "sha256": "<sha256>"},
    {"source": "inputs/skills/references/development.md", "path": "references/development.md", "sha256": "<sha256>"}
  ],
  "core_libraries": [
    {"source": "inputs/libLLVM.so.18.1", "path": "libLLVM.so.18.1", "sha256": "<sha256>"},
    {"source": "inputs/libstdc++.so.6", "path": "libstdc++.so.6", "sha256": "<sha256>"}
  ],
  "profiles": {
    "base": {"source": "inputs/base.a", "sha256": "<sha256>"},
    "sqlite": {"source": "inputs/sqlite.a", "sha256": "<sha256>"},
    "postgres": {"source": "inputs/postgres.a", "sha256": "<sha256>"},
    "both": {"source": "inputs/both.a", "sha256": "<sha256>"}
  },
  "start": [
    {"source": "inputs/crt1.o", "path": "crt1.o", "sha256": "<sha256>"},
    {"source": "inputs/crti.o", "path": "crti.o", "sha256": "<sha256>"},
    {"source": "inputs/crtbeginT.o", "path": "crtbeginT.o", "sha256": "<sha256>"}
  ],
  "libraries": [
    {"source": "inputs/libc.a", "path": "libc.a", "sha256": "<sha256>"},
    {"source": "inputs/libm.a", "path": "libm.a", "sha256": "<sha256>"},
    {"source": "inputs/libmvec.a", "path": "libmvec.a", "sha256": "<sha256>"},
    {"source": "inputs/libgcc.a", "path": "libgcc.a", "sha256": "<sha256>"},
    {"source": "inputs/libgcc_eh.a", "path": "libgcc_eh.a", "sha256": "<sha256>"}
  ],
  "end": [
    {"source": "inputs/crtend.o", "path": "crtend.o", "sha256": "<sha256>"},
    {"source": "inputs/crtn.o", "path": "crtn.o", "sha256": "<sha256>"}
  ]
}
```

`source` and `signing_key` are normalized relative paths below the author root, without symlinks. Link-input `path` is relative to the output's `runtime/<platform>` directory; reserved and duplicate destinations fail. The key must be owner-private on Unix. Unknown fields, missing profiles, invalid targets or digests fail explicitly. The key and author configuration are never included in the output.

`core_libraries` must list the full dynamic dependency closure, not just the two illustrative entries above. These `path` values are plain SONAMEs published under `lib/`. Current Linux x86_64 inputs additionally require gcc_s, z, ffi, edit, zstd, tinfo, xml2, bsd, md, icuuc, icudata and lzma. The compiler declares inherited `RPATH=$ORIGIN/lib`; library search entries may only name that same private directory (`$ORIGIN` or upstream LLVM's `$ORIGIN/../lib`). Maker and signed native installation reject missing dependencies, mismatched SONAME/architecture, unused libraries and escaping search paths. The declared system baseline is GNU libc and its companion libraries; the current prepared compiler requires glibc 2.39. The pack must not replace that system ABI. This replaces the single `llvm` author field directly.

## Versioned AI skill and public release assets

`skill` is required. Copy the complete standalone `dever-main-skills` checkout into the author inputs, excluding `.git` and temporary files, and list every skill file as `{source,path,sha256}`. Paths are relative to its skill root; the maker publishes them under `skills/dever-language/` in the signed manifest. The two entries above illustrate the shape, not the complete file list. `SKILL.md` is mandatory; at most 256 files, 1 MiB per file and 8 MiB total are allowed.

`dever skill path [project-root]` validates and returns the skill belonging to the selected compiler. `dever skill install <new-directory>` installs a small AI entry that resolves the active signed skill on each task. It refuses existing directories. `dever update` and `dever use` change the one active-version journal used by both core and skill; they do not rewrite every user's AI directory. Ordinary users can read the signed skill without the install lock or administrator permissions.

Publish official versions to `shemic/dever-main` GitHub Releases with tag `v<version>`. `sdk/release-assets.py` converts the signed directory into three assets named `dever-<platform>.manifest.json`, `dever-<platform>.manifest.sig`, and `dever-<platform>.tar.gz`. The archive contains exactly the manifest's ordinary artifact files, with no directory headers or embedded manifest. The installer and explicit `dever install/update` download these assets, verify the independent pinned public key before payload extraction, and never use application configuration or environment variables for the official address. Project `run/build` remain offline.

## Linux first installation

Include the stable launcher and daemon in the same signed release by adding this author field:

```json
"bootstrap": {
  "launcher": {"source":"inputs/dever", "sha256":"<sha256>"},
  "daemon": {"source":"inputs/deverd", "sha256":"<sha256>"},
  "libraries": [
    {"source":"inputs/libgcc_s.so.1", "path":"libgcc_s.so.1", "sha256":"<sha256>"}
  ]
}
```

Both Rust entry points must be built with their private `$ORIGIN/lib` RPATH. The maker verifies their separate ELF dependency closure and adds `bootstrap/dever`, `bootstrap/deverd`, `bootstrap/lib/*` and the fixed `bootstrap/deverd.service` to the existing signed manifest. Core-only releases may omit this field; first installation requires it.

An administrator must independently authenticate the first installer executable, its private libraries, and the release public key. A key shipped inside a downloaded release is not a trust source. Place the following configuration under an absolute, root-owned installer directory, in `config/setting.json`:

```json
{
  "release": "/prepared/release",
  "system_root": "/owned/system-image",
  "trusted_key": "/independently-provisioned/release.pub",
  "activate_service": false
}
```

Run the independently trusted launcher with `--dever-install /absolute/installer-root`. It installs the signed compiler version, `/opt/dever/bin` bootstrap closure, `/usr/local/bin/dever` symlink, root-owned systemd unit and its enable link below `system_root`. The root daemon owns the shared machine store; ordinary projects and users share it through the existing authenticated service protocol. `install`, `update` and `use` retain their version-management contracts.

Offline image installation never invokes systemctl. `activate_service: true` is accepted only for the real `/` system root and explicitly performs daemon reload, restart and readiness checking. This session's acceptance uses owned image roots and manually started owned daemons; it does not activate any global service.

The initial key is written to a checked temporary file, synced, then published without replacing an existing key. A bounded journal records seven fixed destinations: bootstrap directory, sandbox helper store, AppArmor policy file, service unit, public entry, enable link and active version. File modes and child directories are persisted before publication. Interrupted uncommitted installs restore their recorded predecessors; committed installs finish cleanup without undoing publication. Rerun the explicit installer to recover. Normal launcher/version operations refuse an unfinished journal. A cleanup failure remains an explicit error with the journal retained. Existing service/policy ownership is checked against its previously signed template, allowing legitimate template upgrades while rejecting unrelated files. These ordering guarantees and interruption tests are not a claim of hardware power-loss testing.

## Ecosystem inputs

The same author configuration accepts an optional `runtimes` object with `pip`, `npm` and `go` entries. Each entry declares `name`, `version` and `files`: normalized archive `path`, author-root `source`, and exact `sha256`. Include `dever-runtime.json` in that list. Python additionally requires `python_markers` and nonempty `python_wheel_tags` matching the prepared interpreter; these are registry resolution metadata, not settings supplied by an application.

Python packs must contain a relocatable interpreter, its standard library, native extensions and their non-OS dependencies, with fixed `-I -S -B` arguments. Node packs include the interpreter with its embedded standard library. Go build packs contain `compile`, `link`, the Dever source analyzer and every standard-library archive referenced by their `importcfg`; applications embed only the resulting Go Worker. Interpreter portability must be tested against an explicit OS baseline. A copied host executable alone does not establish a complete pack.

The maker and Worker builder share descriptor, path and file-closure validation. Packs are deterministic gzip/tar files with sorted entries, fixed archive metadata and executable modes derived from the descriptor. Every runtime input is copied and rehashed; the private-key exclusion applies to these files too. The maker calculates the pack digest and emits `runtime/<ecosystem>/<platform>/{runtime.pack,manifest.json}`. Both files enter the existing top-level release signature. No new lock, installation or signature protocol is introduced.

Node registry metadata may declare `npm_libc` as `glibc` or `musl`, matching the explicitly prepared interpreter and target. This is signed author metadata for package eligibility, never host probing. The pinned GNU Node fixture declares `glibc`.

## Sandbox inputs

External Workers, including `exec`, require the installed release's sandbox assets. Add a `sandbox` array to author settings using the same `{source, path, sha256}` entries as other leaf inputs. Its paths are `bin/bwrap`, `bin/guard`, and flat `lib/<name>` libraries, including the platform loader. Build `dever-sandbox-guard` from the current source with the explicit author toolchain. The bwrap input must support bind-fd and be a static target ELF with neither PT_INTERP nor DT_NEEDED; runtime executes it directly. The guard and language runtimes retain their complete private loader/library closure. The maker validates every asset before publishing `sandbox/<platform>/<path>` under the existing release signature. The asset budget is 128 files / 32 MiB. Inputs must not use RPATH/RUNPATH to escape the supplied bootstrap library set.

The Linux backend requires user, mount, PID and network namespace permission, seccomp and `openat2`. An active host loader preload configuration is rejected. When `apparmor_restrict_unprivileged_userns=1`, a non-root launch selects only `/opt/dever/sandbox/<embedded-bwrap-sha256>/bwrap`. Every ancestor must be a real root-owned directory without group/other write permission; the regular executable must have no setuid/setgid or file capabilities, and its digest must equal the embedded verified asset. Missing or invalid helpers produce an installation error before spawning. Other launches execute the same static embedded bwrap directly; there is no PATH, environment, dynamic-loader or system-bwrap fallback.

The signed bootstrap includes `bootstrap/dever-sandbox`, copied to `/etc/apparmor.d/dever-sandbox` by the same installation transaction. The source is [sdk/apparmor/dever-sandbox](apparmor/dever-sandbox), derived from the [AppArmor 4.0 bwrap restriction profile](https://gitlab.com/apparmor/apparmor/-/raw/apparmor-4.0/profiles/apparmor/profiles/extras/bwrap-userns-restrict). Its attachment permits only 64-lowercase-hex directories below the trusted store. bwrap creates namespaces under the parent policy and exec stacks the capability-denying child policy, including under no-new-privileges. Attachment paths do not restrict an explicit profile transition from an unconfined process: on hosts with user namespace restriction enabled, `kernel.apparmor_restrict_unprivileged_unconfined=1` is therefore mandatory before installation or loading. See [Ubuntu's explanation of strict profile changes](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007). Non-root launches check this flag before selecting a helper; a missing, disabled or unreadable flag fails closed. The real-host installer checks the same prerequisite before any installation changes and again before publishing the policy file, because an independent system policy reload could load even an initially inactive file. Offline image preparation does not change or require the author's host settings; the deployed image must satisfy this prerequisite before its policy becomes loadable.

The runtime also requires both named profiles in enforce mode using the read-only AppArmor policy filesystem; absent or inaccessible policy metadata is an explicit deployment error. No launch changes sysctl or loads policy. A container or restricted mount namespace must expose this policy metadata if it runs such non-root Workers. Hosts without the user namespace restriction do not require the strict-profile sysctl to exist.

Administrator order is explicit: first review and approve the host-wide effect of strict profile transitions, record the original sysctl value, and establish `kernel.apparmor_restrict_unprivileged_unconfined=1`; then install the matching signed helper/policy; only then load with `apparmor_parser -r -K /etc/apparmor.d/dever-sandbox`. Dever never changes this global setting automatically. Review and offline validation use `apparmor_parser -Q -K -j 1 sdk/apparmor/dever-sandbox`, which does not load policy or write its cache. First-install rollback must unload this exact policy with `apparmor_parser -R -K /etc/apparmor.d/dever-sandbox` and remove the newly installed policy file from reload discovery before restoring the original strict-profile sysctl value. For an upgrade, restore the previously approved policy instead, retaining strict mode for any policy that requires it. Keep all helpers needed by deployed applications. Each host operation requires authorization; never disable the user namespace restriction. Real non-root, NNP/aa-exec (including immediate mode) and capability-denial acceptance must follow loading; offline parsing and root-only acceptance do not establish those results.

The installer preserves at most 24 helper identities, each with its original signed manifest/signature, independently of compiler version directories. Unknown entries, incorrect signatures and digest mismatches fail without overwriting that store. The existing transaction's 128-entry/80-MiB tree budget also applies. Retained hashes support independently deployed applications after source or compiler-version removal. A new compiler version selecting different bwrap bytes requires the explicit trusted bootstrap installation before affected non-root Workers can start. Other OS backends and GPU device isolation are not currently supported.

For approved profile-transition acceptance, `test/dever-sandbox-tests/apparmor_transition.c` is a standalone negative probe: compile it statically with the explicit author C toolchain, then invoke it as a nonzero UID through `aa-exec -p dever-bwrap`, immediate `aa-exec -i -p dever-bwrap`, and no-new-privileges variants. It never executes another program; it attempts only its own user/network namespaces, UID/GID maps and loopback capability operation. Record aa-exec rejection separately from a probe result. Exit 0 reports a denied operation, exit 1 reports unsafe access and exit 2 means unavailable setup. Creating the network namespace already proves a capability check succeeded, so the probe returns 1 even if a later loopback operation fails. Preserve the initial label and exact denial stage for each case.

### Static Linux x86_64 author input

The current private fixture uses upstream [bubblewrap v0.11.0](https://github.com/containers/bubblewrap/tree/v0.11.0), SHA-256 `cfeeb15fcc47d177d195f06fdf0847e93ee3aa6bf46f6ac0a141fa142759e2c3` for `https://codeload.github.com/containers/bubblewrap/tar.gz/refs/tags/v0.11.0`, and Ubuntu Noble's `libcap-dev_2.66-5ubuntu2.4_amd64.deb`, SHA-256 `07f2462867569a2119a2ad0f1593232663f2d1612b791c230d22a8d73a15abee` from the authenticated apt package index. Its download URL is `https://archive.ubuntu.com/ubuntu/pool/main/libc/libcap2/libcap-dev_2.66-5ubuntu2.4_amd64.deb`. Extract with `dpkg-deb -x` into the author directory; do not install packages on the target host. Retain upstream `COPYING`, the libcap package's `usr/share/doc/libcap-dev/copyright`, sources and the exact build recipe with author inputs.

Below, `target/sandbox-author-inputs/` contains the extracted `bubblewrap-0.11.0/` and `libcap/`. Its source-local `config.h` contains only `#define PACKAGE_STRING "bubblewrap 0.11.0"` and `#define ENABLE_REQUIRE_USERNS 1`. Build with the explicitly selected author GNU C toolchain and static libc; no customer build tools are needed:

```sh
/usr/bin/cc -static -O2 -D_GNU_SOURCE \
  -I target/sandbox-author-inputs/libcap/usr/include \
  -I target/sandbox-author-inputs/bubblewrap-0.11.0 \
  target/sandbox-author-inputs/bubblewrap-0.11.0/bubblewrap.c \
  target/sandbox-author-inputs/bubblewrap-0.11.0/bind-mount.c \
  target/sandbox-author-inputs/bubblewrap-0.11.0/network.c \
  target/sandbox-author-inputs/bubblewrap-0.11.0/utils.c \
  target/sandbox-author-inputs/libcap/usr/lib/x86_64-linux-gnu/libcap.a \
  -o target/sandbox-author-inputs/bwrap-static
readelf -l target/sandbox-author-inputs/bwrap-static
readelf -d target/sandbox-author-inputs/bwrap-static
sha256sum target/sandbox-author-inputs/bwrap-static
```

The 2026-10-02 private author build is 1,099,016 bytes with SHA-256 `c9e8bd758c2d01ca6668a842bad7b020bf700436e87b666cfaae3803be0be1b3`; readelf confirms no interpreter and no dynamic section. A different author compiler/libc may produce a different digest: sign the actual validated bytes and deploy their matching content-addressed helper. The current policy source SHA-256 is `a6d2ef490d12914e90a96272a197b8b78667403702b1324bae661a10b78125ec`; AppArmor parser 4.0.1 accepted it offline with the command above. These are input/parse results, not evidence that host policy has been loaded.

Application file access is explicit in `config/setting.json`, for example `adapter.notification.delivery.files.uploads = {"path":"data/upload", "write":true}` with checked `allow file`. The Worker sees `/data/uploads`. Grant paths cannot write the executable cache or other execution inputs; no environment configuration is introduced.

## Make and consume

Build the private author executable once, then run it directly. The command optimizes existing compression and digest dependencies even in a development build, so complete language distributions do not spend minutes in unoptimized byte loops. These author/test options do not change application configuration or compile-service limits.

```sh
cargo build --offline --locked -p dever-cli --example native-release \
  --config 'profile.dev.package.miniz_oxide.opt-level=3' \
  --config 'profile.dev.package.flate2.opt-level=3' \
  --config 'profile.dev.package.sha2.opt-level=3'
target/debug/examples/native-release <author-root> --output <new-release-directory>
```

The output parent must exist. A private sibling staging directory holds copied inputs until the existing runtime validator succeeds and the manifest is signed. Publication atomically requires an absent destination, including under concurrent makers. Failures remove only owned staging. Output artifacts are independent copies; subsequent source changes do not change a completed pack.

The result contains `dever-core`, `lib/libLLVM.so.18.1`, `runtime/<platform>/manifest.json`, four `archives/<profile>.a` files, the configured link inputs, and top-level `manifest.json`/`manifest.sig`. The signed manifest covers the complete native closure. Place this version directory in the existing machine download store and use the established installer; trusted public-key provisioning remains explicit and separate. The maker does not trust a key merely because it accompanies a release, install a service, publish a catalog or update a global command.

For private `dever` development, the same verified `runtime/` and `lib/` trees may be copied beside a matching private compiler in a new directory. An executable named `dever-core` still requires the managed installation layout and authenticated daemon. This is not a service-unavailable fallback.

## Acceptance

Default `native_release` tests use synthetic input and temporary keys. The explicit Linux acceptance additionally consumes the four optimized archives in `target/native-release-inputs/`, the prepared native SDK and the built author executable:

Real ecosystem acceptance temporarily holds the signed release, installed version, compiler cache and Worker bundles simultaneously. Allow several GiB of free storage in addition to author inputs and Cargo artifacts. When the system temporary filesystem is too small, an optional author-only `target/native-release-inputs/config/setting.json` can select an existing executable filesystem:

```json
{"machine_temporary_root":"/dev/shm"}
```

The path must be absolute; unknown configuration fields are rejected. Only the ecosystem machine directory and bootstrap image use this setting. Author inputs remain on the normal temporary filesystem so their immutable hard links stay on the source filesystem. With no configuration, the existing temporary-directory behavior is unchanged. No environment variable is read for this setting. A memory-backed filesystem consumes real memory: provide both sufficient free capacity and an explicit acceptance-process memory budget, and run these large acceptances serially.

Before the no-host checks, build their test-only static init explicitly:

```sh
cargo rustc --offline --locked -j1 -p dever-cli --example native-acceptance-init -- -C target-feature=+crt-static
```

```sh
cargo test --offline --locked -p dever-cli --test native_release
cargo test --offline --locked -p dever-cli --test native_release \
  acceptance::optimized_profiles_make_reproducible_signed_release_and_run_through_daemon \
  -- --exact --ignored --nocapture
cargo test --offline --locked -p dever-cli --test native_release \
  acceptance::signed_core_builds_without_host_compiler_libraries_or_tools \
  -- --exact --ignored --nocapture
```

The second command starts only a test-owned daemon and SQLite databases. It checks actual driver symbols, identical manifests/signatures for repeated packaging, signed installation, four profile executions through the daemon, and independent programs after source removal and daemon shutdown. PostgreSQL server behavior is a separate acceptance, not implied by linking the PostgreSQL profile.

The third command requires root and namespace permission for a test-owned mount namespace root. It checks source and builds a SQLite program inside a root containing only the signed compiler closure, selected native archives and named GNU OS ABI libraries, then executes the program after removing its source. No host LLVM, C++ library, language tool, compiler SDK or package manager is visible there. The harness uses the explicitly prepared bwrap/loader assets to pivot into that root.

The harness rejects a dynamically linked init. Inside the already pivoted namespace, it replaces bwrap's proc submounts with a complete private proc mount so the nested Worker can mount its own proc. It keeps the outer root fixture's existing capabilities; the product's inner namespace, capability removal and guard remain unchanged. It never binds the host proc into the final root. This private compiler test complements the managed daemon test; it does not install a machine service.

To accept an already prepared official release (including its real signing identity), use the owned-image verifier. `--assets` feeds the exact three download files through the real installer using local transport; signature checks, extraction and bootstrap execution are unchanged. It requires the explicit static bwrap/init fixtures described above, creates a new image, starts only its own daemon and records each passed stage. Online `dever update` and real-host service activation remain separate checks.

```sh
python3 -B test/onboarding/verify_prepared_release.py \
  --release /prepared/release --assets /prepared/download-assets \
  --image /new/owned-image --report /owned-records/acceptance.json
```

The existing fixture-based bootstrap regression additionally requires the built `dever` and `deverd` binaries:

```sh
cargo test --offline --locked -p dever-cli --test native_release \
  acceptance::bootstrap::signed_bootstrap_installs_and_serves_two_projects_without_host_libraries \
  -- --exact --ignored --nocapture
```

It installs only into a new owned image, verifies stable entry and systemd assets, exercises a legitimate older signed unit upgrade and rejects tampering, then runs two projects through the installed daemon in the minimal OS root. Separate clients retain two real kernel UIDs to check shared-cache use and denied machine mutation. Finally the independently built programs run after source and machine-store removal. Neither the global service nor a public signing identity is involved.

Identical prepared inputs and signing key produce identical payloads, manifests and signatures. This establishes deterministic packaging, not reproducible source builds across different machines, CRTs or Rust toolchains. Temporary test signatures and Linux execution do not establish public release, six-platform delivery, an OS sandbox or performance capacity.

The explicit three-ecosystem acceptance uses fixed upstream archives and the retained Go build fixture listed with exact digests in `test/ecosystem-release/prepare.py`. Supply those inputs explicitly, then prepare a new directory:

| Input | Explicit source |
| --- | --- |
| `downloads/cpython-3.12.14.tar.gz` | [python-build-standalone 20260901, Linux x86_64 install-only stripped](https://github.com/astral-sh/python-build-standalone/releases/download/20260901/cpython-3.12.14%2B20260901-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz) |
| `downloads/node-v24.15.0-linux-x64.tar.xz` | [Node v24.15.0 Linux x64](https://nodejs.org/dist/v24.15.0/node-v24.15.0-linux-x64.tar.xz), verified against the release's `SHASUMS256.txt` |
| `target/go-managed/runtime.pack` | Go 1.26.3 private host fixture with explicit `build.host=linux-x86_64`; the pinned archive preserves the original tools/stdlib and repacks the updated descriptor. New fixtures use `test/go-managed/make_pack.go`; this is not an upstream published pack. |

The two download paths are below `target/ecosystem-release-inputs/`. No command here performs implicit downloads; the preparation script rejects different digests.

```sh
/usr/bin/python3 test/ecosystem-release/prepare.py target/ecosystem-release-inputs/prepared
cargo test --offline --locked -p dever-cli --test native_release \
  --config 'profile.dev.package.miniz_oxide.opt-level=3' \
  --config 'profile.dev.package.flate2.opt-level=3' \
  --config 'profile.dev.package.sha2.opt-level=3' \
  acceptance::ecosystems::signed_ecosystems_build_and_run_without_system_language_installations \
  -- --exact --ignored --nocapture
```

This opt-in test requires Linux root and mount/PID/user namespace capability. It signs and installs an owned release, uses the managed daemon for each ecosystem's lock/check/run/build, then removes project sources and the entire machine store. The resulting programs run inside an owned mount namespace root containing only named Linux OS ABI libraries, a private proc mount and synthetic devices. Python checks bundle-local imports, standard-library native modules and the source-built simplejson C extension; Node loads the source-built bufferutil addon; Go retains its managed probe. The already prepared source-build locks and exact output artifacts come from `target/source-build/dependency` and `target/npm-build/dependency`; this acceptance never rebuilds hooks or downloads dependencies. It signs the exact runtime-pack bytes bound by those build receipts, so this test's author signature differs from the separate deterministic-maker evidence. No system Python, Node, Go, compiler, SDK or package manager is visible there. Other operating systems and a public release identity remain separate acceptance.
