<!-- TRELLIS:START -->
# Trellis Instructions

These instructions are for AI assistants working in this project.

This project is managed by Trellis. The working knowledge you need lives under `.trellis/`:

- `.trellis/workflow.md` — development phases, when to create tasks, skill routing
- `.trellis/spec/` — package- and layer-scoped coding guidelines (read before writing code in a given layer)
- `.trellis/workspace/` — per-developer journals and session traces
- `.trellis/tasks/` — active and archived tasks (PRDs, research, jsonl context)

If a Trellis command is available on your platform (e.g. `/trellis:finish-work`, `/trellis:continue`), prefer it over manual steps. Not every platform exposes every command.

If you're using Codex or another agent-capable tool, additional project-scoped helpers may live in:
- `.agents/skills/` — reusable Trellis skills
- `.codex/agents/` — optional custom subagents

Managed by Trellis. Edits outside this block are preserved; edits inside may be overwritten by a future `trellis update`.

<!-- TRELLIS:END -->

## Dever Language Compiler

- This repository develops a new Rust bootstrap compiler, not the existing Go Dever application framework. Do not introduce Model, Page JSON or Service scaffolding here.
- This is a new language under active design: change source contracts directly and migrate repository callers together. Do not add legacy aliases, compatibility modes, fallback implementations or speculative defensive branches. Keep required type/protocol validation, explicit errors and resource lifecycle semantics.
- Program source is `.dever` or `.dever.md`; the latter compiles only top-level fenced `dever` blocks and keeps Markdown headings/prose nonsemantic. One file owns one package; both formats share language/contract checks. The language contract is `.trellis/tasks/09-04-dever-language-mvp/prd.md`; Markdown source rules are in `LANGUAGE.md` and `.trellis/spec/backend/toolchain-and-library.md`.
- Compiler phases belong to `crates/dever-core`. Add the native runtime and CLI when their implementation stages need them; avoid empty crates and placeholder commands.
- Read `.trellis/spec/backend/index.md` before editing. All permanent tests belong under repository-root `test/`.
- Parser success means syntax success only. Do not present it as name resolution, type checking or executable language support.
- Use focused offline checks for the changed phase. Do not run full test suites, service checks or install/replace global commands. Do not commit without user authorization.
