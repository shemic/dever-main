# Error Handling

Source loading returns path-specific `LoadError` values because unavailable or invalid UTF-8 files do not have a valid source span. Parsed-source errors use `Diagnostic` with a stable code, primary span and optional related labels.

- `Span` is a half-open UTF-8 byte range owned by one `SourceId`.
- Display positions are one-based lines and Unicode scalar columns. LF, CRLF and CR line endings use the same source model.
- Render the original source line with its explicit line/column location. Do not guess terminal display width with one ASCII space per Unicode scalar.
- Preserve source literal spelling through parsing; numeric representability belongs to checking.
- Reject invalid source with a diagnostic. Never turn an unsupported construct into a successful default AST.
- Parser recovery synchronizes only at balanced body boundaries or top-level newlines.
- Both parser call depth and resulting expression tree depth are bounded by `MAX_SYNTAX_DEPTH`. Long iterative operator/postfix chains must not bypass the limit and overflow during recursive AST destruction.
- I/O and syntax errors are explicit results. `expect` is reserved for compiler-owned invariants or infallible String formatting, not malformed source.
- Contract structural-depth rejection is fatal to that analysis pass: `contracts::check` returns `Err(C015)` before producing summaries. Both the main checker and Port fake reanalysis must stop before consumers read those summaries; ordinary contract diagnostics still aggregate after summaries are populated. The 256-layer nominal-type regression must report C015 rather than panic in a later phase.

Current codes: `L001` unexpected character, `L003` excluded keyword, `L004` malformed number, `L005` unterminated Text, `L006` invalid escape, `P001` expected syntax, `P002` invalid statement/assignment, `P009` excessive syntax nesting.

Markdown source codes: M001 no top-level program block, M002 unclosed program fence, M003 declaration spans multiple blocks, M004 invalid package documentation structure, M005 invalid declaration documentation structure, M006 documented package/API/declaration name mismatch, M007 function input mismatch, M008 function output or type-shape mismatch. Semantic documentation mismatches use the Markdown value as the primary span and add the corresponding Dever declaration as a related label. Tokens and diagnostics refer to original `.dever.md` byte positions, including Unicode columns and source newline conventions. Markdown cannot introduce a synthetic filename or bypass normal language diagnostics.

Semantic codes: `C002` duplicate declarations/bindings/known Map keys, `C003` package path or reserved namespace, `C004` unresolved names/signatures or uninitialized locals, `C005` type/arity/construction/primitive-contract errors, `C006` private cross-package access, `C007` function/type/package cycles, `C008` statically known numeric faults, `C009` clause overlap/coverage/reachability, `C010` naming format. `W001` reports unused/overwritten writes without rejecting the program. Diagnostics sort by source ID and byte span; check unused declarations before native reachability pruning.

Standard output errors propagate to the native entry as a failure containing the `.dever` call location. The process exits nonzero even if stderr itself cannot be written. CLI/toolchain failures are explicit command errors; `check` never invokes rustc. Native temporary directories have exclusive ownership and are cleaned on compilation failure or after use. Cleanup failure cannot mask the primary result; explicit build output never overwrites an existing file.

External Adapter business errors are decoded only when their exact identity and payload match a Port-declared error variant. Startup, schema/handshake mismatch, invalid or out-of-order protocol messages, timeout, child crash and undeclared error identities are runtime faults at the Port call site, never synthetic business variants. Protocol diagnostics contain metadata only and must not echo setting secrets or call payloads. A protocol fault terminates the Worker channel and closes pending calls. Generated entry cleanup runs after success, business failure and runtime fault; shutdown/database cleanup augments rather than replaces the primary error.

Embedded external resources are never executed before their relative path, regular-file type and SHA-256 digest verify. Extraction rejects symlinks, path escape, duplicate entries, truncated staging and invalid bundle identity; an existing or concurrently published directory is reusable only after a full recheck. Missing artifacts remain an explicit offline preparation error with the corresponding `dever lib add/update` command.

## Core Execution Contract

Compiler contract diagnostics add C011 (unproved range), C012 (unhandled failure), C013 (pure/clause contract), C014 (Model/transaction contract) and C015 (bounded proof work). W002–W005 and W007 are suggestions; W001 remains the owner of unused/overwritten writes. Details and counterexamples are in [Compiler Contracts](./compiler-contracts.md).

### 1. Scope / Trigger

Applies when changing checker-to-HIR-to-native contracts, bundled packages or runtime operations.

### 2. Signatures

- `check(&SourceMap) -> Result<Program, Vec<Diagnostic>>`; successful programs expose `warnings()` separately.
- `native::compile_project(&Program, &SourceMap, rustc, profile) -> Result<NativeProgram, String>` generates the application entry from API/CMD/Job declarations; compiler-internal library fixtures may still use `native::compile` with an explicitly selected zero-input entry.
- `dever.io.read(file, limit) -> ReadState`; `dever.net.read(socket, limit)` returns the same state. ReadResult is the capture choice.
- `dever.io.chunks(file, limit) -> Stream<ReadEvent>`; `dever.net.chunks(socket, limit) -> AsyncStream<ReadEvent>`, and `dever.net.connections(listener) -> AsyncStream<ConnectResult>`. ReadStreamResult choices capture creation failures.
- `dever.http.send -> Response`; ResponseResult captures recoverable errors. `serve` has zero outputs and propagates configuration/accept faults. Handler faults are logged at the request boundary and return 500 or abort an already-started response; later requests remain available. Peer protocol/I/O errors close only the affected connection; body/handler timeouts return 408/504 before closing, incomplete-header timeout closes directly.

### 3. Contracts

`ReadState` is `Read(bytes: Bytes)` or `End`; `ReadResult` is `Done(state: ReadState)` or `Failed(message: Text)`. Ordinary calls return success values; explicit result captures convert the inferred failure set into choice values. Stream reads yield source-defined `ReadEvent` values and produce at most one terminal `Failed`; normal EOF produces no synthetic item. File creation uses create-new; resources share identity and close state across aliases, unlike ordinary value-semantic records/collections. Read limits and socket timeouts are positive Ints; reads may be short. Stream creation rejects a non-positive limit or already-closed source before returning a Stream. Core `close(stream)` is idempotent.

Handler parameters are compile-time bindings, never runtime values or clause axes. Every value type nested in a handler signature uses the same generic arity and Map-key validation as an ordinary function boundary. Handler signature, output-name and specialization-cycle errors are source diagnostics before build. `reduce_until` applies the current item, then stops without pulling another item.

`result(call)` changes business-failure representation, never scheduling. Captured calls share ordinary-call suspension/liveness and inline-blocking checks. A synchronous capture must remain legal with synchronous resources; capturing a suspending call must diagnose non-transferable live locals at the capture span, and capturing a blocking call must not bypass an async caller's worker boundary. Keep both negative paths and the synchronous positive control in `structured_concurrency`.

Build/run use the local bootstrap toolchain (`RUSTC`, optional `CARGO`) and locked cached dependencies with `--offline`; missing tools/dependencies fail explicitly. Generated binaries link the runtime, not the checker or a generic evaluator, and execute without the build toolchain.

All runtime feature combinations build through one Cargo target. Only artifacts reported for the current Cargo graph enter an immutable input view; a missing, symbolic-link, changed or incomplete input fails before generated code is linked. Native temporary directories carry a versioned owner marker and are removed immediately by RAII. Recovery cleanup deletes only current-user, real directories with an exact Dever name/marker older than 24 hours; unknown, fresh, active or symbolic-link paths are skipped. Cleanup failure may be returned by explicit `clean`, but automatic cleanup never replaces the current compile or run result.

### 4. Validation & Error Matrix

| Condition | Result |
| --- | --- |
| Incomplete record/output, type drift, illegal key | Source diagnostic before build |
| Invalid generic or Map key nested in a handler signature | C005 before build |
| Clause overlap, gap or empty complement | C009; overlap labels both clauses |
| Known Int/Decimal overflow or zero divisor | C008 before build |
| Dynamic Int/Decimal fault or stdout failure | Source-located native failure and nonzero exit |
| Invalid bytes, file/socket error, closed resource | Explicit Failed choice |
| Invalid stream limit or closed source at creation | Explicit Failed choice; no Stream returned |
| Stream producer terminal error | One Failed item, then end; producer released immediately |
| timeout/race receives a program fault | Propagate after structured cleanup; never invoke timeout fallback for a fault |
| timeout expires | Stop and drain the task before its signature-matched fallback runs |
| Pool closed, request/stream deadline or TLS verification failure | Explicit Failed; failed/incomplete connections are not reused |
| Stream(Channel/List/Bytes) contains error choices | Preserve their failure obligations; constructing or closing the stream does not handle them |
| HTTP listener explicitly closed | Stop accepting, drain active responses; application timeout may stop remaining work |
| Invalid/reused build output path | Command error; existing file preserved |
| Runtime input view is incomplete or changed | Build error; never scan unrelated historical artifacts |
| Clean candidate has no valid owner marker, is fresh, active or a symlink | Preserve it unchanged |

### 5. Good / Base / Bad Cases

Good: `Text` plus `null` clauses infer `Text?`; `first([null])` in an `Int?` output context remains `Int?`. Base: hello prints one UTF-8 line. Bad: `Bool` plus `true` clauses overlap; repeated constant or zero-payload choice Map keys are rejected.

### 6. Required Tests

Use `core_semantics` for checking/rejection, `core_native` for exact outputs/evaluation order/native failure locations, `runtime_foundations` for decimal precision/COW/resource closure, and `hello_native` for CLI-backend regressions. Stream tests must assert that mapping preserves a terminal item as terminal, releases its producer on that pull, and that official wrappers map invalid creation to `ReadStreamResult.Failed`. Run only affected targets and explicitly announce owned temporary-file/loopback checks.

### 7. Wrong vs Correct

Wrong: select the first matching source clause, append anonymous standard files to a cloned SourceMap, emit an untyped Value interpreter, reorder record expressions by field name, or implement Stream mapping through an API that erases `Pull::Last`.

Correct: prove disjoint exhaustive domains before emission, keep bundled source origins retrievable by the caller's SourceMap, emit concrete types/direct handlers, evaluate constructor fields in source order, and preserve `Item`/`Last`/`End` through Stream adapters.
