# Bootstrap project guidelines

## Scope

This repository is a Rust workspace for the Dever language compiler, runtime,
CLI and repository-root tests. `AGENTS.md` and the actual implementation are
the source of truth for `.trellis/spec/backend/`. There is no frontend package
or UI source in this repository; frontend scaffolding from `trellis init` is
not applicable and must not prescribe hypothetical conventions.

## Completion

- [x] Document compiler/runtime/CLI ownership and real source examples in
  `.trellis/spec/backend/directory-structure.md`.
- [x] Document compiler contracts, ORM, errors, logging, toolchain and focused
  checks in `.trellis/spec/backend/`, backed by source and tests.
- [x] Remove unused frontend template specs; no frontend rules are claimed.
- [x] Keep the backend index in sync and verify no template placeholders remain
  in applicable backend specs.
