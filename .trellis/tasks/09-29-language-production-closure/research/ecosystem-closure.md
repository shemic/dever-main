# Research: External ecosystem closure

- Query: Identify remaining pip/npm/Go implementation owners, contract changes, reusable paths, and independently executable slices.
- Scope: mixed; read-only source and primary protocol documentation; no Cargo, package installation or service tests.
- Date: 2026-10-02
- Task: parent explicitly supplied this task directory; `task.py current --source` returned none in the child session. No task pointer changed.

## Findings

### Existing implementation is substantial

Signed Python/Node/Go pack authoring, immutable runtime identities, Worker preparation, Go compilation, shared compilation and no-host-language Linux standard-library execution already exist. Latest acceptance is recorded in implement.md:660. These are not new implementation gaps.

| Owner | Current behavior and relevant source |
| --- | --- |
| `crates/dever-cli/src/libs.rs` | `LibSpec` at 118, canonical lock v2 at 198/244, independent Worker resolution at 487, graph doctor at 1343, explicit commands at 1461. |
| `crates/dever-cli/src/libs/registry.rs` | Bounded official transport at 98; Python/npm solver at 357–628; archive locking at 725; Go MVS at 773; extraction at 1155. |
| `crates/dever-cli/src/workers.rs` | Dependency closure at 460; Python installation at 529; npm installation at 567; shared resource Tree at 620. |
| `crates/dever-cli/src/workers/runtime.rs` | Shared author/consumer runtime manifest and file-closure validation. Interpreter descriptors cannot currently declare build tools. |
| `crates/dever-cli/src/workers/go_build.rs` | Private owned staging, exact packed tools, transitive package compile/link, cancellation/deadline. `Builder::compile` at 346 includes deterministic trimpath. |
| `crates/dever-cli/src/packages.rs` | Package Lib declarations use the same `LibSpec` parser at 274, 601, 672, 953; no second ecosystem resolver is needed. |
| `crates/dever-core/src/check/ports.rs:333` | External Lib declarations receive shape checks; quoted `name[extra]@version` already fits the syntax. Semantic extras checking belongs in the CLI library contract. |
| `test/dever-cli-tests/external_libs.rs` | Injected `LocalRegistry`, real archive fixtures, exact provider metadata, independent Worker versions, canonical lock, offline remove, real loopback redirect rules. |
| `test/dever-cli-tests/external_workers.rs` | Prepared resource inspection, native Python rejection, npm hook rejection, real optional managed execution. |
| `test/go-managed/analyze/main.go` | Go build tag/source analyzer; deliberately rejects cgo/assembly/native source. Not a general Go build frontend. |

### First independently executable slice: Python extras

Actual missing behavior:

- `LibSpec` has only ecosystem/name/version. Parsing name as `PackageName` rejects brackets (`libs.rs:130`).
- `python_dependencies` evaluates markers with an empty extras list and rejects dependency extras (`registry.rs:920–947`).
- The bounded solver chooses versions by base name; extras need monotone dependency activation, including newly activated extras on already-selected nodes and after backtracking (`registry.rs:445–573`).

Smallest useful design:

1. Extend the existing Lib identity/parser with sorted, normalized Python extra names using installed `pep508_rs::ExtraName`. Keep `name` the canonical distribution name. Serialize `pip:name[a,b]@version` through the single `key()` owner. Reject extras on npm/go, malformed or empty brackets and noncanonical serialized identities.
2. Use explicit lock format v3 if selected-extra or dependency-edge semantics change; directly migrate repository callers/fixtures. Do not add a permissive v2 compatibility path. Package source manifests can continue storing the same strings.
3. Reuse `Requirement::evaluate_markers(markers, extras)` and existing PEP440 constraints. Maintain extras per requested distribution inside each search branch; activating an extra must re-evaluate that selected distribution's metadata until stable. Do not mutate sibling branches or silently ignore unknown declared extras.
4. Preserve the exact requested extra variant on Worker roots and dependency edges. A single package version may have base and extra variants sharing the same archive. Do not union all Workers' extras globally: `resolve_workers` intentionally solves each Worker separately before merging.
5. During materialization, deduplicate identical distribution/version/artifact installations; conflicting bytes remain an error. Do not change runtime imports or introduce per-extra runtime modules.

Important graph caveat: storing only one union-extra `LockedLib` per base package is insufficient. A plain Worker and an extras Worker could then share expanded edges, violating isolation or triggering `inconsistent resolved metadata` in `resolve_workers`. Either retain explicitly requested variants as distinct lock nodes (while selecting one version per base name per Worker) or introduce an explicit resolved-context identity. The former is the smaller extension. Existing cross-context version-selection conflicts must remain explicit, never overwritten. A `foo -> foo[feature]` relationship is not a package version cycle; avoid accidental self-edge rejection from treating expansion as an ordinary recursive package edge.

Acceptance for this slice:

- Normalize case/separators, duplicate extras, deterministic ordering; round-trip and malformed non-Python rejection.
- Base request excludes extra-only dependencies; extra request includes them.
- Transitive `a -> b[x]`; two parents request disjoint extras on the same b; late activation on an already-selected b; backtracking selects compatible versions without leaking failed-branch extra state.
- Different Workers requesting plain versus extra variants preserve exact roots/edges and installation closure.
- Package-provided extra declarations, lib remove/update, stale/corrupt lock rejection, offline preparation and one physical wheel installation.
- Focused default targets: external_libs, affected external_workers and packages. No interpreter/network required for resolver tests.

### Python platform wheels and source distributions are separate slices

`lock_python` only selects `bdist_wheel` matching signed runtime tags (`registry.rs:630–692`). Worker installation then rejects every `.so/.pyd/.dll/.dylib`, `.data/platlib` and `Root-Is-Purelib: false` (`workers.rs:537–557`). Thus tags alone do not establish native-wheel support.

Platform-wheel slice should first make signed pack tags describe actual ABI/platform compatibility, check WHEEL identity/tag and RECORD/file integrity rules, install purelib/platlib through the existing bounded Tree and verify native dependencies against a packaged OS ABI baseline. Keep `.pth` executable hooks a distinct policy. Test an actual compatible extension and missing/wrong ELF/ABI dependencies in the no-host execution fixture. Merely allowing filename suffixes is not enough.

Sdist requires a real PEP517 build frontend: acquire and lock source plus build-system requirements; resolve dynamic `get_requires_for_build_wheel`; run backend in a private no-network build sandbox; validate and publish the generated wheel with build provenance. `run/build` then consume only locked outputs, never call an online package manager. Existing `archive_files` supports zip and gzip tar, with bounded no-link extraction; reuse it for admitted sdists instead of unrestricted host extraction.

The current pack has an interpreter/stdlib only and deliberately strips development inputs. It does not include setuptools/build backends, Python development headers, C/C++/Rust compiler toolchains, sysroot, or arbitrary external system libraries. Source/native builds need signed explicit build packs and locks, not discovery of host cc/cmake/rustc. A deterministic source-build lock must bind source digest, backend+build dependency closure, target/runtime ABI, selected signed toolchain, explicit args/config and output digest. Add that structured provenance to the same lock owner, not a loose per-project cache flag.

### npm advanced dependency graph

`npm_dependencies` rejects nonempty optionalDependencies, peerDependencies, bundledDependencies and bundleDependencies (`registry.rs:959`). Existing search and `doctor` enforce one version per name per Worker; installation is flat `node_modules/<name>` (`workers.rs:608`).

- First add typed metadata/edge kinds and target eligibility. Optional dependencies override same-named regular entries. Optional peers are constraints when present, not auto-installed dependencies. Record omitted optional nodes with a reproducible reason; checksum/signature/protocol failures must not become optional-success fallback.
- Required peers need resolution in their consumer context. A one-version flat graph supports only the compatible subset. Full npm semantics require package-instance identity and installation path ownership, since independent branches legitimately select incompatible versions. Do not claim complete npm support by promoting every peer/optional edge to an ordinary mandatory edge.
- Bundled dependencies are already bytes in the parent tarball. Validate declared bundled package names, package.json identities and nested closures; bind them to the signed parent artifact rather than downloading or flattening them. Reject undeclared/colliding injected trees.
- Keep one extraction/publication owner. Extend lock graph and installer together, with exact instance references and safe relative installation paths if nested graphs are introduced.

Minimal units: optional/peer metadata+graph tests; bundled archive installation tests; nested package-instance graph+layout if required; then controlled hooks/native builds. Scope claims must distinguish these.

### npm hooks/native compilation

`workers.rs:583` rejects lifecycle scripts and gypfile, and 606 rejects `.node`/binding.gyp. Removing these checks would execute or accept unverified install behavior without a build owner.

Use the same isolated explicit build-artifact owner as Python. npm lifecycle scripts are shell strings, not safely interchangeable with a simplistic `split_whitespace` Node command. Supporting their existing ecosystem semantics requires a signed known shell/tool set, limited filesystem/network/process capabilities, declared Node/ABI/toolchain/headers, bounded capture and complete descendant cleanup. That internal fixed build environment must not read user PATH/env. If only direct JavaScript entry hooks are supported, name it as a restricted contract rather than general npm lifecycle support. Product user configuration stays in setting.json.

Lock completed outputs and source/script/tool/target provenance during explicit lib preparation. Runtime preparation must not rerun hooks or consult the network. Test hostile output paths/links, timeout descendants, denied network, native ABI mismatch and cached replay.

### Go sumdb

Current `go_mod` fetches unverified .mod bytes, and `resolve_go` fetches ZIP then immediately calls `lock_archive` (`registry.rs:831–839`). Local archive SHA verifies cached bytes later but does not authenticate registry content. Both .mod and ZIP must be sumdb-verified before use/publication.

Required owner: a bounded `libs/registry` sumdb module behind the existing transport, with fixed official key/origin, explicit injected fixture key/transport for tests, not GOSUMDB/GOPRIVATE environment variables. Existing ring SHA/Ed25519, sha2, base64 are available. Either implement the small checked protocol against upstream vectors or supply a signed verifier with a narrow offline API; do not invoke a host Go command.

Full contract: Go dirhash h1 for the uncompressed logical module file tree (not raw ZIP SHA), separate go.mod h1; lookup record framing/module/version identity; signed checkpoint note verification; inclusion proofs; consistency against previously trusted checkpoints; bounded authenticated tile cache. Verified lock provenance must include enough checkpoint/record evidence or immutable digest binding for offline install. Do not trust HTTPS lookup text alone. Verify every .mod consulted for MVS, including versions not in the final build graph. Retain raw artifact SHA for Dever cache identity alongside upstream h1.

Tests: official independent dirhash/tlog/note vectors; ZIP order/recompression same h1; changed file/go.mod rejected before publication; invalid key/signature/wrong record/inclusion/consistency/rollback; uppercase path escaping; offline verified replay; transport limits. Then one real public pure-Go package rebuilt and executed with existing signed pack. Existing cgo/assembly exclusion is a separate documented boundary, not a missing sumdb feature.

### Order and shared contracts

1. Python extras is executable without adding runtime/build assets or changing sandbox policy.
2. npm metadata and graph semantics can follow independently; preserve claims about nested versions.
3. Go sumdb is a security-critical independent resolver slice and needs dedicated protocol review.
4. Platform-wheel/native-loader acceptance can precede arbitrary source builds.
5. Finish shared OS-enforced build sandbox and signed tool/header/backend assets, then Python sdist/npm hooks through one owned artifact preparation boundary.
6. Finally extend the actual signed-release acceptance with pinned third-party inputs, offline reconstruction and no-host execution; do not reuse standard-library probes as evidence for third-party installation.

## External references

Primary sources inspected on 2026-10-02:

- https://packaging.python.org/en/latest/specifications/pyproject-toml/ — build-system dependencies and dynamic project metadata. Build requirements belong to preparation, not runtime imports.
- https://peps.python.org/pep-0517/ — backend hooks and isolated build requirements; confirms sdist support is a build frontend, not archive copying.
- https://docs.npmjs.com/cli/v11/configuring-npm/package-json/ — npm v11 optional/peer/bundled contracts. Optional peers are not auto-installed; optionalDependencies override regular same-named entries.
- https://go.dev/ref/mod#authenticating — Go checksum database requires inclusion and consistency verification before accepting lookup data; h1 differs from ZIP archive digest.
- https://pkg.go.dev/golang.org/x/mod/sumdb/tlog — canonical transparent-log API and formats useful for independent conformance vectors.

## Related specs

- `.trellis/spec/backend/toolchain-and-library.md`: signed packs, no PATH/env/network fallback, resource budgets, independent Worker lifecycle, immutable cache and focused tests.
- `.trellis/spec/backend/error-handling.md`: explicit errors, no secret-bearing protocol diagnostics, owned cleanup and offline preparation failures.
- `.trellis/spec/backend/directory-structure.md`: core/CLI/runtime ownership and root test layout.
- Parent `prd.md` C/D/E and `design.md` External Lib boundary; `implement.md` records existing evidence and retained gaps.

## Caveats / Not Found

- Research only; no product code or acceptance was changed or executed.
- Current optional/peer/sdist policies deliberately fail closed. They are missing supported behaviors, not justification to delete validation.
- No signed general native build-tool pack or controlled package-hook executor found. Interpreter runtime packs do not provide those by implication.
- Full npm multi-version context resolution and Python extras cross-Worker identity need explicit lock semantics. A broad lock bump should cover changed semantics coherently, without speculative unused fields.
- Existing author/CLI tests are canonical reusable fixtures; public network package tests should remain explicit rather than become ordinary unit-test dependencies.
