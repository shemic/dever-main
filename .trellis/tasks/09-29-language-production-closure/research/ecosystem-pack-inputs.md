# Research: ecosystem pack author inputs and acceptance reuse

- Query: Identify available real Python/Node/Go inputs, reusable acceptance, relocation limitations, and disk costs for signed ecosystem packs.
- Scope: internal; read-only source/filesystem inspection and executable version queries; no builds, tests, downloads or services.
- Date: 2026-10-02

## Findings

### Available inputs

| Ecosystem | Explicit input | Observed identity / size | Evidence limit |
|---|---|---|---|
| Python | `/usr/bin/python3.12`, `/usr/lib/python3.12` | Python 3.12.3 (Aug 31 2026 local build), executable 8,020,928 bytes; stdlib directory 54 MiB | Distribution configured for `/usr`, not verified relocatable. |
| Node | `/root/.nvm/versions/node/v24.15.0/bin/node` | v24.15.0, 122,889,056 bytes | Actual runtime, but needs OS dynamic libraries. Whole nvm version tree is 991 MiB and includes unrelated global dependencies. |
| Go | `target/go-managed/runtime.pack` | 55,785,046 compressed bytes, 203,288,938 extracted bytes, 358 regular entries; manifest says go1.26.3 linux/amd64 | Existing private fixed-target build pack, not an independently verified published release. |

`df -h /data/project/dever` reported 2.0 GiB free during inspection. `target/go-managed/cache` is 50 MiB; preserve it unless the parent explicitly needs reclaimable build caches. Serial acceptance avoids duplicating multiple 194 MiB extracted Go trees and signed release copies concurrently.

Python `-I -S -B` currently reports `/usr/lib/python312.zip`, `/usr/lib/python3.12`, `/usr/lib/python3.12/lib-dynload` in `sys.path`. `sysconfig` records `--prefix=/usr`, system expat and system SSL. `readelf` shows executable dependencies on libm, libz, libexpat and libc. Extension modules add libssl/libcrypto, sqlite3, ffi, bz2, lzma, crypt, ncurses/tinfo/panel, db/gdbm and readline. Therefore copying only python3.12 is definitively insufficient for independent Python operation. Copying stdlib alone also does not close the extension native dependency set.

Python directory contains symlinks: `_sysconfigdata__linux_x86_64-linux-gnu.py` is a sibling alias, `sitecustomize.py` points outside to `/etc/python3.12/sitecustomize.py`, and config libpython points outside the directory. Prepared author inputs must be explicit regular files; do not blindly archive this tree or copy developer sitecustomize. No isolated relocation probe was run in this research; relocating executable plus `lib/python3.12` must be verified rather than assumed. A complete stdlib claim also needs extension loading and filesystem path assertions, not just a JSON echo.

Node `readelf` NEEDED entries are libdl, libstdc++, libm, libgcc_s, libpthread, libc and ld-linux-x86-64. There is no RPATH/RUNPATH entry. No external JavaScript stdlib directory is referenced by the current SDK/runner; only the runtime binary plus SDK/adapter/dependencies is needed for existing fixtures. This is a practical runtime input, but host-copy execution does not prove the declared OS ABI baseline or a clean machine lacking these shared libraries.

The Go pack already bundles compiler, linker, analyzer, a complete generated stdlib importcfg and archives. Its manifest does not declare a runtime interpreter. A bounded search did not find an active Go installation at `/usr/local/go`, `/usr/bin/go`, `/usr/local/bin/go`, `/root/go` or `/root/.local`; the retained pack suffices for existing build acceptance without locating/rebuilding host Go.

### Existing owners and tests

- `crates/dever-cli/src/libs.rs:631`: installed registry provider reads signed `runtime/<ecosystem>/<platform>/manifest.json` and adjacent runtime.pack; verifies target, metadata requirements, pack SHA and 256 MiB compressed bound. Reuse this chain rather than introducing a second trusted-runtime loader.
- `crates/dever-cli/src/libs/registry.rs:30`: `InstalledRegistryPack` / `RegistryRuntime` own registry metadata and Python markers/wheel tags.
- `crates/dever-cli/src/workers.rs:58`: existing `dever-worker-runtime-v1` parser; `prepare_worker` at 151 validates lock identity, then archive and inner manifest. Tree bound at 17 is 256 MiB uncompressed and 65,536 files. The retained Go pack fits with about 53 MiB remaining for related generated content.
- `crates/dever-cli/src/workers/go_build.rs:27`: fixed-target build descriptor; line 452 uses environment-cleared absolute tool execution. No new Go launcher needed.
- `test/go-managed/make_pack.go:22`: existing private maker builds source selector, lists all stdlib exports, copies tools and writes sorted tar entries. It is explicitly a private fixture maker, not a signed production author tool; its Go build environment is author-side only.
- `test/dever-cli-tests/external_workers.rs:287`: ignored real Go test consumes retained pack, locked module ZIP, target build tags and go:embed, compiles SDK/runner, removes original adapter source, and runs standalone Worker with cleared environment. Asserts generated resources do not retain build/source trees. Reuse this behavioral coverage for produced Go pack acceptance.
- `test/dever-cli-tests/external_workers.rs:647`: real Node fixture run; 653 Python host fixture run; 660/677 CommonJS/ESM export binding; 691 packaged Python SDK shadow protection. Several intentionally override generated executable with a host path. These prove protocol/binding, not release relocation.
- `test/dever-cli-tests/external_workers.rs:706`: relocates an embedded Node Worker after removing source, lock and raw pack. It still compiles via old explicit Rust author helper. New signed-release acceptance should use current LLVM path rather than duplicate this old harness.
- `test/dever-cli-tests/support/daemon.rs`: shared owned-daemon lifecycle helper from native acceptance; use for signed managed execution rather than another child owner.
- `sdk/python/dever_component.py:8`: SDK imports asyncio/json/math/re/struct/sys/threading/uuid/dataclasses/typing and therefore real Python stdlib availability is required before worker startup.
- `sdk/javascript/dever_component.mjs`: checked wire/protocol implementation to embed; current generated runner in workers.rs uses `node:console` and `node:fs`.

### Concrete acceptance recommendations

1. Keep maker consumption deterministic and offline: prepare explicit digest-bound regular-file ecosystem inputs separately, then include registry manifest + inner runtime.pack in the same signed release manifest. Do not discover Python/Node/Go through PATH or add application environment configuration.
2. Reuse retained Go pack immediately for signed-install → lock-bound prepare → worker execution. Preserve target/stdlib contract and assert executable output only; source/compiler archives must not survive in final Go Worker resources.
3. For Node, stage only the explicit binary and existing SDK/fixtures, not the 991 MiB nvm tree. Verify CommonJS and ESM, relocation, empty environment, removed original source/pack and tampered signed artifact rejection.
4. For Python, provide an explicit prepared stdlib tree and reject symlink/escaping input; test `sys.prefix`, `sys.path` and imported modules' `__file__` all point inside the relocated runtime where applicable. Exercise asyncio plus SSL/sqlite/ctypes/compression if advertising their availability. A bare executable test can falsely pass by reading `/usr/lib/python3.12`.
5. Distinguish same-host relocation from a no-system-runtime OS image. Neither `env_clear()` nor deleting the project source hides `/usr/lib/python3.12` or host shared libraries. Do not label same-host copied interpreter checks as clean-machine release acceptance. Do not rename/hide global host directories to simulate this.

## Related specs

- `.trellis/spec/backend/index.md`: focused offline compiler checks and root test ownership.
- `.trellis/spec/backend/toolchain-and-library.md:173`: verified installed signed registry runtimes and explicit network boundaries.
- `.trellis/spec/backend/toolchain-and-library.md:175`: one lock-bound pack protocol, Python isolation, Go fixed-target tools, no host language lookup.
- `.trellis/tasks/09-29-language-production-closure/{prd,design}.md`: actual ecological independence remains a required outcome, unavailable evidence must be marked honestly.

## External references

No external browsing was performed: this topic inventories local prepared inputs and existing code only. Version numbers above came from explicit binaries or retained manifest, not current upstream release claims.

## Caveats / Not Found

- Sub-agent `task.py current --source` returned none because its session pointer is absent; parent explicitly supplied this output task directory, so no task selection was guessed or changed.
- No ready signed Python/Node ecosystem release pack was found in the bounded target/tmp paths inspected.
- No clean-machine container/rootfs, official upstream signature, source-build reproducibility, license aggregation or alternate target runtime was verified here.
- No modifications outside this research file; no Cargo, Go compilation, downloads, tests, service startup or destructive action performed.

## Follow-up: shared compilation size and LLVM representation

- Query: Are proposed 128 MiB single / 256 MiB total resources, 384 MiB request and 320 MiB compiled output enough for the real Node binary?
- Date: 2026-10-02. Read-only inspection, no measurements or tests.

### Definite blocker beyond IPC limits

`crates/dever-core/src/llvm/external.rs:128` emits every resource byte as a three-character `\\XX` escaped LLVM string. The 122,889,056-byte Node executable becomes 368,667,168 textual bytes (351.6 MiB) before other code. `crates/dever-backend-bridge/src/lib.rs:57` rejects IR larger than 8 MiB. Raising only IPC/cache limits is therefore insufficient.

`crates/dever-backend-bridge/src/bridge.cpp:59` additionally copies all textual IR into an LLVM MemoryBuffer before parsing; parsing then materializes the actual constant. `worker.rs:75` imposes CPU 120 seconds, address space 2 GiB, file size 1 GiB, 128 FDs and no core dump. Raising the IR cap to accept 256 MiB of escaped resources would admit 768 MiB text plus its C++ copy, decoded request/resources, LLVM objects and object-output copies; this conflicts with the existing 2 GiB worker budget before realistic linking overhead.

Smallest coherent direction: keep bounded textual code IR and move resource payloads to a separate dense bridge input. IR declares named external byte-array constants; the existing private LLVM bridge defines them with dense ConstantDataArray initializers before verification/emission, or emits one separately linked data object. Reuse `native::resource_inputs` digest deduplication and the exact existing symbols/descriptors. Validate missing/extra resources, symbol shape/length/collision and raw SHA at the existing owner. Do not introduce an independent archive/decoder in application runtime merely to solve compile transport.

Compression of IPC bytes is not necessary to admit this actual Node case: base64 expands it to 163,852,076 bytes, within proposed 384 MiB. It also would not solve the current LLVM expansion. Compression would add another protocol plus decompression limits; defer unless measured transport costs warrant it after dense resource emission. Existing encoded cap remains an independent cap: 256 MiB resources consume about 341.3 MiB base64, leaving about 42.7 MiB for text/metadata; pathological escaping of the permitted 16 MiB source can exceed that, which should remain an explicit encoded-limit rejection rather than an allocation assumption.

### Limit consumers and preserved ownership

- `toolchain/compilation.rs:12,117,136,174,194,292`: encoded cap, resource count/total, single decoded cap, canonical encode/decode and base64 deserializer encoded bound must move together; use named constants to avoid mismatched limits.
- `toolchain/service/compile_request.rs:45`: returned executable receipt needs compiled-specific validation before allocating output.
- `toolchain/service/compile.rs:97,144`: output regular-file metadata and outgoing receipt need the same compiled-specific limit.
- `toolchain/cache.rs:293`: `publish` is trusted compiled-output owner; change only this limit, preserve 2 GiB quota and 4096 entries.
- `toolchain/cache.rs:462`: `inspect_entry` currently checks format/key and file length/hash but no compiled byte bound. Apply compiled receipt/size validation before `restore` reads entire file (`cache.rs:356`), including status and quota inspection paths.
- Keep ordinary `ArtifactReceipt::validate` (cache.rs:86), publish_artifact/artifact/artifact_path, service artifact_put/get and ArtifactPut dispatch at 64 MiB. Do not weaken generic user-upload quota to admit trusted compiled output.

### Memory/lifecycle cleanup worth doing with caps

`CompileRequest::new` clones resource bytes; `encode` deep-clones the full request to sort; `decode` re-encodes to verify canonical representation. The daemon retains its upload buffer, decoded request and canonical payload while running the worker; `worker.rs` retains encoded input through lowering, while `request.embedded_resources()` clones decoded bytes again. At the proposed limits these create substantial avoidable overlapping allocations. Drop encoded input after canonical decode, release daemon decoded request before long worker execution once identity/version are retained, and avoid cloning payloads merely to sort serialization. Prefer existing ownership moves/borrowed sorted references; do not add compression as a substitute for fixing live owners.

Daemon admission allows two active compilations (`service/compile.rs:27`); service transfer reader caps upload at 60 seconds (`service.rs:580`), individual socket read/write timeout is 5 seconds, total compilation including upload/link/download is 180 seconds, and client response/download is 190 seconds. Keep these limits initially and measure real Node acceptance. CPU/address-space limits constrain the child only, not daemon/client copies. Raw object output is also copied by bridge reply ownership, so dense inputs remove a major expansion but do not by themselves prove maximum-limit memory acceptance.
