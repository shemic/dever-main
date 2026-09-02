# Dever Language MVP Design

- Date: 2026-09-02
- Status: Written specification approved by the user on 2026-09-02
- Scope: First executable vertical slice

## 1. Summary

Dever is an AI-oriented, general-purpose language system whose primary development surface is a multilingual natural semantic workbench rather than conventional source code.

An AI may propose implementations, but it cannot directly modify the trusted program. A deterministic refiner validates typed semantic candidates and promotes only accepted semantics into the trusted semantic graph. Humans review semantic changes, capabilities, state transitions, and resource effects. The accepted graph is lowered to a language-independent Dever IR and then compiled through replaceable platform backends.

The MVP validates this model end to end with a simulated temperature alarm controller. It generates a Rust service and a React web interface without requiring physical hardware.

## 2. Goals

The MVP must prove that:

1. A program can be represented and reviewed as natural semantics instead of generated implementation code.
2. Multiple interface languages can display the same program without changing its behavior.
3. AI-produced changes can remain untrusted candidates until a deterministic refiner accepts them.
4. Unjustified or invalid candidate logic can be isolated in a code scrap yard.
5. Runtime values that violate a declared contract can be isolated without inventing fallback behavior.
6. One trusted semantic graph can generate an executable Rust service and a React interface.
7. The language semantics remain independent of Rust, React, natural-language wording, and any AI provider.
8. An approved project can build deterministically without an AI model or network connection.

## 3. Non-Goals

The MVP will not provide:

- a complete general-purpose standard library;
- a package registry or public package manager;
- a direct LLVM backend;
- a self-hosted compiler;
- physical microcontroller flashing or hardware drivers;
- additional web frameworks beyond the React backend;
- native desktop or mobile UI backends;
- databases, authentication, or deployment infrastructure;
- advanced generics, macros, unrestricted concurrency, or a complete borrow checker;
- formal worst-case execution-time proof;
- a built-in AI chat interface or dependency on a specific model provider.

These exclusions limit the first implementation, not the long-term language direction.

## 4. Core Principles

### 4.1 AI proposes; deterministic systems decide

AI tools operate outside the trusted compiler boundary. They submit typed candidate changes and may use diagnostics to revise them. Only deterministic validation rules can promote a candidate into the trusted program.

### 4.2 The semantic graph is the source of truth

Natural-language views, diagrams, generated Rust, generated React, Dever IR, and machine code are projections or build artifacts. None may independently redefine program behavior.

### 4.3 Natural language is presentation, not identity

Every semantic object has an opaque stable ID. Localized labels and sentences never serve as symbol identity. Changing a translation cannot change executable behavior.

### 4.4 No implicit behavior

I/O, allocation, failure, state transitions, permissions, and fallback policies must be declared. Missing behavior produces a validation error rather than an AI-generated default.

### 4.5 Backends are replaceable

Rust and React are first-generation targets, not parts of the Dever language specification. Dever semantics and IR must not refer to Rust syntax, React hooks, JSX, or a particular runtime implementation.

## 5. System Boundaries

| Component | Responsibility | Must not own |
|---|---|---|
| AI adapter boundary | Accept typed semantic candidate patches from external AI tools | Model-specific prompts inside the compiler |
| Natural semantic workbench | Display, edit, compare, and approve semantic objects in the selected locale | Executable semantics hidden in UI-only state |
| Dever Core | Stable IDs, types, states, rules, capabilities, effects, constraints, and semantic hashing | Temperature-specific or platform-specific behavior |
| Dever Refiner | Deterministic validation, candidate promotion, and rejection diagnostics | Probabilistic acceptance decisions |
| Dever IR | Stable, language-independent executable representation | Rust, React, or localized wording |
| Rust backend | Bootstrap native/service output | Authority to change Dever semantics |
| React backend | Bootstrap web UI output | Authority to change Dever UI semantics |
| Temperature example | Domain rules and simulated device adapters | Compiler or refiner internals |

Dependencies flow from examples and backends toward public Dever interfaces. Dever Core must never depend on the temperature example.

## 6. Trusted Semantic Model

The MVP semantic model contains these first-class objects:

- project and module;
- scalar, record, enum, range, and optional types;
- input, output, and operation;
- capability and effect;
- state machine, state, event, transition, guard, and action;
- invariant, precondition, postcondition, and resource constraint;
- UI screen, field, control, binding, and interaction;
- candidate patch, provenance reference, validation evidence, and approval;
- code-scrap record and runtime-quarantine event.

Each object has a stable ID, kind, typed properties, references to other stable IDs, and optional localized presentation data.

For the MVP, the graph is persisted as canonical UTF-8 JSON with an explicit format version. Object keys are serialized in lexical order; unordered semantic collections are sorted by stable ID; ordered collections retain their declared semantic order; duplicate keys are invalid; and numbers use one normalized representation. Equivalent semantic graphs therefore produce identical behavior hashes. Localized labels and presentation preferences are stored separately and excluded from the behavior hash so translation edits cannot appear as executable changes.

The workbench is the normal editing surface. The serialized form is intended for storage, tooling, and version control, not routine manual programming.

## 7. Multilingual Natural Semantic Workbench

The workbench is a manually implemented React application in the MVP. It is not generated by Dever itself.

It provides:

- project and module navigation;
- natural semantic summaries;
- rule and decision tables;
- state-machine views;
- capability and effect views;
- resource-constraint views;
- UI structure and interaction views;
- candidate semantic diffs;
- code-scrap diagnostics;
- runtime-quarantine events;
- approval state and validation evidence.

The initial locale packs are Simplified Chinese and English. Both are deterministic renderers of the same semantic objects. The architecture permits later locale packs without changing Dever Core.

Built-in phrases come from controlled locale templates. Domain terms may have localized labels, but their stable IDs and types remain unchanged. AI may propose translations, but translation is never part of compilation.

The default experience contains no conventional implementation code. An optional expert diagnostic view may expose lowered forms, but it is not a trusted authoring surface.

## 8. Candidate and Refinement Workflow

External AI tools submit candidate patches containing typed operations such as:

- add or remove a semantic object;
- update a typed property;
- connect or disconnect stable-ID references;
- add a state transition or rule;
- provide an implementation candidate for an approved operation;
- attach provenance to a requirement or approved contract.

The refiner applies candidates only in an isolated candidate graph. It then performs deterministic checks for:

- valid schema and stable-ID references;
- type correctness;
- valid states and transitions;
- authorized capabilities and effects;
- declared failure behavior;
- contract provenance for branches and fallbacks;
- duplicate or conflicting rules;
- unreachable semantic objects;
- dependency closure;
- MVP resource constraints, including forbidden dynamic allocation.

A candidate is promoted atomically only when every required check passes and a human approves the semantic diff. Partial promotion is forbidden because selected fragments may not form a coherent program.

AI-generated explanations are not validation evidence. Review summaries are derived mechanically from the candidate and trusted semantic graphs.

## 9. Human and AI Collaboration

Humans own:

- goals and acceptance behavior;
- module boundaries and public operations;
- domain types, invariants, and state transitions;
- capability grants and resource policies;
- approval of semantic changes;
- explicit authorization of unsafe or platform-specific operations.

AI tools own:

- candidate implementations within approved boundaries;
- candidate tests and examples;
- revisions in response to deterministic diagnostics;
- internal optimizations that preserve approved contracts.

The workbench review artifact must show at least:

- the requirement or contract being implemented;
- added, removed, and changed semantic objects;
- state-machine changes;
- capability and effect changes;
- resource-policy changes;
- affected modules;
- validation evidence;
- rejected alternatives and their reasons when relevant.

Routine review focuses on semantics and boundaries. High-risk operations can require an expanded expert review.

## 10. Memory and Execution Model

Dever has no mandatory tracing garbage collector. It distinguishes memory allocation from memory reclamation:

- ownership and regions determine deterministic lifetime;
- transfer, temporary use, exclusive mutation, copying, sharing, and regional storage are explicit semantic choices;
- the refiner infers local ownership where the result is unique;
- ambiguous cross-module or long-lived ownership requires an explicit semantic decision;
- dynamic allocation is an effect that a target or module may forbid.

The MVP needs only the subset required by the temperature example: value types, immutable data, explicit state ownership, and bounded event records. A complete general-purpose ownership and lifetime system is outside MVP scope.

The Rust service may use heap storage where declared. A future microcontroller target can reject all allocation effects. React output executes within the browser's JavaScript runtime and therefore follows that platform's memory behavior without changing Dever Core semantics.

## 11. Bootstrap and Backend Strategy

The initial compiler, refiner, and service runtime are implemented in Rust.

The first backend path is:

1. Trusted semantic graph to Dever IR.
2. Dever IR to generated Rust.
3. Generated Rust to native output through the external Rust toolchain.

Generated Rust is disposable build output. It is not trusted source and is not presented for routine human review.

The first web path is:

1. Trusted UI semantics to React and TypeScript build output.
2. Trusted service semantics to the Rust service.
3. Generated client bindings connect the two outputs without redefining domain behavior.

The long-term path adds direct LLVM lowering and gradually rewrites Dever Core and compiler components in Dever. Rust remains a bootstrap implementation until self-hosting is reproducible. The Dever specification must remain independent throughout.

## 12. The Two Isolation Areas

### 12.1 Code scrap yard

The code scrap yard is a build-time and review-time store for rejected candidates. A record includes the candidate, semantic base version, rejection reasons, provenance, and validation evidence.

Scrap content:

- never enters the trusted graph;
- is never compiled or deployed;
- cannot be promoted without revalidation against the current trusted graph;
- may be retained for audit or discarded by an explicit retention policy.

### 12.2 Runtime quarantine

Runtime quarantine receives values or events that violate an input contract. It is not a silent fallback channel. Each quarantine event records the rejected input, contract violation, source capability, and time or sequence metadata available on the target.

The target policy determines whether quarantine uses a bounded buffer, callback, external report, or immediate stop. The MVP Rust service uses a bounded in-memory event list. A future microcontroller backend may use a fixed-size ring buffer or fail-stop behavior.

Neither isolation area is a tracing-GC memory heap.

## 13. Temperature Alarm Vertical Slice

### 13.1 Domain contract

| Semantic item | Approved behavior |
|---|---|
| Valid temperature | From -40 degrees Celsius through 125 degrees Celsius, inclusive |
| Initial state | Normal |
| Enter alarm | A valid reading greater than or equal to 80 degrees Celsius |
| Leave alarm | A valid reading below 75 degrees Celsius |
| Hysteresis interval | From 75 inclusive to below 80 retains the prior Normal or Alarm state |
| Sensor disconnect | Enter Offline and immediately enable the alarm |
| Recovery from Offline | A valid reading at or above 80 enters Alarm; a valid reading below 80 enters Normal |
| Invalid reading | Keep the current domain state and alarm output, reject the reading, and create a runtime-quarantine event |
| Alarm output | A simulated alarm capability displayed in the web UI |

All thresholds and failure actions are explicit approved rules. The AI and refiner may not invent default readings, retries, state resets, or alarm suppression.

### 13.2 Simulator behavior

The example provides:

- a manual temperature slider restricted to valid values;
- an automatic normal-fluctuation scenario;
- an automatic heating scenario that enters Alarm;
- an automatic cooling scenario that clears Alarm below 75 degrees;
- a sensor-disconnect injection;
- out-of-range and malformed-reading injections;
- current temperature, state, alarm output, state history, and quarantine views.

No physical hardware is required. Simulated temperature input and alarm output implement example-owned capability adapters. They remain outside Dever Core.

### 13.3 Generated outputs

The trusted example semantics generate:

- a Rust service that owns the state machine and enforces the approved contract;
- a React interface that drives simulations and displays trusted state and quarantine events;
- generated bindings between the interface and service;
- semantic source maps that relate backend diagnostics to stable semantic IDs.

Generated files live under the example build output and are not committed as trusted source.

## 14. Logical Repository Structure

The MVP is one independent repository at `/data/project/dever` so tightly coupled compiler, workbench, backend, and example changes can be reviewed together. Logical packages remain separate:

- `dever-core`;
- `dever-refiner`;
- `dever-ir`;
- `dever-backend-rust`;
- `dever-backend-react`;
- `dever-workbench`;
- `examples/temperature-alarm`.

This list defines ownership boundaries, not a requirement to scaffold every directory before its implementation phase begins.

Removing `examples/temperature-alarm` must not prevent the core, refiner, IR, backends, or workbench from building independently.

## 15. Acceptance Criteria

The MVP is accepted only when all of the following hold:

1. Chinese and English views render the same stable semantic IDs and behavior hash.
2. A locale-only edit leaves the behavior hash unchanged.
3. A valid typed candidate produces a deterministic semantic diff and can be atomically approved.
4. An unjustified fallback, duplicate rule, invalid effect, or broken reference is rejected into the code scrap yard with a specific reason.
5. Approved semantics generate a working Rust service and React interface.
6. The manual slider drives valid readings through the generated service.
7. Automatic heating enters Alarm at 80 degrees or above.
8. Automatic cooling leaves Alarm only below 75 degrees.
9. Sensor disconnect enters Offline and enables the alarm.
10. Invalid readings preserve the current domain state and create runtime-quarantine events.
11. Generated backend diagnostics can be related to Dever stable semantic IDs.
12. With the declared toolchain and locked dependencies already available locally, the approved project can rebuild without access to an AI model or network service.
13. Generated Rust and React are disposable and do not become trusted source.
14. Dever Core contains no temperature-domain concepts and remains usable without the example.

## 16. Principal Risks and Controls

| Risk | Control |
|---|---|
| Natural wording becomes ambiguous | Controlled deterministic locale templates render typed semantic objects |
| AI explanation disagrees with behavior | Review artifacts are mechanically derived from semantic graphs |
| A second AI merely approves the first AI | Final acceptance is performed only by deterministic rules and human approval |
| Selected snippets form an incoherent program | Candidate promotion is atomic and dependency-closed |
| Rust semantics leak into Dever | Dever Core and IR define independent types, ownership, effects, and contracts |
| Generated code becomes the maintained source | Generated outputs are disposable and excluded from trusted project state |
| The MVP grows into a full language implementation | The non-goals are enforced until the vertical slice passes its acceptance criteria |

## 17. Completion of the Design Phase

After this document is reviewed and approved, the next artifact is an implementation plan that decomposes the vertical slice into independently testable milestones. No implementation should begin from this design document alone without that plan.
