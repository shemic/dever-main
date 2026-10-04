# Compiler Development Guidelines

This repository implements the Dever language compiler in Rust. It is not a Go Dever application or a database-backed service.

## Pre-Development Checklist

1. Read the current task PRD, design and implementation progress.
2. Read [Directory Structure](./directory-structure.md), [Error Handling](./error-handling.md) and [Quality Guidelines](./quality-guidelines.md).
   For CLI, formatter, native optimization or official packages, also read [Toolchain and Library Contracts](./toolchain-and-library.md), including signed ecosystem packs and bounded binary resource attachments.
   For language contracts and API boundaries, read [Compiler Contracts](./compiler-contracts.md).
   For LLVM typed lowering or runtime C ABI work, read both contracts; distinguish kernel/object evidence from complete application semantics and formal target runtime packs.
   For Model, ORM, database configuration or runtime connections, also read [Database Guidelines](./database-guidelines.md).
   For Dever logging or HTTP request-boundary records, read [Logging Guidelines](./logging-guidelines.md).
3. Inspect affected compiler phases and existing tests before adding another rule or helper.

## Quality Check

- Check the implemented milestone only; syntax success must not imply semantic validity.
- Run the focused checks listed in the quality guide.
- Keep all tests under repository-root `test/`.
- Preserve unrelated user changes and do not create Git commits without authorization.

Database behavior is a compiler/runtime contract only within the static ORM boundary documented in [Database Guidelines](./database-guidelines.md). Shared logging and native exit-flush contracts are documented in [Logging Guidelines](./logging-guidelines.md).
