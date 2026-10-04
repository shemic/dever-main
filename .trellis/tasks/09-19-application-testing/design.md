# Dever 应用测试体系技术设计

## Architecture

```text
module/ + test/
  -> one SourceMap with compiler-assigned Application/Test origins
  -> existing parser and Markdown extraction
  -> existing declaration registration and semantic checker
       -> Test layout/access rules
       -> filename-matched TestCase metadata
       -> test-only assert/assert_eq lowering
  -> checked Program
  -> one native suite containing every checked TestCase specialization
  -> case-id dispatch into one isolated native process + temporary project directory per case
  -> captured result + deterministic CLI summary
```

There is no test interpreter, dynamic test registry, source macro, runtime reflection or production test symbol. Test behavior is a thin compiler-owned entry mode over the same checked Program and native backend.

## Source Ownership And Identity

`SourceFile` gains a compiler-assigned source origin rather than inferring test authority from a user-controlled path. Production loading remains the current `SourceMap::load(module_root)` behavior. A dedicated project-test loader loads `module/` as application sources and an existing `test/` as test sources; a missing test root is an empty set, while a non-directory or unreadable root is an explicit load error.

Test sources retain two paths:

- display path: `test/user/account/register.dever`, used by diagnostics and CLI output;
- logical path: `user/account/register.dever`, used for package identity and Markdown package documentation.

The new `SourceLayout::Test { component, domain, topic }` is valid only for exactly three logical segments. It applies existing lower-snake-case and generic-bucket rules. `.dever` and `.dever.md` with the same logical stem are one duplicate test identity.

The checker cache identity includes source origin/logical path so a strict application source and a test source with equal bytes cannot share the wrong checked result.

## Test Entry Contract

For `test/user/account/register.dever`, the logical package is `user.account.register`; the checker selects its only ordinary `register() ()` function and records a `TestCase` containing:

- stable public identity `user/account/register`;
- checked private function id;
- source span and whether its reachable graph uses a Model.

Other declarations stay private to the same test file. A same-name overload, transaction/recovery entry, parameters or outputs are rejected before native compilation. Test functions are never marked public and never enter API/model snapshots.

`Program` exposes read-only test metadata to the CLI. Native compilation accepts an already checked test-case id through a dedicated method instead of weakening `Program::entry` or making test functions public.

## Visibility

Test resolution extends the existing role-aware symbol owner rather than adding a parallel resolver:

- same file: private helper functions/types;
- same domain: App and Domain functions/types;
- other domains: App public surface only;
- Model values/types: readable under existing public/type rules;
- Model operations: still rejected because only the owning App role may issue CRUD;
- Adapter, Port, API and other Test owners: not callable.

All production callers reject Test targets before ordinary same-domain checks, preventing reverse dependencies. Test assertions are available only while checking a Test owner.

## Assertions

`assert` and `assert_eq` are compiler-owned test intrinsics, not bundled library functions. They reuse the existing `Intrinsic` expression shape so dependency, effect, liveness and native traversal do not gain a second assertion IR hierarchy.

- `assert(value)` checks one `Bool` expression.
- `assert_eq(actual, expected)` checks the left expression first, uses its exact type as the right-hand expectation, and requires the existing `Type::comparable` contract.
- both return Unit and are legal only as ordinary zero-output actions in Test-owned functions;
- declarations named `assert` or `assert_eq` remain ordinary outside tests and are reserved inside a test file.

Native emission evaluates each operand once and preserves source order. On failure it creates a source-located assertion error. `assert_eq` renders concrete typed values through the existing `Render` implementations; generated records/choices retain their source-shaped renderer. No generic runtime `Value`, serialization round-trip or reflection is introduced.

## Native Execution And Reporting

The CLI checks the combined source set once, sorts `TestCase` metadata, then compiles one native test suite and runs each case serially. The suite unions the existing specialization graphs and dispatches one checked private entry by its stable sorted case index. It does not introduce a dynamic function registry or runtime reflection.

Every case still starts a fresh process from the same immutable suite executable. The runner stages that executable beside a new temporary project for each process, so runtime singletons, generated SQLite settings, Model initialization and data remain isolated without an in-process reset protocol. A suite that contains any database test links the SQLite-capable runtime once; only a selected database branch loads its generated setting and initializes Models. A non-database branch does neither.

Native cache identity continues to cover generated code, compiler identity, runtime bytes, toolchain, platform, environment and compiler options. Tests use a dedicated fast compilation profile because they validate behavior rather than production code generation performance; `run` and `build` retain the production optimization contract.

For each case the runner:

1. creates an owned temporary project directory;
2. reuses the single compiled suite and selects the checked private test entry by internal case index;
3. stages the executable in that directory;
4. writes a generated `config/setting.json` only when the reachable graph uses Model operations;
5. runs the executable with the temporary project as its executable/config root;
6. captures stdout/stderr and records pass/failure;
7. drops all owned native and runtime artifacts before continuing.

Successful process output is captured and omitted from runner status. Failed output is printed in clearly labelled blocks below the case result. A source/check failure prevents all execution; a per-case compile/start/run/assert/business/fault failure is recorded and later cases continue. The final exit status is nonzero when any case failed or was not run.

The first release has no child timeout or signal manager. Explicit cancellation may terminate the command; recoverable success/failure paths own deterministic cleanup.

## SQLite Isolation

The runner never loads the project's deployment database configuration for tests. If no test graph has a database effect, the suite uses the base runtime profile. A mixed suite links the SQLite-capable runtime once, but a selected non-database case writes no setting file and does not call database bootstrap or Model initialization.

If the graph has a database effect, the runner generates a temporary setting with:

- `database.default` as SQLite;
- one SQLite entry for each explicit Model connection name;
- paths below that case's `data/db/`;
- one connection and the existing bounded page defaults.

Package-root selectors retain runtime resolution semantics: an undeclared root falls back to `default`; explicit selectors retain separate connections. The generated settings are parsed by `dever_runtime::config::Settings`, validated by `Program::validate_database_settings`, and consumed by the normal bootstrap, schema migration, Seed, transaction and shutdown code. No test-only database handle reaches Dever source.

The emitter currently initializes the application's complete Model schema once a reachable test uses the database. This preserves foreign keys, migrations and Seed as an application-wide contract. A non-database test uses no database profile even when unrelated Models exist in the module.

## CLI And Formatting

`Action::Test` is added to the existing explicit CLI argument parser; there is no generic subcommand framework. The command performs semantic checking, normal warnings and API baseline validation before execution.

Formatting remains syntax-only. `fmt` discovers both source roots, prepares every replacement before committing any, validates both snapshots again, then performs atomic per-file renames. A missing `test/` contributes no files. This prevents module files from changing when a test Markdown contract or file permission fails.

Human output follows one stable shape:

```text
running 2 tests
test news/article/publish ... ok
test user/account/register ... FAILED

test result: FAILED. 1 passed; 1 failed; 0 not run
```

No JSON/JUnit output or test filter is introduced.

## Markdown And CMS

Test Markdown uses the existing strict document contract. Its package metadata names the logical package (for example `user.account.register`), and its function section documents the filename-matched entry. No test-specific Markdown headings are added.

The maintained CMS Dever and Markdown projects receive equivalent application tests. They call existing App boundaries and assert observable business values and Model counts. They do not add test-only production App functions or duplicate CRUD.

## Compatibility And Trade-offs

- Production `check/api/run/build` continue loading only `module/`; their checked Program and native cache inputs remain unchanged.
- `fmt` intentionally expands to the optional test root because tests are first-class Dever source.
- One suite link removes per-case native compilation. Process-per-case dispatch retains runtime/database isolation, while the suite may be larger than one reachability-pruned test binary.
- Real PostgreSQL application tests are deferred. Adding them later requires an explicit safe test connection and cleanup contract; it must not alter default SQLite behavior.
- There are no fixture hooks or shared test modules. A later addition must prove that it does not introduce hidden order or shared mutable state.

## Cleanup And Rollback

All test execution artifacts are owned by RAII temporary-directory and native-program guards. Cleanup errors do not replace the primary test result. No project source, deployment setting or data path is rewritten by `dever test`.

The change is additive. Rollback removes the Test source origin/layout, assertion lowering, CLI action and example tests; no production source migration or database migration is required.
