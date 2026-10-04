# Dever Minimal Core 0.1 Design

## N4 Concurrent Composition And Reusable Clients (2026-09-12, Complete)

Source `dever.task.start` establishes the one runtime from a synchronous entry with an explicit configuration and static async handler. HTTP system primitives infer an optional final transferable context and pass it to a statically checked handler; applications can wrap this in their own concrete context signature. Ordinary records retain value semantics; Channel/client/resource fields share their existing resource identity.

`timeout(task, millis, fallback)` consumes a Task, waits for cancellation cleanup on expiry, then invokes a zero-input async fallback with the identical output signature. `race(tasks...)` consumes equally typed tasks and drains losers before returning the first result. Neither operation suppresses program faults. `stream(Channel|List|Bytes)` and periodic `dever.task.ticks` are pull-driven; async memory traversal reuses collection handlers and bounded Group dispatch.

Network clients own bounded reusable HTTP/1 connections and stream bodies. TLS uses tokio-rustls/ring, with certificate verification, bundled public roots and explicit custom roots/server PEM material. TCP/TLS share the concrete I/O owner used by HTTP and WebSocket upgrades. Protocol parsing stays in Hyper/tungstenite. Idle connections, pending requests, body chunks and cancellation have explicit owners. No HTTP/2, implicit retries, redirects or detached connection drivers.

The implemented pool fixes one origin per shared HttpClient and uses Hyper's low-level HTTP/1 API. Separate request and physical-connection permits include idle/queued drivers in the same limit. One structured task drives the bounded queue and FuturesUnordered, expiring idle entries without a future request. Its owner guard is constructed before task creation, including the unpolled-child cancellation case. Response EOF returns its lease; early close/fault/cancellation aborts the connection. Listener.close triggers graceful HTTP shutdown; timeout(server, grace_ms, fallback) bounds draining active responses and upgrades.

Locked additions are tokio-rustls 0.26.5, rustls 0.23.44 with ring/std/tls12 and webpki-roots 1.0.8. Shared TLS configuration uses small explicit session-cache capacities. Transport boxes only the large TLS variant; ordinary TCP stays direct. The backend example combines configured source entry, concrete Context with Channel, a pooled request and graceful shutdown on an owned random loopback port.

## N3 Streaming HTTP, SSE And WebSocket (2026-09-12, Approved)

Keep ordinary `serve` direct; `serve_live` binds an async zero-output `(Request, HttpReply)` handler. A Group of one owns each connection's live handler. Shared request decoding, checked response encoding and Hyper configuration avoid a parallel HTTP implementation. A one-slot body channel provides backpressure. Response-body Drop signals cancellation for streamed, buffered and HEAD responses; Session cleanup closes the producer or upgraded socket. Handler faults propagate and cannot be emitted as a successful truncated body.

On upgrade, wait for Hyper's driver, extract its known TcpStream and preread bytes, then keep the connection reservation while awaiting the handler Group. This removes the HTTP write deadline and the need for an extra transport abstraction. WebSocket uses independent reader/writer locks around tokio-tungstenite split halves, the shared Endpoint close signal and bounded message/write buffers with a 4 KiB read buffer. Acquire the writer lock before arming cancellation cleanup. close takes final transport ownership, wakes other operations and completes a bounded handshake.

SSE is a small field encoder plus response-body timer; no routing framework or heartbeat task is needed. Preserve CR/LF/CRLF and trailing data lines, compute final size before allocation and reserve Content-Type for the protocol. Request headers carry Last-Event-ID; replay belongs to application code. WebSocket control frames are processed while receive/messages is polled; no automatic reader or Ping task. Public exact limits and lifecycle are defined once in LANGUAGE.md.

References checked: [tokio-tungstenite 0.30.0](https://docs.rs/tokio-tungstenite/0.30.0/tokio_tungstenite/), [WebSocketConfig](https://docs.rs/tungstenite/0.30.0/tungstenite/protocol/struct.WebSocketConfig.html), [EventSource parsing](https://html.spec.whatwg.org/multipage/server-sent-events.html#parsing-an-event-stream). No TLS, HTTP/2, pooling, protocol extensions or compatibility layer in N3.

## N2 HTTP Engine (2026-09-12, Approved)

Hyper HTTP/1 owns parsing/framing/persistence, using the existing Tokio runtime and only hyper-util's Tokio I/O/timer adapter. No Axum dependency is needed for the current single static route contract. This supersedes earlier source-only HTTP and per-request fault-isolation proposals below: handler program faults propagate through the structured server task, while peer protocol/I/O failures end only that connection.

Official source owns Header/Request/Response/Limits, header helpers, send and serve. Header is Text name plus Bytes value; List preserves repeated values. Native conversions resolve fields by name after exact shape checks. Intrinsic carries an optional static callback; callback extraction is shared with Collection for dependency/cycle/specialization traversal, and handler effects/recovery propagate through the existing summaries.

HTTP accepts unique Tokio streams through the existing Listener owner before constructing any aliased public Socket. Existing AsyncStream/Group reserves capacity before accept, observes connection faults while idle, and drains descendants on stop. Hyper never spawns a detached connection driver. Client send polls driver and response in the same bounded future and releases both on completion/cancellation; pooling is deferred.

Bytes uses bytes::Bytes for shared engine storage, slicing and compatible Text ownership transfer. Bodies are buffered within explicit limits, merging fragments incrementally without retaining one metadata entry per tiny frame. Header, body, handler and stalled-write deadlines have separate owners. The public contract and exact protocol exclusions are in LANGUAGE.md; TLS/HTTP2, streaming responses, client pooling and WS/SSE are not N2 deliverables.

## N1 Async Stream And TCP (2026-09-12, Approved)

This new-language phase supersedes the historical compatibility baseline and network pause below. Migrate callers directly; no synchronous TCP backend, aliases or fallback paths.

Use one existing Tokio runtime with its I/O driver enabled. AsyncStream owns one Send stream producer, serializes pulls, and shares a close signal across aliases. Closing or finishing a consumer releases its producer; there is no unbounded buffer or per-item boxed future. Stream remains the distinct synchronous file-sequence type.

Socket and Listener share explicit resource closure. Socket read and write have independent async locks; closure wakes both. A write tracks partial progress and closes the socket if abandoned after writing any bytes, so subsequent operations cannot append to a truncated message. Operation timeout includes waiting for its direction lock. No retry of an entire interrupted write.

AsyncStream traversal may call synchronous effect-checked handlers or async handlers. parallel_each on AsyncStream requires an async zero-output handler and uses the existing structured Group. It checks completed tasks and acquires group capacity before pulling, and observes handler failures even while the producer is idle. No new detached task model or private thread pool.

Compiler types, effects, liveness, native specialization, official network API and existing HTTP transport move together. The synchronous reference evaluator explicitly excludes async execution; no second network implementation is retained for differential tests.

## Structured Concurrency Design And Implementation Contract (2026-09-12)

This preceding phase is implemented through the compiler frontend, checked HIR, native backend and bounded runtime, with evidence in `implement.md`. It superseded the first-stage no-async model. The current N1 decision permits direct source-contract changes without a compatibility baseline.

### One Task Model

Dever has one concurrency model rather than separate async, coroutine and thread abstractions:

- an async function compiles to a stackless coroutine;
- `await` suspends that coroutine without occupying a worker thread;
- `run` schedules the coroutine as a child Task;
- the runtime may move transferable tasks across worker threads;
- `parallel` and `blocking` cross from async scheduling into bounded synchronous execution.

The program creates one Tokio runtime at an async entry point. A program whose reachable entry graph is entirely synchronous retains the current direct native entry and does not initialize Tokio. The runtime configuration owns async worker count, blocking capacity and task limits once; packages do not create private runtimes or pools.

### Source Surface

`async` prefixes a function clause. Every clause in one logical function must agree on this modifier:

```dever
async load_user(id: Id) (user: User) {
  user = await(fetch_user(id))
}
```

An async call is context-only, not an ordinary storable future. It may appear directly under `await`, `run` or the matching compiler-owned task operation. This avoids adding a general `Awaitable<T>` value and preserves the current static-call model.

```dever
first = run(load_user(first_id))
second = run(load_user(second_id))
first_user = await(first)
second_user = await(second)
```

`await(async_call(...))` invokes and waits without creating a separately owned Task. `run(async_call(...))` starts first and returns an affine Task whose associated output signature remains in checked HIR. `await(task)` consumes it and yields the same zero, single or named multi-output shape as the function. Task operations consistently use the language's existing call appearance; no second prefix spelling is accepted.

Static handler syntax places `async` before `handler`:

```dever
async serve(
  route: async handler(request: Request) (response: Response)
) () {
  response = await(route(request))
}
```

`async` has one canonical declaration position immediately before the function name. Existing `pure` and `recover` contracts retain their post-signature positions. Parser, formatter, Markdown signatures and examples must change atomically; no postfix async spelling is accepted.

### Task Ownership

Task is a compiler-owned affine local type, not a new user-defined generic facility. It may be held only in a local slot of the async function that created it. It cannot be copied, compared, stored in an aggregate, passed as an ordinary value, returned, captured by a handler or left live at any clause exit.

Each async entry and spawned task has an internal runtime Scope; the compiler treats every async function as a lexical ownership scope. Directly awaited calls inherit the current Scope because runtime faults cannot be caught, while `run` and Group create a supervised child Scope. Every Task must be consumed exactly once by `await` or `stop`. On abnormal runtime exit, the root or child supervisor requests cancellation and waits for descendant and blocking-work cleanup before returning the failure. There is no detached task in the first release.

`stop` means cancel-and-wait, not fire-and-forget cancellation. To keep failure and output obligations explicit, the first release accepts only zero-output Task values. Result-bearing tasks must be awaited. Cancellation is cooperative at await/yield boundaries; it neither kills a running synchronous operation nor rolls back effects that already occurred.

### Dynamic Task Groups

`group(limit)` creates an affine Group for a dynamic number of zero-output async calls. It is needed for later connection/session ownership without allowing detached children:

```dever
workers = group(128)
run(workers, serve_connection(connection))
await(workers)
```

`run(group, async_call(...))` transfers the child into the Group and returns no Task. It may suspend while the positive capacity limit is full, providing backpressure before starting more work. Ungrouped `run(async_call(...))` is likewise subject to the owning function scope's task limit. `await(group)` closes admission, waits for all children and consumes the Group. `stop(group)` closes admission, requests cancellation, waits for every child and consumes the Group. Group cannot be copied, stored or escape its creating async function.

Only zero-output calls enter a Group, so the runtime cannot accumulate unbounded result values and expected failures cannot be silently discarded. A later result-collecting combinator must define ordering and memory limits separately.

### Thread Transfer And Execution Classes

The type model derives a `transferable` property recursively, alongside the existing comparison/move/transfer properties. Bool, numeric values, Text, Id and Bytes are transferable. Records, choices, nullable values, Lists and Maps are transferable only when every nested value is transferable. Each opaque resource is opted in only when its runtime representation and lifecycle are thread-safe; the current pull-based Stream is initially non-transferable.

Values live across an `await`, captured by `run`, or passed through channel/parallel boundaries must be transferable. Last-use analysis may move uniquely owned values into a task; otherwise ordinary value semantics require a copy. This rule is checked before native emission and is not delegated to a Rust compiler error.

Execution classes remain distinct:

- ordinary synchronous calls execute inline;
- `run` schedules an async call on the shared runtime;
- `parallel(sync_call(...))` accepts only a pure synchronous target with transferable inputs/outputs and runs it through bounded CPU execution;
- `blocking(sync_call(...))` accepts a synchronous target whose transitive effect summary contains a compiler-known blocking system boundary and uses bounded blocking capacity.

`parallel` and `blocking` are async-context operations in the first release: they suspend their caller until the synchronous call completes. Synchronous code continues to use `parallel_each`; it does not initialize Tokio solely to access these operators.

Existing synchronous `parallel_each` keeps its public behavior and joined scoped workers. Async List/Bytes lowering requires a named synchronous function with no transitive concurrency effect, then submits bounded batches directly to the shared Tokio blocking executor, so it neither blocks a Tokio scheduling worker nor creates a nested thread pool. Other collection operations inspect the complete handler effect and cannot hide blocking, concurrency or an unconstrained handler effect on a Tokio scheduling worker. The first release has no handler effect bounds, so an async function cannot invoke a synchronous handler parameter inline. Both parallel paths share worker-limit and batch-size validation. Synchronous Stream remains unavailable in async functions until async Stream is designed.

### Channels

`channel(Type, capacity)` is compiler-contextual syntax: its first operand is a TypeRef, not a runtime type value. It creates a bounded `Channel<Type>` resource. Capacity is required, positive and capped. This deliberately avoids adding generic function invocation syntax solely for one constructor.

Channel aliases share one state. `send(channel, value)` asynchronously waits for capacity; `receive(channel)` asynchronously waits for an item and returns `Item?`, where a value is an item and `null` means the Channel is closed. Because nullable types are idempotent, receiving from `Channel<Item?>` also returns `Item?`; native emission flattens the runtime's nested option. A runtime scheduler or Channel fault remains a located fatal execution failure rather than a domain value. `close(channel)` is idempotent and wakes blocked senders and receivers. No unbounded constructor exists. Values must be transferable. Channel itself cannot be a Map key or comparable value, and its close/alias behavior follows explicit resource semantics rather than ordinary value semantics.

### Effects, Failures And Static Handlers

Async is part of a function and handler signature. Sync callers cannot invoke async functions or async handlers. Async callers may invoke sync code directly and async code only through an explicit async operation. Collection handlers remain synchronous because collection operations do not await handler futures; async handlers are called only through `await` or `run`. Handler specialization keys include the async signature bit, and cycle detection continues across await/run/forwarding edges; async does not legalize recursion.

`await`, `run`, `stop`, group operations, channel waits, timers, `parallel` and `blocking` are effects. The existing transitive effect graph owns these facts, so the first release rejects `async` together with `pure`. Task boundaries do not discharge error-bearing outputs: awaiting restores the original output obligations, while Group and stop accept only zero-output work. Runtime scheduler failure is a located fatal execution failure; expected domain failures remain explicit source choice values.

### HIR And Native Boundaries

The syntax tree records the async modifier and contextual concurrency expressions. Checked HIR owns explicit `AwaitCall`, `RunCall`, `AwaitTask`, `StopTask`, Group and Channel operations. It never represents a generic user-visible Future. Function metadata and HandlerSignature carry the async bit; Task stores its concrete checked output fields.

Native emission generates concrete Rust futures and direct calls. Static handlers remain specialized direct targets; no boxed dynamic callback, generic runtime Value, reflection registry or garbage collector is introduced. The runtime owns scheduler construction, task/group/channel state, limits and cancellation primitives. Compiler checks own source legality, transferability, affine consumption and diagnostic locations.

Runtime cancellation separates a business future from its supervisor. Task/Group stop aborts the business future; the supervisor itself remains alive, drains its child Scope, and only then publishes completion to the parent Scope. Started `spawn_blocking` jobs register a sticky completion signal because Tokio cannot abort them safely. Drop paths request cancellation and detach only supervisor handles whose completion is still owned and awaited by the parent Scope; no Drop implementation calls `block_on`.

### Compatibility And Deferred Work

Existing synchronous source and `parallel_each` behavior stay valid. `LANGUAGE.md`, Markdown contracts and API snapshots are updated with the implementation. The `run` source operator is distinct from the CLI command by grammar context.

The task-foundation phase deferred networking; N1 above resumes AsyncStream and TCP on these primitives. The HTTP engine and WebSocket/SSE remain later work. Detached tasks, arbitrary shared mutation, locks, user thread creation, unbounded channels and forceful cancellation remain out of scope.

## Markdown Source Follow-through (2026-09-09)

`.dever.md` is an additional source container alongside `.dever`. Top-level literal `dever` and `typescript dever` fences contain ordinary Dever declarations; the latter supplies a TypeScript highlighting hint while retaining an explicit Dever marker. A dedicated Markdown contract parser maps one H1 to the package header and one H2 to each type or logical function group before formatting. After shared signature resolution, semantic validation compares package/API/use metadata, type fields/variants and function inputs/outputs with syntax and HIR, retaining both documentation and declaration spans in diagnostics. Shared fence extraction keeps original spans for parsing, diagnostics and per-block formatting. `.dever` and language/runtime semantics are unchanged. The authoritative source-format contract is `LANGUAGE.md` and `.trellis/spec/backend/toolchain-and-library.md`; earlier `.dever`-only and unconstrained-heading limits below are superseded.

## Performance Implementation Boundary (2026-09-08)

- Compiler analysis worker owns types.rs, check.rs integration and contracts dependency scheduling: memoized type properties and monotone reverse-dependency work queues, retaining handler substitution and proof limits. Existing graph traversal is reused.
- Cache worker owns source/parse caching, native/build.rs and its cache helper plus CLI cache integration if necessary. Cache identities include effective source, compiler/toolchain/runtime and options; checked API baselines stay outside cached native execution. No new external dependency or source format.
- Main owns native liveness/emission, runtime collections, JSON/HTTP source algorithms, performance fixtures and integration. Record updates preserve value aliases and evaluation order; consuming iteration retains resource scope; direct split consumption avoids eager character lists; HTTP parsing tracks incremental boundaries and framing in Dever.
- Root tests own semantic regressions and bounded before/after measurements; no services/full suite/commit. Workers are not alone and must preserve unrelated edits. Shared check.rs edits require coordination with the analysis owner. Independent final review checks values, failure order, cache invalidation and protocol bounds.

## Follow-through Implementation Design (2026-09-07)

This section owns the current follow-through; older extension boundaries below describe completed milestones.

- Formatter: add an AST writer using existing spans, preserved comments and precedence. Preserve record initializer expression order. CLI owns filesystem mutation and prepares every result before any rewrite. Expose syntax-only formatting separately from semantic checking.
- Reference evaluator: add an opt-in `reference` compiler feature, not a runtime dependency. Consume checked HIR, normalized domains and static handler bindings; reuse runtime number/resource primitives. Frames hold value locals and preserve copy semantics. Capture stdout and named results for differential tests. Do not add an evaluator production CLI path.
- Standard library: extend the single intrinsic/checker/native catalog for atomic representation/OS operations only. Public result choices live in `library/`; share runtime implementations with the evaluator. Text indexes are Unicode scalar positions; Bytes indexes remain byte offsets. Number parse failures are nullable results, never a silent zero; finite Decimal rules remain unchanged.
- Optimization: analyze actual read order and writes per clause, retaining outputs and assignment hosts. Move dead values, preserve live aliases and short-circuit paths; transfer reduction state between iterations. Verify value semantics before performance comparison.
- Parallel ownership: formatter worker owns formatter modules, formatting CLI and root format tests; reference worker owns evaluator modules and root differential tests. Main agent owns libraries, primitive integration, native optimization, task documents and integration. Coordinate shared module registration edits.
- Verification uses offline focused targets and test-owned files/processes. Performance checks are bounded local computation, exclude compiler startup and use matching numeric/collection wrappers. Existing user sources are never formatter test fixtures.
- Production native compilation and matching performance baselines share one optimization-argument owner (`opt-level=3`, `codegen-units=1`). The single generated source compiles as one unit to keep generic collection inlining independent of source size; compilation time is measured separately from hot paths.

- Status: Core foundations and foundation extension implemented and verified on 2026-09-07
- Date: 2026-09-04
- Product source of truth: `prd.md`

## Completed Foundation Boundary (Historical)

Extend the implemented core with the approved sequence and resource foundation: `reduce`, `reduce_until`, Bytes traversal, `Stream<Item>`, restricted static handler inputs, and file/TCP stream wrappers. Existing clause-only branching, linear bodies and the ban on source loops and user recursion remain permanent. Compiler distribution, cross-platform work and full HTTP/JSON/CRUD packages remain deferred.

Extend the existing checker and HIR in place. Represent user types nominally, resolve every function signature before bodies, prove clause coverage over symbolic input domains, and emit concrete native types/functions. Keep source expression order, including record construction; formatting must not reorder effectful field expressions. Use one runtime implementation for numeric operations, ordered collections and resource primitives. Bundled `.dever` sources define official wrappers and result types through the same frontend as user packages; only fixed primitive signatures have native implementations.

## Foundation Extension Design

This section records the completed foundation-extension design. The Follow-through Implementation Design above owns subsequent additions; later numbered sections retain the original Core roadmap.

### Sequence Contract And Reuse

The checker extends the existing collection-operation owner rather than adding parallel validators. One internal sequence classifier returns the element type and source kind for `List<Item>`, Bytes and `Stream<Item>`; `each`, `reduce` and `reduce_until` reuse it for handler checking and lowering.

- `reduce(handler, sequence, initial_state)` requires a two-input handler `(Item, State) -> State` and returns `State`.
- `reduce_until(handler, sequence, initial_state)` requires `(Item, State) -> (State, stop: Bool)`, processes the current item before stopping, and returns only `State`.
- List and Bytes use finite direct loops. Bytes yields Int values from 0 through 255 without allocating `List<Int>`.
- Stream consumption calls its pull boundary only when the next item is needed. `reduce_until` leaves the next item unread; `reduce` consumes through natural exhaustion.
- `each(Stream)` accepts only a zero-output handler. List/Bytes retain their existing zero-output action and one-output mapping behavior. `filter`, `find` and `sum` remain List-only.
- Core `close(stream)` is a zero-output, idempotent Stream operation. It shares the same sequence operation registry but is checked as resource termination, not traversal.

Handler signature diagnostics, sequence classification and output-shape validation each have one semantic owner. Native lowering receives already classified sequence and handler facts; it does not repeat source-level type decisions.

### Static Handler Representation

`handler(inputs) (named_outputs)` is contextual syntax allowed only after a function input colon. The syntax tree represents value patterns and handler parameter declarations as separate input kinds; it does not disguise a handler as an ordinary `TypeRef`. The semantic model likewise keeps `HandlerSignature` separate from runtime `Type`, because a handler is a compile-time binding rather than a value.

`HandlerSignature` stores positional input types and ordered named output fields. Input names do not participate in compatibility; output name, position and type all do. The checker accepts only a matching named function or a handler parameter already in scope. Handler references cannot enter local slots, aggregates, outputs, comparisons or clause domains.

HIR uses explicit call and argument forms:

```text
HandlerTarget = NamedFunction(FunctionId) | Parameter(HandlerParameterId)
CallTarget    = NamedFunction(FunctionId) | Handler(HandlerTarget)
CallArgument  = Value(Expression) | Handler(HandlerTarget)
```

Collection/sequence HIR stores the same `HandlerTarget`; it no longer carries a collection-specific optional function ID. This is the reusable representation for direct handler calls, sequence operations and multi-level forwarding.

Native compilation treats a function with handler inputs as a template. A specialization key is the function ID plus its ordered concrete named-function bindings. Forwarding substitutes bindings until every handler call has a concrete target, then emits an ordinary direct call. The specialization graph rejects a repeated active key as direct or indirect recursion before any private Rust source is compiled. Generated handler plumbing contains no function pointers, closures, virtual dispatch or reflective registry; the opaque Stream producer described below is the only dynamic call boundary.

### Stream Representation And Lifecycle

The semantic type model adds `Stream(Box<Type>)`. It is the only new compiler-provided generic type; it is a valid parameter, local, field and choice payload but is not comparable and cannot be a Map key. Copying it preserves opaque resource semantics: every alias shares one cursor, exhausted flag and explicit-close flag.

`dever-runtime` adds one generic pull-stream owner backed by shared synchronized state. Its producer returns either one item or natural end. The only dynamic producer abstraction is inside this opaque I/O boundary, where the PRD explicitly permits it; Dever handlers remain statically specialized. Pulling and closing are serialized, with no prefetch, worker thread or async task.

The terminal rules are:

1. Normal EOF marks the Stream exhausted and releases its producer without yielding an element.
2. A terminal file/socket read error is converted once to `ReadEvent.Failed`, then the Stream becomes exhausted.
3. A terminal accept error is converted once to `ConnectResult.Failed`, then the Stream becomes exhausted.
4. `close(stream)` marks all aliases closed, drops the producer and is a no-op when repeated; later traversal sees ordinary exhaustion.
5. Closing a separately held File, Socket or Listener invalidates the shared underlying resource. The dependent Stream then yields its one typed Failed item and ends.
6. Closing only the Stream drops its source alias but does not close a separately held source handle. Native destruction remains the final cleanup fallback.

`reduce_until` never pre-pulls, so its stop item is consumed and the following item remains available to every Stream alias. Concurrent consumption is outside the language contract; the synchronized state protects resource integrity but does not introduce concurrency semantics.

### Official Package And Runtime Boundary

The public stream API remains ordinary bundled `.dever` source:

```dever
type ReadEvent {
  Chunk(bytes: Bytes)
  Failed(message: Text)
}

type ReadStreamResult {
  Streaming(stream: Stream<ReadEvent>)
  Failed(message: Text)
}
```

`dever.io.chunks(file, limit)` and `dever.net.chunks(socket, limit)` return `dever.io.ReadStreamResult`; `dever.net.connections(listener)` returns `Stream<dever.net.ConnectResult>`. Existing one-shot `read` remains unchanged. Invalid limits and already-closed sources fail during stream creation; later source failures become one terminal stream item.

The ownership flow remains:

```text
bundled .dever function and result choices
        -> fixed dever.system intrinsic signature
        -> generated typed adapter
        -> dever-runtime Stream/resource primitive
```

The generated adapter maps runtime pull outcomes (item, natural end or error) into the source-defined concrete choices. Runtime code therefore owns pulling, locking and OS resources, while `.dever` owns public names and recoverable result shapes. No HTTP parser, route table, JSON codec or user model enters the runtime.

### Implementation And Rollback Boundaries

Implementation proceeds through coherent compiler boundaries: syntax/semantic handler declarations; shared handler and sequence HIR; List/Bytes reduce lowering; runtime Stream; file/TCP adapters; then native specialization and end-to-end fixtures. Each boundary must compile and pass its focused tests before the next. Unsupported handler or Stream positions remain compile diagnostics; there is no compatibility fallback to runtime values or eager Stream collection.

The existing one-shot file/TCP APIs, List operations and generated-binary boundary remain compatible. No stored data or public artifact format needs migration. Reverting the extension removes its syntax, standard source declarations, HIR forms and runtime module together while leaving the completed core foundations intact.

## Historical Hello Milestone (2026-09-06)

The completed deliverable is a native `user.dever` whose `main() ()` prints `hello`. A future user API may start with in-memory fixtures; database selection is no longer a prerequisite. Existing parser and surface syntax are preserved.

The implemented Text/action subset covers package and function resolution, Text parameters and local assignment, zero-output calls, checked HIR, native Rust emission and local check/build/run. It checks all source declarations and rejects unsupported constructs, duplicate symbols, invalid exposures, private cross-package calls, source/package mismatches and cycles. This is not full Core checking.

The standard `dever.io.println(Text)` signature resolves to a typed intrinsic. Its implementation belongs to a small dependency-free `dever-runtime`; the native backend embeds that same runtime source in disposable generated Rust. Text uses owned Rust Strings to preserve Dever value semantics. User function and local identifiers lower to numeric IDs, not raw Rust identifiers. Output failures propagate internally to a nonzero process exit with the Dever call location; no new source exception syntax is introduced.

The CLI owns argument parsing and process execution; the core owns checked programs and native compilation. Invoke rustc directly without a shell, using RUSTC or PATH. Compile into an exclusively created temporary directory, retain it through execution and remove owned artifacts afterwards. Explicit build output must not overwrite an existing file. No external dependency, package distribution, interpreter or service is needed.

The following sections describe the wider roadmap. Implemented foundations are recorded in the Active Core Checklist and Core Foundation Evidence in `implement.md`; formatter, reference evaluator, performance gates and application packages are not part of this delivery.

## Deferred Application Architecture

This is a later HTTP/JSON/CRUD direction, not part of the approved foundation extension. It must be reconsidered against the implemented Stream and static-handler contracts before application work begins.

The native path remains `.dever -> parser -> resolver/checker -> typed HIR -> private Rust -> native executable`. HTTP requests must execute the compiled business functions, not an interpreter or a handwritten Rust user service.

| Owner | Required responsibility |
| --- | --- |
| `.dever` application packages | User/input types, validation, CRUD orchestration, parameterized queries, routes and business error mapping |
| `dever-core` | Symbol/type/clause/initialization checks, standard signatures, I/O effects, typed handler checks and native code generation |
| `dever-runtime` | Mature HTTP and JSON implementations, the selected database driver, connection/resource lifecycle and per-request fault boundary |
| CLI | Configuration inputs, `check`, native `build/run` and diagnostics |

Necessary semantic adaptations, with no new surface keywords:

- Allow named function references in fixed HTTP registration positions, extending the existing collection-handler mechanism. They remain statically resolved, not general first-class functions.
- Add opaque standard resource types for database pools and server resources. Ordinary records/collections keep value semantics; resource handles explicitly identify external state and cannot be serialized or mutated as records.
- Give standard I/O operations fixed effect metadata and propagate effects through the call graph. Preserve statement/expression evaluation order; internal async lowering does not introduce `async` or `await` into `.dever`.
- Reuse choice results for recoverable HTTP/JSON/database failures. Program faults are isolated to the failing request and retain Dever source locations.
- Bundle only the required standard HTTP/JSON/database contracts with the toolchain. External package distribution remains deferred.

The current `TypeRef` AST can represent parameterized standard types, but `parser.rs::type_ref_inner` presently recognizes only List/Map/MapEntry generic spellings. Any required qualified standard generic type must be supported through the standard-type contract, without a User-specific parser branch. User-defined generics remain deferred.

Persistence selection is pending. The initial design candidate uses parameterized SQL and static type-driven row decoding, with schema/query definitions owned by one application persistence package. Do not build an ORM or multi-database framework speculatively. JSON and row decoding should share resolved type facts, rather than duplicate user field definitions in Rust.

Verification must show that changing `.dever` handlers, types or routes changes the compiled API. A second small schema/route fixture should prove that compiler/runtime code contains no user-API special case. Report actual CRUD throughput, latency percentiles, errors and memory against equivalent native code under identical database/durability settings; finalize the performance budget after storage and deployment scope are selected.

## 1. Architecture Decision

Core 0.1 is a textual language toolchain, not a semantic-graph workbench. It has one checked frontend and two deliberately different consumers of the same typed HIR:

```text
.dever source root
      |
      v
source loader -> lexer -> parser -> syntax tree
                                |          |
                                |          +-> formatter
                                v
                    package/name resolver
                                v
                type + clause + flow checks
                                v
                         typed HIR
                         /       \
                        v         v
          reference evaluator   native specialization
                                      v
                              private Rust emission
                                      v
                              rustc / LLVM -> binary

dever CLI -> load / check / format / build / run
```

Dever text in `.dever` files or program blocks of `.dever.md` files is the program source. The syntax tree and typed HIR are in-memory compiler representations; they are not editable JSON formats or public compatibility contracts. The evaluator defines a simple executable reference for conformance tests. Production `build` and `run` use ahead-of-time native compilation and never route through the evaluator.

The bootstrap compiler is implemented in Rust because rustc provides a mature LLVM path. The repository starts without an existing workspace or implementation. This is an implementation choice, not a surface-language dependency: diagnostics, type identities and runtime semantics are owned by Dever, and generated Rust is a disposable private build artifact.

## 2. Scope Boundaries

The implementation contains three production crates:

```text
crates/
  dever-core/
    src/
      source.rs
      diagnostic.rs
      token.rs
      lexer.rs
      syntax.rs
      parser.rs
      format.rs
      package.rs
      resolve.rs
      types.rs
      clauses.rs
      check.rs
      hir.rs
      value.rs
      stdlib.rs
      eval.rs
      native.rs
      lib.rs
  dever-runtime/
    src/
      number.rs
      collections.rs
      fault.rs
      lib.rs
  dever-cli/
    src/
      main.rs
```

Compiler phases remain in one `dever-core` library rather than separate syntax, semantic and IR crates. They share source spans and language contracts and will evolve together during 0.1. `dever-runtime` is the only justified split because generated native programs consume it without depending on parser, checker, evaluator or CLI code. It contains only language-owned numeric, ordered-collection and runtime-fault primitives; it does not contain a dynamic interpreter or plugin layer.

`dever-cli` owns argument parsing, filesystem invocation, exit codes and rendering. It calls `dever-core` APIs and contains no parser, type or evaluation rules.

Permanent integration tests stay under repository-root `test/`, matching project policy. Plain-source examples live under `examples/dever/`; Markdown examples live under `examples/markdown/`. Both may contain small fixture data when needed.

## 3. Reuse And Replacement

### Reuse

- Pinned dependency discipline in the new Rust workspace.
- Small modules and explicit data structures.
- The idea that every diagnostic maps back to a source location.
- Root-level integration tests and deterministic fixtures.

### Replace

- `SemanticGraph`, canonical JSON loading and semantic hashing.
- Refiner candidates, approval, scrap and quarantine workflows.
- Dever IR designed around the old graph.
- Old target-facing Rust and React source generators and their public backend contracts.
- Workbench presentation and temperature-alarm example.
- Old child-task plan and old repository rules that call the semantic graph trusted source.

No adapter translates old JSON into new `.dever`. Such a bridge would preserve the rejected architecture, expand the change surface and create two sources of truth.

The replacement list describes rejected concepts, not files present at kickoff. No old implementation exists in the working tree or HEAD. Initialize `dever-core` and the root test crate first; add the runtime and CLI when their real consumers exist, without empty crates or partially working public commands.

## 4. Source Model

### 4.1 Source Files And Spans

`SourceFile` owns:

- canonical source-root-relative path;
- UTF-8 text;
- precomputed line-start offsets.

`Span` contains a file identifier and half-open byte range. Line and column are calculated only when rendering a diagnostic, keeping compiler nodes small. Every token, syntax node, resolved reference and HIR instruction retains a span.

The loader:

1. walks the supplied source root for `.dever` files;
2. sorts paths lexically before parsing;
3. rejects invalid UTF-8 and duplicate canonical paths;
4. derives the expected package name from the path;
5. returns all diagnostics in deterministic file/span order.

### 4.2 Tokens And Trivia

The lexer emits semantic tokens plus newline and `#` comment trivia. Newlines matter for separating linear statements; whitespace otherwise does not. Comments attach to the following syntax item, or to the preceding item when trailing on the same line, so the formatter can preserve them. `//` is always one integer-division token and is never considered comment syntax.

Numeric text remains unconverted in tokens. Parsing a very large literal directly into i64 or f64 would lose the ability to issue precise range and rounding diagnostics; conversion happens during checking with the expected type. An inexact Decimal literal is rejected rather than silently rounded, while checked Decimal arithmetic may produce the specified rounded result.

### 4.3 Syntax Tree

The parser produces a lossless-enough typed AST:

- `PackageDecl` and exposed names;
- record/choice `TypeDecl`;
- `FunctionClause`;
- input patterns and output declarations;
- assignment/call statements;
- value construction, call, field, literal, unary and binary expressions.

The parser does not resolve names or infer types. It supports bounded recovery at a newline, closing delimiter or next top-level declaration and returns a partial tree plus diagnostics. Later phases run only for packages without syntax errors.

## 5. Formatter

The formatter consumes the syntax tree and attached comments, never performs string replacements over raw source, and owns one layout:

- two-space indentation;
- multiline package exposure and type members, one item per line;
- commas only where grammar requires positional separation;
- no semicolons;
- stable spaces around operators;
- record fields in declaration order after successful resolution;
- parentheses inserted only when required by precedence;
- one blank line between top-level definitions and no trailing whitespace.

`format(format(source)) == format(source)` is an acceptance invariant. Invalid files are not rewritten in Core 0.1; the CLI reports diagnostics and leaves them untouched.

## 6. Resolution And Symbols

Resolution runs in three passes:

1. Register every package, type, function name and function arity.
2. Register record fields, choice variants and full function signatures.
3. Resolve bodies, patterns, exposed names and cross-package references.

Symbols use compiler-assigned integer IDs internally. User-visible identity remains the package-qualified source name, not an opaque semantic graph ID.

The resolver enforces:

- one file per package and exact path/package match;
- local source plus the fixed compiler-owned standard catalog as the only Core 0.1 package origins;
- reserved core single-segment names and the `dever.*` namespace, which local source cannot declare;
- unique names inside a package;
- one function signature per name/arity;
- contiguous clauses for the same signature;
- public access only through `exposes`;
- full qualification across packages;
- acyclic package, type and function-call graphs.

Resolution never performs network access or searches outside the supplied source root in Core 0.1. An unavailable official or third-party package is an ordinary unknown-package diagnostic. Future lockfile-pinned components will be registered as an additional origin before these same resolution passes; this does not require imports, aliases or version text in `.dever` calls.

### 6.1 Distribution Compatibility Contract

External distribution is deferred to 0.2, but Core 0.1 must not close off its confirmed identity model:

- a stable component ID such as `shemic.contact` owns the root of package and type identity;
- registry, Git and local paths are replaceable source transports and never appear in `.dever` references;
- Git additions require an explicit tag or revision, then lock the full commit and normalized SHA-256 content hash;
- CLI-maintained `dever.mod` records direct dependency intent while `dever.lock` records the complete exact graph;
- verified sources live in a global read-only content-addressed store shared across projects;
- only explicit `add`, `update` and `fetch` operations may access the network;
- every package supplied by a component must remain under that component ID, and duplicate providers are errors.

No 0.2 parser, Git client, resolver, store or command is implemented speculatively in Core 0.1. The current resolver needs only reserved namespaces and origin-neutral internal symbol IDs so adding a locked package origin later does not change source names.

The ordinary call graph is computed after all bodies resolve. Sequence handlers and handler-input forwarding are explicit HIR edges; native specialization checks the fully substituted instance graph, so indirect recursion cannot hide behind `each`, `reduce`, `reduce_until` or a handler parameter.

## 7. Type System

`TypeId` points to interned compiler types:

- the six scalar types;
- user record and choice types;
- `Nullable(TypeId)`;
- `List(TypeId)`;
- `Map(KeyTypeId, ValueTypeId)`;
- compiler-owned `MapEntry(KeyTypeId, ValueTypeId)`;
- compiler-owned `Stream(ItemTypeId)` opaque resources;
- ephemeral named multi-output result types.

There are no user-defined generic parameters in 0.1. List, Map, MapEntry, Stream and sequence functions are compiler-provided generic contracts. Static `HandlerSignature` is a separate function-parameter contract and is not a runtime `Type`.

The type interner owns idempotent nullable construction for standard generic returns; explicit doubled `?` syntax remains invalid. Structural equality follows the PRD, including ordered Map pairs and IEEE Float comparisons inside aggregates. Checking, reference evaluation and native lowering must share these contracts.

Checking is expression-directed:

- function inputs and outputs provide declared boundary types;
- assignment provides an expected type once the local is initialized;
- an uninitialized local takes the inferred type of its first complete assignment;
- empty List/Map and context-sensitive numeric literals require an expected type;
- the only implicit numeric conversion is Int to Decimal;
- function output, static handler and sequence-operation contracts are checked before lowering.

Linear bodies make definite assignment a forward pass. The checker tracks each local as uninitialized or initialized with one stable type. Record field writes require an initialized record and preserve its type.

## 8. Clause Analysis

Clause-based conditional dispatch is a permanent source-language constraint, not a Core 0.1 deferral. Native decision trees are its internal implementation and do not introduce another source-level branching form.

Each same-name/same-arity group becomes one `FunctionDef` with multiple clauses. Analysis separates base input types from clause patterns.

Patterns normalize into symbolic sets:

- Bool and no-payload choice types become finite atoms.
- Nullable adds a distinct null atom.
- Payload choices keep the variant atom and bind payload fields after selection.
- Text/numeric constants become singleton sets.
- Int and Decimal comparisons become normalized intervals with open or closed endpoints.
- `other` is computed as the complement of all explicit patterns in that argument position and group.

A multi-input clause is a product of its per-input sets. The analyzer incrementally subtracts each product from the function input universe:

- intersection with prior coverage means overlap;
- an empty clause domain means unreachable;
- remaining universe after all clauses means non-exhaustive.

The algorithm works on finite atoms and interval boundaries, not enumerated i64 or Decimal values. Float range patterns are rejected before this phase. The source order is never part of selection.

At runtime, HIR stores a compiler-chosen decision tree built from the normalized domains. It can optimize tests while preserving order-independent semantics; no runtime “first matching clause” rule exists.

## 9. HIR And Reference Evaluation

The typed HIR contains only resolved IDs and typed operations:

- constants and value construction;
- local/field load and store;
- checked numeric/text/boolean operations;
- direct function and statically bound handler calls;
- standard collection/sequence calls with explicit handler targets;
- clause decision nodes;
- named outputs.

The evaluator uses an explicit call frame with owned locals and a source span for every operation. There is no global mutable program state.

Runtime values are:

```text
Bool
Int(i64)
Decimal(DecimalValue)
Float(f64)
Text
Id
Record
Choice
Nullable
List
Map
Stream
MultiOutput
```

Aggregate values use shared immutable storage with copy-on-write mutation. This implements source-level value semantics without eagerly cloning every nested List, Map or record. The optimization is internal and must pass alias-observation tests.

Map storage uses an insertion-ordered map. In Rust, `indexmap::IndexMap` is the planned implementation; replacement keeps the same wrapper contract. Removal must use order-preserving removal, and overwrite must retain the existing position.

Evaluation returns either named outputs or a `RuntimeFault`. A fault carries category, message and originating `Span` and unwinds only to the current CLI execution boundary. There is no language-level catch path.

The evaluator is intentionally not optimized as the production runtime. Its job is to make language semantics easy to inspect, to evaluate focused tests and to serve as a differential oracle for native output.

## 10. Native AOT Compilation

The native path consumes only fully checked typed HIR. It performs whole-program specialization from the selected zero-input entry function and emits one transient Rust crate:

```text
reachable typed HIR
       |
       v
specialize concrete List/Map/Stream and handler-binding combinations
       |
       v
emit static Rust records, enums, functions and loops
       |
       v
cargo build --release -> native executable
```

There is no generic `Value` in generated function signatures. Dever records lower to concrete Rust structs, choices to concrete enums, nullable values to an internal option representation, and calls to direct functions. Clause domains lower to compiler-selected decision trees. Because sequence handlers are concrete after specialization, `each`, `reduce`, `reduce_until`, `filter`, `find` and `sum` lower to direct calls and loops. List/Bytes loops have no virtual dispatch; only opaque Stream pulling may use the internal producer abstraction at the I/O boundary.

Source-level value semantics do not require eager deep copies. A last-use/liveness pass may move a value when the source is dead; when both aliases remain live, generated aggregates use a language-owned clone or copy-on-write representation. This is invisible to `.dever` and is verified against the evaluator. Locals and non-escaping records remain eligible for normal Rust/LLVM stack and register optimization. The AOT runtime has no garbage collector and no reflection registry.

Checked Int and Decimal behavior is emitted explicitly, so the release profile cannot remove language-required overflow or invalid-operation faults. Runtime helpers carry compact source-location IDs; the executable maps a fault back to the original `.dever` path and span. Internal Rust compiler errors are toolchain failures, not Dever diagnostics, and generated Rust names must never appear as user symbols.

The transient crate depends only on `dever-runtime`. Its generated source and Cargo metadata live in a compiler-managed build directory, are not emitted by a public command and may be deleted at any time. The optimized profile starts with `opt-level = 3`, thin LTO and one codegen unit; benchmark evidence may change those internal settings without changing the language.

The first backend deliberately uses stable Rust as a bootstrap bridge to LLVM. A later direct LLVM or Cranelift backend can consume the same checked HIR and runtime contracts without changing `.dever` source. No `dever-backend-rust` public crate or target-specific surface syntax is introduced.

Self-hosting is a staged replacement, not a Core 0.1 syntax promise:

1. The Rust bootstrap compiler establishes parser, checker, HIR, native semantics and conformance fixtures.
2. Later language phases add the explicit file/process/memory capabilities required to write compilers.
3. The frontend and standard library are reimplemented in `.dever` against the same fixtures and HIR contracts.
4. A stage-1 compiler builds stage 2; stage 2 builds stage 3; stage-2/stage-3 behavior and reproducible artifacts are compared before the Rust frontend becomes only a seed.

## 11. Numeric Implementation

`dever-runtime::number` is the only module allowed to depend on the decimal implementation. It exposes a language-owned `DecimalValue` and operations returning:

```text
Result<DecimalValue, NumericFault>
```

Core 0.1 uses the Rust `dec` crate's `Decimal128` plus a reusable Context configured for 34 digits and round-half-even. Its status flags are cleared and checked around every operation: inexact/rounded is an accepted rounded result, while overflow, underflow, division by zero and invalid operation become Dever runtime faults. Decimal NaN/Infinity never enter a normal Dever `Value`. Compile-time constant evaluation and the reference evaluator call this same runtime wrapper, so native and reference behavior cannot acquire separate Decimal implementations.

This boundary is necessary because `dec` uses libdecnumber internally and exposes more IEEE decimal states than the language permits. Conformance tests, not the dependency's default operators, define Dever behavior.

Int operations use Rust checked arithmetic. Division and remainder explicitly handle zero and `i64::MIN / -1`. Int-to-Decimal conversion is exact.

Float uses Rust f64 operations without fast-math transformations. List summation is left-to-right. NaN/Infinity classification is exposed through `math` functions.

## 12. Standard Library Boundary

Core names such as `each`, `reduce`, `reduce_until`, `close` and `get` are registered from a static table containing:

- accepted arities;
- generic input/output constraints;
- whether an argument is a sequence, value or statically bound handler;
- evaluator function.

Qualified standard packages (`text`, `decimal`, `float`, `math`) use the same table with qualified names. This is a fixed compiler-owned catalog, not a plugin registry.

Every implemented standard operation has one checking contract and one native lowering rule. Sequence source classification and handler validation are shared so List, Bytes and Stream forms cannot drift. The deferred evaluator must later consume these same checked contracts rather than define a second interpretation.

## 13. CLI Contract

`dever-cli` builds one `dever` binary:

```text
dever check <source-root>
dever fmt <source-root>
dever build <source-root> <package.function> --output <path>
dever run <source-root> <package.function>
```

- `check` loads every package and runs all static phases.
- `fmt` prepares every file before writing; if any source file is invalid, no file is modified. Record-field reordering requires successful resolution.
- `build` checks the complete source root, specializes one exposed zero-input function as the entry point and writes an optimized native executable to the requested path.
- `run` uses the same native compilation path in a compiler-managed directory and then invokes the executable; it does not run the reference evaluator.
- Outputs print in declaration order using deterministic Dever value formatting.
- Diagnostics go to stderr; program output goes to stdout.
- Exit 0 means success, exit 1 means source/runtime diagnostics, exit 2 means invalid CLI usage.

The CLI is deliberately not a project/package manager. Source-root selection avoids introducing a manifest before dependency versions or executable targets are designed.

Core 0.1 therefore has no `add`, `update`, registry, signature or component-cache commands. Those are a 0.2 distribution concern; `check`, `build` and `run` remain offline and deterministic.

### 13.1 Command Identity And Coexistence

The Cargo binary and canonical product command remain `dever`. Command examples, help semantics and the public CLI contract use that name.

During Core 0.1 coexistence with the existing Dever framework CLI, a machine may expose this compiler on `PATH` as `deverc`. That entry is a symlink or launcher for the same compiler binary, not a second CLI implementation. It must not replace, shadow or modify the existing `dever` executable.

Repository builds and automated tests invoke the built binary directly and never mutate global executables or shell configuration. Installing, updating or removing the temporary `deverc` entry is an explicitly authorized machine-level operation. Moving the compiler onto the global `dever` name requires a separate migration plan.

## 14. Diagnostics

All compile diagnostics use one structure:

```text
severity
code
message
primary span
zero or more labeled related spans
optional help
```

Examples include both conflicting clause spans, declaration and bad-reference spans, or first initialization and later type-changing assignment. Rendering uses `path:line:column` and a source excerpt. Messages describe the violated Dever rule and never expose Rust type names, crate names or implementation stack traces.

Runtime faults use the same renderer with a runtime category. The CLI may include a Dever call trace, but the first diagnostic always points to the failing `.dever` operation.

## 15. Migration And Repository Cleanup

Implementation starts from the documentation-only repository verified on 2026-09-05:

1. Initialize `dever-core` and `dever-tests`; add `dever-runtime` and `dever-cli` at their implementation stages.
2. Implement `dever-core` around textual language phases.
3. Do not recreate old Refiner, IR, Workbench, generated backend or semantic JSON architecture.
4. Preserve the existing deletion of superseded planning documents; no old child links exist.
5. Update repository `AGENTS.md` and Trellis package specs so future work treats `.dever` as the trusted source.
6. Add the Core 0.1 examples and tests.

No database, persisted user data or old implementation exists, so there is no migration. Git history retains the old planning documents.

## 16. Verification Strategy

Tests are organized by behavior rather than compiler module:

```text
test/dever-tests/tests/
  syntax_and_format.rs
  package_resolution.rs
  type_checking.rs
  clause_analysis.rs
  value_semantics.rs
  collections.rs
  numbers.rs
  failures.rs
  native.rs
  differential.rs
  performance.rs
  cli.rs
```

Fixtures include valid programs plus one focused invalid case per diagnostic. Tests assert diagnostic code and relevant spans, not full prose, except dedicated rendering tests.

The highest-risk property tests are:

- parse/format/parse semantic equivalence and formatter idempotence;
- clause order permutation does not change coverage or result;
- Int division/remainder identity across boundary samples;
- Decimal conformance vectors for precision, rounding and fault flags;
- copy-on-write values do not expose alias mutation;
- Map overwrite/remove/reinsert order;
- Float sum remains left-to-right.
- reference evaluator and native executable produce identical outputs/fault categories for the conformance corpus;
- generated executables do not link the evaluator or perform per-element dynamic dispatch;
- representative native hot paths stay within the PRD performance budget against equivalent safe Rust baselines.

The foundation extension adds focused cases to the existing `syntax_and_format`, `core_semantics`, `core_native`, `runtime_foundations` and `hello_native` targets. They cover handler syntax and forbidden positions, signature matching and forwarding, specialization-cycle rejection, List/Bytes reduce behavior, Stream alias/EOF/stop/close/error behavior, official package compilation, and actual CLI check/run/build of a chunked state parser. File fixtures use test-owned temporary directories; TCP fixtures use only loopback port 0, bounded data and explicit timeouts.

No existing service, browser, npm build, full workspace test or external integration environment is involved in this extension.

## 17. Key Trade-offs

- One compiler crate is less independently reusable than many compiler crates, but avoids unstable public boundaries during the first grammar; the native runtime is separate only because generated programs are a real independent consumer.
- No recursion/effects limits programs to finite pure computation, but makes the first language deterministic and prevents hidden control flow.
- Full clause checking is more compiler work than ordered pattern matching, but it directly enforces the user's requirement that AI-written code cannot hide behavior in ordering or nested branches.
- A decimal dependency is safer than hand-written decimal128 arithmetic; the wrapper and conformance suite prevent it from defining public language behavior.
- A small fixed standard catalog is less extensible than plugins, but keeps the first implementation inspectable and removes speculative architecture.
- Rust emission gets optimized native code much earlier than implementing an LLVM backend, while typed HIR, differential tests and a private artifact boundary prevent Rust from becoming the language definition.
- Keeping a reference evaluator adds implementation work, but it localizes semantic debugging and gives native code generation a deterministic oracle; it is excluded from production binaries.
- Separating component identity from Git/registry location adds two CLI-managed metadata files in 0.2, but prevents repository moves from becoming source-breaking type renames and enables one verified global source store.
# Compiler Contract Extension (2026-09-08)

Extend existing AST fields and clause modifiers, normalize field bounds with the same interval owner as patterns, then analyze checked HIR before publishing Program. Keep private checks in field access/construction owners. Function/package metadata feeds effect/dependency checks and deterministic API snapshots. Failure obligations follow values across aggregates and calls, with explicit source recovery declarations as the only normal-value discharge boundary. Static handlers share existing specialization bindings. Facts must be conservative on mutation and unknown operations; no runtime interpreter, speculative fallback, new dependencies or source keyword for every domain.

Independent syntax/formatter work owns syntax.rs/parser.rs/format.rs; effect/API work owns contracts/effects.rs/api.rs and CLI; main owns semantic types, HIR integration, fact/failure analysis, standard-source migration, tests and docs. All workers preserve shared changes. Core check is the single enforcement entry used by CLI and native execution.

# AI-Oriented Source Architecture Design (2026-09-19)

Source classification is one compiler-owned value derived from the relative source path. It records main or component/domain/role/topic identity and is reused by parsing, Model registration, symbol resolution, API route construction and architecture diagnostics. Those layers must not rediscover roles with independent string searches.

Every physical source still has a unique internal owner name so diagnostics, HIR ownership and native symbol generation remain deterministic. App declarations additionally receive one stable logical alias, `<component>.<domain>.<name>`, regardless of whether they live in `app.dever` or `app/<topic>.dever`. Alias collisions are duplicate declarations. Topic paths remain an implementation detail and never leak into API snapshots or call sites.

Name lookup tries the current physical owner first, then role-relative names in the current domain, same-component App aliases, and finally full App aliases. A resolved reference is accepted only when the role dependency matrix permits it. Main and API may call App; App may call private roles in its own domain and App aliases elsewhere; private roles may collaborate only inside their own domain. API functions are registered for HTTP routing but rejected as source call targets. Model type identity stays path-derived, while generated operations are restricted to their owning domain.

Application `public` modifiers are rejected. App functions and App-declared types form the external contract automatically; Model record/choice/ID types remain type-visible where relations or App contracts require them. After failure inference, every App function is also checked so a private Domain/Adapter error type cannot leak through its effective error set. Bundled standard sources keep compiler-internal visibility metadata because they are not application architecture. The existing API baseline and unused-export checks consume logical App names.

Architecture validation runs before semantic bodies: legal layouts, reserved names, file/directory exclusivity, flat non-API topic directories and the no-single-file-role-directory rule. Declaration-role validation then keeps API thin, Model storage-only and App as the sole cross-domain capability boundary. Port syntax itself is not expanded in this change; `port` is a reserved architectural role until a separately approved replaceable-boundary declaration contract exists.
