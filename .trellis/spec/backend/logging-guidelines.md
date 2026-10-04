# Logging Guidelines

`library/dever/log.dever` provides `debug`, `info`, `warn` and `error` with a
`Text` message and `Map<Text, Text>` fields. Native emission routes these calls
to `crates/dever-runtime/src/log.rs`; do not add a second logger in the CLI or
generated program.

The runtime writes one JSON record per line to stderr, with `level`, `message`
and `fields`. The bounded writer may drop debug/info events under pressure and
reports the count in `dropped_events` on a later record; warn/error events wait
for room, and error records flush immediately. Native program exit flushes the
writer. Keep log messages and fields bounded instead of logging unbounded
payloads.

LLVM applications with reachable log calls use the same invocation-owned
`application::Session` and load `config/setting.json` once, even for log-only CMD.
Configured level filtering and request identity fields come from that owner;
Scope drain precedes pool close and log flush. Pure scalar CMD/kernel entries
do not acquire an API owner. `llvm_api_source` tests warn-level filtering and
65 flush-before-return invocations; do not infer configured logging support
from the standalone kernel logger alone.

The single writer uses one preallocated 128-command queue; debug/info overflow
and warn/error backpressure remain distinct. Flush callers are serialized onto
one persistent acknowledgement state. Release preceding records and flush the
output before acknowledgement; writer exit/panic clears queued owners and wakes
all waiters. Do not add per-flush channels or a second queue. A pure-flush native
ledger exposed lazy mpsc receiver waiter allocations after acknowledgement, so
ordinary first-flush success alone is not a stable exit/allocation boundary.
Keep the original strict ledger, not sleeps, extra warmup or relaxed counts.
Root test/runtime_logging.rs covers FIFO/drop counts, backpressure, concurrent
flushes and writer failure; native_runtime_link covers the actual ABI barrier.

`crates/dever-runtime/src/http/mod.rs` records an HTTP request-boundary error
with `boundary`, `request_id`, `protocol` and `target`. Do not include secrets,
credentials or full request/response bodies in fields. The source-to-stderr
contract is covered by `test/dever-tests/tests/logging.rs`; extend that focused
test when changing the public logging surface or output format.

Secret and complete Models containing private fields are non-observable types,
including nested containers and related Models. Compiler entry/error/API/assertion
render boundaries must reject them before native emission; log arguments remain
Text and Map<Text, Text>. Runtime Secret Debug emits one fixed redacted marker and
never reveals bytes. Do not add a generic conversion or derive Render for a
sensitive nominal type. Ordinary record private fields remain an access rule,
not a promise of cryptographic secrecy.
