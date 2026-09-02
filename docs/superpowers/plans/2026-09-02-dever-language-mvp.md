# Dever Language MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first executable Dever vertical slice in which a human reviews multilingual natural semantics, an external AI can submit only typed candidates, a deterministic refiner controls promotion, and one trusted temperature-alarm graph generates a Rust service and a React simulator.

**Architecture:** The repository is a Rust workspace plus two React applications. `dever-core` owns platform-neutral semantics, `dever-refiner` owns candidate validation and atomic promotion, `dever-ir` owns the executable representation, and each backend only lowers that IR. The temperature example depends on those public packages; no core package may depend on the example. Generated Rust and React live only under `examples/temperature-alarm/build/` and never become trusted source.

**Tech Stack:** Stable Rust (edition 2021), Cargo, `serde`, `serde_json`, `sha2`, Axum/Tokio for local HTTP adapters, React, TypeScript, Vite, Vitest, Testing Library, and npm lockfiles.

---

## Execution Constraints

- The current machine has Node.js `24.15.0`, npm `11.12.1`, and Corepack `0.34.6`.
- The current machine does **not** have `rustc`, `cargo`, or `rustup`. Stop at Task 0 and request authorization before installing a Rust toolchain. Toolchain installation is external setup, not a repository change.
- Do not run `npm run build`, workspace-wide tests, or end-to-end/integration tests by default. Run only the targeted commands listed under each task. Before the final service/browser exercise, explain that it starts local processes and obtain confirmation.
- Do not create a Git commit unless the user explicitly requests it. Commit steps are intentionally omitted from this plan.
- Keep permanent tests below repository-root `test/`. Do not place lasting tests beside production source.
- Add packages only when their implementation task begins. Do not scaffold all future directories in Task 1.
- Do not add provider systems, plugin registries, databases, authentication, deployment files, queues, caches, or compatibility branches.

## Acceptance Traceability

| Design acceptance criterion | Owning task(s) |
|---|---|
| Same IDs/hash in Chinese and English; locale edits do not affect behavior | 3 |
| Deterministic candidate diff and atomic human approval | 4, 5 |
| Invalid candidates enter the code scrap yard with exact reasons | 4, 5 |
| Trusted graph lowers to Dever IR and generated Rust/React | 6, 7, 8 |
| Temperature, hysteresis, disconnect, recovery, and quarantine behavior | 6, 7, 11 |
| Backend diagnostics map to stable semantic IDs | 7, 8 |
| Rebuild needs no AI/network once toolchains and locked dependencies exist | 11 |
| Generated output is disposable; core has no temperature concepts | 1, 11 |

## Repository Map

Create each path only in its owning task.

```text
/data/project/dever/
  AGENTS.md                              AI/human engineering rules for this repository
  .gitignore                             generated output and local cache exclusions
  Cargo.toml                             Rust workspace membership and shared dependency versions
  Cargo.lock                             locked compiler/workbench-server dependencies
  crates/
    dever-core/                          semantic types, stable IDs, canonical JSON, behavior hash
    dever-refiner/                       isolated candidate graph, validators, diff, approval, scrap
    dever-ir/                            normalized executable IR and lowering
    dever-backend-rust/                  deterministic Rust service generator and source maps
    dever-backend-react/                 deterministic React simulator generator and bindings
    dever-workbench/                     local project store and HTTP adapter
    dever-cli/                           thin command-line composition root
  apps/
    dever-workbench/                     manually maintained multilingual React review surface
  examples/
    temperature-alarm/
      semantic/program.json              trusted language-neutral graph
      locale/zh-CN.json                  Simplified Chinese presentation only
      locale/en-US.json                  English presentation only
      candidates/                        reviewed sample candidate inputs
      build/                             ignored generated Rust/React output
  test/
    dever-tests/                         Rust contract and acceptance tests
    fixtures/                            reusable valid and invalid semantic inputs
    workbench/                           Vitest UI tests
```

Dependency direction:

```text
temperature example -> CLI/workbench -> refiner -> core
temperature example -> CLI -> backends -> IR -> refiner -> core
generated React client -----------------> generated Rust HTTP contract

core -X-> temperature example
IR   -X-> Rust syntax / React / locale text
```

## Milestone 0: Toolchain Gate

### Task 0: Verify prerequisites without changing the machine

**Files:** None.

- [x] Run:

  ```bash
  cd /data/project/dever
  node --version
  npm --version
  rustc --version
  cargo --version
  ```

  Expected now: Node and npm report the versions above; `rustc` fails with `command not found`. Do not reinterpret that as a repository defect.

- [x] Ask the user for permission to install the stable Rust toolchain with the minimal profile. After authorization, use the then-current official Rust installation procedure and restart the shell environment.

- [x] Re-run `rustc --version && cargo --version` and record the exact versions in the implementation session log. Expected: both commands succeed. Do not pin a historical compiler version merely to match this plan.

  Verified on 2026-09-02: `rustc 1.98.0`, `cargo 1.98.0`, `rustup 1.29.1`, target `stable-x86_64-unknown-linux-gnu`, and GCC `13.3.0`. The command runner does not export `HOME`, so implementation commands use `/root/.cargo/bin/cargo` and `/root/.cargo/bin/rustc` explicitly instead of changing shell startup files.

## Milestone 1: Trusted Semantic Kernel

### Task 1: Establish repository rules and the smallest Rust workspace

**Files:**

- Create: `AGENTS.md`
- Create: `.gitignore`
- Create: `Cargo.toml`
- Create: `crates/dever-core/Cargo.toml`
- Create: `crates/dever-core/src/lib.rs`
- Create: `test/dever-tests/Cargo.toml`
- Create: `test/dever-tests/src/lib.rs`
- Create: `test/dever-tests/tests/workspace_smoke.rs`

- [ ] Write `AGENTS.md` with these executable repository rules:

  ```markdown
  # Dever Repository Rules

  - Trusted behavior lives only in the canonical semantic graph.
  - AI output is an untrusted typed candidate until deterministic validation and explicit human approval both succeed.
  - Do not add implicit fallback, retry, default-value, capability, allocation, or error-suppression behavior.
  - `dever-core` and `dever-ir` must remain independent of Rust syntax, React, locale wording, AI providers, and examples.
  - Example packages may depend on core packages; core packages must never depend on examples.
  - Generated files belong under `examples/*/build/` and are not trusted or committed.
  - Permanent tests belong under repository-root `test/`.
  - Prefer explicit data and small functions. Add an abstraction only for a stable invariant or real shared behavior.
  - Do not commit, install dependencies, start services, or run broad builds/tests without the user's authorization when required by the active session instructions.
  ```

- [ ] Write `.gitignore` with only owned generated/local paths:

  ```gitignore
  /target/
  /apps/dever-workbench/node_modules/
  /apps/dever-workbench/dist/
  /examples/*/build/
  *.log
  ```

- [ ] Create a virtual Cargo workspace containing only `dever-core` and the root test crate. Use edition 2021 and centralize exact dependency versions under `[workspace.dependencies]`; initially add only `serde`, `serde_json`, and `sha2` because canonical storage and hashing require them.

- [ ] Keep `test/dever-tests/src/lib.rs` empty and put this deliberately failing smoke test in `test/dever-tests/tests/workspace_smoke.rs`:

  ```rust
  #[test]
  fn workspace_smoke_test() {
      assert_eq!(dever_core::FORMAT_VERSION, 1);
  }
  ```

- [ ] Run `cargo test -p dever-tests workspace_smoke_test -- --exact`. Expected: compilation fails because `FORMAT_VERSION` does not exist.

- [ ] Add exactly this public constant to `crates/dever-core/src/lib.rs`:

  ```rust
  pub const FORMAT_VERSION: u32 = 1;
  ```

- [ ] Re-run the same targeted test. Expected: `1 passed; 0 failed`.

- [ ] Inspect `cargo tree -p dever-core`. Expected: only serialization/hash dependencies and their transitive dependencies; no HTTP, UI, AI, or temperature package.

### Task 2: Implement stable IDs, typed semantic objects, and canonical behavior hashes

**Files:**

- Create: `crates/dever-core/src/id.rs`
- Create: `crates/dever-core/src/model.rs`
- Create: `crates/dever-core/src/canonical_json.rs`
- Create: `crates/dever-core/src/behavior_hash.rs`
- Modify: `crates/dever-core/src/lib.rs`
- Create: `test/dever-tests/tests/core_semantics.rs`
- Create: `test/fixtures/minimal-program.json`
- Create: `test/fixtures/minimal-program-reordered.json`
- Create: `test/fixtures/duplicate-key.json`

- [ ] Define `SemanticId` as a validated value object. Its accepted grammar is lowercase ASCII segments separated by one dot: `[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+`. It must reject whitespace, locale text, empty segments, uppercase letters, and leading digits. Expose `parse`, `as_str`, `Display`, `Serialize`, and `Deserialize`; keep its inner `String` private.

- [ ] Add targeted tests named:

  ```text
  stable_id_accepts_namespaced_ascii_identifier
  stable_id_rejects_localized_or_ambiguous_identity
  canonical_loader_rejects_duplicate_object_keys
  canonical_loader_rejects_non_integer_numbers
  equivalent_unordered_objects_have_identical_behavior_hashes
  changed_transition_has_a_different_behavior_hash
  ```

  The minimal fixtures must contain no temperature terms. Use a generic switch machine with `state.off` and `state.on` so this test cannot accidentally couple core to the example.

- [ ] Run each new test individually, for example:

  ```bash
  cargo test -p dever-tests stable_id_accepts_namespaced_ascii_identifier -- --exact
  ```

  Expected before implementation: unresolved imports or failed assertions.

- [ ] Implement the minimum platform-neutral model, using `BTreeMap`/`BTreeSet` for semantically unordered collections and `Vec` only where order is behaviorally meaningful:

  ```rust
  pub struct SemanticGraph {
      pub format_version: u32,
      pub revision: u64,
      pub project: Project,
      pub modules: BTreeMap<SemanticId, Module>,
      pub objects: BTreeMap<SemanticId, SemanticObject>,
      pub resources: ResourcePolicy,
  }

  pub enum SemanticObject {
      ScalarType(ScalarType),
      RangeType(RangeType),
      RecordType(RecordType),
      EnumType(EnumType),
      OptionalType(OptionalType),
      Contract(Contract),
      Capability(Capability),
      Effect(Effect),
      Input(Input),
      Output(Output),
      Operation(Operation),
      State(State),
      Event(Event),
      Transition(Transition),
      StateMachine(StateMachine),
      UiScreen(UiScreen),
      UiControl(UiControl),
      UiBinding(UiBinding),
      UiInteraction(UiInteraction),
      Scenario(Scenario),
  }
  ```

  Every reference field is a `SemanticId`. Integer ranges use inclusive lower and upper bounds. Transition guards are limited to `Always` and integer intervals in this MVP; this restriction makes overlap and coverage validation deterministic. Actions are limited to state assignment, output assignment, and declared capability invocation. Allocation is represented only by `ResourcePolicy`, never inferred from backend containers.

  Each state machine declares exactly one owning module. MVP values are immutable and have explicit copy or owned value semantics; histories and quarantine buffers are owned by one runtime instance and bounded by `ResourcePolicy`. Shared mutable ownership is outside the MVP rather than silently delegated to a backend.

- [ ] Implement a streaming JSON loader that rejects duplicate keys before typed deserialization. Reject fractional/exponent numbers in semantic files; the MVP normalized numeric representation is signed or unsigned base-10 integer JSON.

- [ ] Implement canonical JSON output by recursively sorting every JSON object key lexically, emitting no insignificant whitespace, preserving declared `Vec` order, and terminating with no newline. Never derive a behavior hash from raw input bytes.

- [ ] Implement:

  ```rust
  pub fn load_semantic_graph(input: &[u8]) -> Result<SemanticGraph, SemanticLoadError>;
  pub fn canonical_behavior_bytes(graph: &SemanticGraph) -> Result<Vec<u8>, CanonicalError>;
  pub fn behavior_hash(graph: &SemanticGraph) -> Result<String, CanonicalError>;
  ```

  `behavior_hash` returns lowercase hexadecimal SHA-256. Locale data is absent from `SemanticGraph`, so it cannot affect this hash by construction.

- [ ] Re-run only the six named tests. Expected: each reports `1 passed; 0 failed`.

- [ ] Refactor pass: verify no semantic validator has entered `dever-core`; core owns representation and canonical identity only.

### Task 3: Add separate locale catalogs and deterministic natural-semantic projections

**Files:**

- Create: `crates/dever-workbench/Cargo.toml`
- Create: `crates/dever-workbench/src/lib.rs`
- Create: `crates/dever-workbench/src/presentation.rs`
- Modify: `Cargo.toml`
- Create: `test/dever-tests/tests/presentation.rs`
- Create: `test/fixtures/locale/en-US.json`
- Create: `test/fixtures/locale/zh-CN.json`

- [ ] Add `dever-workbench` only now. It may depend on `dever-core`; `dever-core` must not depend on it.

- [ ] Define the locale file as a presentation-only catalog:

  ```json
  {
    "locale": "en-US",
    "terms": {
      "project.switch": "Switch",
      "state.off": "Off",
      "state.on": "On"
    },
    "templates": {
      "state.initial": "Initial state: {state}",
      "transition.on_event": "When {event}, change from {from} to {to}."
    }
  }
  ```

  The Chinese fixture must contain the same term IDs and template keys with Chinese values. Missing keys are errors; there is no silent fallback to English.

- [ ] Add tests named:

  ```text
  chinese_and_english_views_keep_the_same_semantic_ids
  chinese_and_english_views_keep_the_same_behavior_hash
  locale_only_edit_does_not_change_behavior_hash
  missing_locale_entry_is_an_explicit_error
  ```

- [ ] Run each test before implementation. Expected: unresolved presentation API.

- [ ] Implement a `NaturalDocument` made only of ordered sections and `NaturalStatement { semantic_ids, message_key, arguments }`. The server-side presenter derives statements mechanically from typed graph objects. Locale rendering substitutes only declared arguments and must return `PresentationError::MissingTerm` or `MissingTemplate` instead of inventing text.

- [ ] Re-run only the four named tests. Expected: each passes.

- [ ] Run `cargo tree -p dever-core` and inspect the output. Expected: it contains no `dever-workbench`; core remains independent.

## Milestone 2: Deterministic Refinement

### Task 4: Apply typed candidates in isolation and derive deterministic semantic diffs

**Files:**

- Create: `crates/dever-refiner/Cargo.toml`
- Create: `crates/dever-refiner/src/lib.rs`
- Create: `crates/dever-refiner/src/candidate.rs`
- Create: `crates/dever-refiner/src/diff.rs`
- Create: `crates/dever-refiner/src/validation/mod.rs`
- Modify: `Cargo.toml`
- Create: `test/dever-tests/tests/refiner_candidates.rs`
- Create: `test/fixtures/candidates/valid-add-transition.json`
- Create: `test/fixtures/candidates/stale-base.json`

- [ ] Define a typed patch envelope:

  ```rust
  pub struct CandidatePatch {
      pub proposal_id: SemanticId,
      pub base_revision: u64,
      pub base_behavior_hash: String,
      pub provenance: BTreeSet<SemanticId>,
      pub operations: Vec<PatchOperation>,
  }

  pub enum PatchOperation {
      AddObject {
          id: SemanticId,
          object: SemanticObject,
          evidence: BTreeSet<SemanticId>,
      },
      ReplaceObject {
          id: SemanticId,
          expected_hash: String,
          object: SemanticObject,
          evidence: BTreeSet<SemanticId>,
      },
      RemoveObject {
          id: SemanticId,
          expected_hash: String,
          evidence: BTreeSet<SemanticId>,
      },
  }
  ```

  There is no raw source insertion operation and no natural-language operation.

- [ ] Add tests named:

  ```text
  valid_patch_changes_only_an_isolated_candidate_graph
  equivalent_patch_inputs_produce_identical_diff_hashes
  stale_base_revision_is_rejected_before_operations_apply
  replace_requires_the_expected_object_hash
  failed_multi_operation_patch_leaves_trusted_graph_unchanged
  ```

- [ ] Run each test individually. Expected before implementation: failure.

- [ ] Implement `prepare_candidate(trusted, patch) -> Result<PreparedCandidate, RefinementFailure>`. Clone the trusted graph once, check base revision/hash before any operation, apply all operations to the clone, and derive added/removed/changed object IDs by comparing canonical object hashes. Sort diff entries by stable ID.

- [ ] Hash the canonical semantic diff and expose it as `review_hash`. This is the value later bound to human approval.

- [ ] Re-run the five candidate tests. Expected: each passes and the trusted fixture bytes/hash remain unchanged after every failure.

### Task 5: Validate candidates, bind approval, and persist rejected work in the code scrap yard

**Files:**

- Create: `crates/dever-refiner/src/validation/references.rs`
- Create: `crates/dever-refiner/src/validation/types.rs`
- Create: `crates/dever-refiner/src/validation/state_machines.rs`
- Create: `crates/dever-refiner/src/validation/effects.rs`
- Create: `crates/dever-refiner/src/validation/resources.rs`
- Create: `crates/dever-refiner/src/approval.rs`
- Create: `crates/dever-refiner/src/scrap.rs`
- Modify: `crates/dever-refiner/src/lib.rs`
- Create: `test/dever-tests/tests/refiner_validation.rs`
- Create: `test/dever-tests/tests/refiner_approval.rs`
- Create: `test/fixtures/candidates/invalid-broken-reference.json`
- Create: `test/fixtures/candidates/invalid-duplicate-transition.json`
- Create: `test/fixtures/candidates/invalid-effect.json`
- Create: `test/fixtures/candidates/invalid-unjustified-always-guard.json`

- [ ] Give every diagnostic a stable code, severity, primary semantic ID, deterministic message arguments, and related IDs. Start with these exact error codes:

  ```text
  D_REF_MISSING
  D_TYPE_MISMATCH
  D_STATE_UNKNOWN
  D_TRANSITION_OVERLAP
  D_TRANSITION_GAP
  D_RULE_DUPLICATE
  D_EFFECT_UNDECLARED
  D_FAILURE_UNDECLARED
  D_FALLBACK_UNJUSTIFIED
  D_OBJECT_UNREACHABLE
  D_DEPENDENCY_INCOMPLETE
  D_ALLOCATION_FORBIDDEN
  D_BASE_STALE
  D_APPROVAL_MISMATCH
  ```

- [ ] Add one focused failing test for each of the four required rejection examples plus:

  ```text
  valid_integer_intervals_are_exhaustive_and_non_overlapping
  forbidden_dynamic_allocation_is_rejected
  state_machine_requires_one_explicit_owner_module
  approval_must_match_candidate_review_hash
  valid_human_approval_promotes_the_whole_candidate_atomically
  rejected_candidate_is_serialized_as_scrap_not_trusted_semantics
  scrap_candidate_requires_revalidation_against_current_base
  ```

- [ ] Run each test individually. Expected before implementation: failure.

- [ ] Implement a fixed validation pipeline. Use an explicit ordered array of validator functions so output ordering cannot vary. Do not introduce a plugin registry:

  ```rust
  const VALIDATORS: &[Validator] = &[
      validate_references,
      validate_types,
      validate_state_machines,
      validate_effects_and_failures,
      validate_reachability_and_dependencies,
      validate_resources,
  ];
  ```

- [ ] Treat an `Always` guard or other fallback as valid only when its patch operation cites at least one existing `Contract` semantic ID. An AI explanation string is never evidence.

- [ ] Define `HumanApproval { candidate_id, base_behavior_hash, review_hash }`. `promote` must verify all three fields, increment revision once, and return a new trusted graph. It must have no partial-write API.

- [ ] Define `ScrapRecord { sequence, candidate, base_behavior_hash, diagnostics, validation_evidence }`. The refiner returns the record plus canonical bytes without performing filesystem I/O. Task 9's `ProjectStore` persists those bytes under its configured scrap directory. The trusted-graph loader must reject scrap-record format.

- [ ] Re-run only the named validation/approval tests. Expected: each passes.

- [ ] Refactor pass: diagnostics contain no localized prose and validators contain no temperature IDs or thresholds.

## Milestone 3: Executable IR and Rust Backend

### Task 6: Add normalized Dever IR and the temperature example semantics

**Files:**

- Create: `crates/dever-ir/Cargo.toml`
- Create: `crates/dever-ir/src/lib.rs`
- Create: `crates/dever-ir/src/lower.rs`
- Modify: `Cargo.toml`
- Create: `examples/temperature-alarm/semantic/program.json`
- Create: `examples/temperature-alarm/locale/zh-CN.json`
- Create: `examples/temperature-alarm/locale/en-US.json`
- Create: `examples/temperature-alarm/candidates/invalid-fallback.json`
- Create: `test/dever-tests/tests/temperature_semantics.rs`
- Create: `test/dever-tests/tests/ir_lowering.rs`

- [ ] Encode the approved example contract exactly:

  ```text
  accepted temperature: -40..=125 integer Celsius
  initial: Normal, alarm off
  Normal + -40..=79: Normal, alarm off
  Normal + 80..=125: Alarm, alarm on
  Alarm + -40..=74: Normal, alarm off
  Alarm + 75..=125: Alarm, alarm on
  Offline + -40..=79: Normal, alarm off
  Offline + 80..=125: Alarm, alarm on
  any state + disconnect: Offline, alarm on
  invalid value/type: preserve state and output; append quarantine event
  history capacity: 64
  quarantine capacity: 32
  dynamic allocation policy: bounded
  ```

  Give every contract, state, event, transition, capability, effect, UI control, binding, and scenario its own stable ID. Keep all temperature terms out of Rust source outside the example fixtures and backend output tests.

- [ ] Add targeted tests named:

  ```text
  approved_temperature_graph_passes_every_refiner_validator
  temperature_transition_intervals_are_exhaustive
  temperature_locales_share_ids_and_behavior_hash
  lowering_is_independent_of_locale_catalog
  lowering_sorts_unordered_objects_by_stable_id
  ir_contains_no_rust_or_react_constructs
  ```

- [ ] Run each test individually; expect failure before IR implementation.

- [ ] Define IR as normalized integer/state tables, explicit effects, bounded resources, HTTP-neutral operations, and UI-neutral screen/control intents. Retain `source_id: SemanticId` on every executable instruction. Do not store Rust names, JSX names, URL paths, or localized strings in IR.

- [ ] Add `dever-core` and `dever-refiner` as `dever-ir` dependencies. Implement:

  ```rust
  pub fn lower(
      graph: &SemanticGraph,
      validation: &ValidationReport,
  ) -> Result<ExecutableProgram, LoweringError>;
  ```

  Reject a report whose validated behavior hash differs from the graph's current behavior hash. `dever-ir` consumes the report but does not rerun or redefine refiner rules.

- [ ] Re-run the six tests. Expected: each passes.

- [ ] Run `rg -n -i 'temperature|celsius|alarm|react|jsx|rust' crates/dever-core crates/dever-refiner crates/dever-ir`. Expected: no temperature-domain hits and no backend constructs in core/IR; occurrences in comments describing forbidden coupling must be removed rather than exempted.

### Task 7: Generate a pure state machine plus a thin Rust HTTP service

**Files:**

- Create: `crates/dever-backend-rust/Cargo.toml`
- Create: `crates/dever-backend-rust/src/lib.rs`
- Create: `crates/dever-backend-rust/src/generator.rs`
- Create: `crates/dever-backend-rust/src/source_map.rs`
- Create: `crates/dever-backend-rust/src/templates.rs`
- Modify: `Cargo.toml`
- Create: `test/dever-tests/tests/rust_backend.rs`
- Create: `test/dever-tests/tests/generated_runtime.rs`

- [ ] Add only `dever-core` and `dever-ir` as internal dependencies. Generator implementation may use `serde_json`; it must not depend on the temperature example crate or path.

- [ ] Define one backend result:

  ```rust
  pub struct GeneratedProject {
      pub files: BTreeMap<PathBuf, Vec<u8>>,
      pub source_map: SourceMap,
  }

  pub fn generate(program: &ExecutableProgram) -> Result<GeneratedProject, RustBackendError>;
  ```

  Pure generation returns bytes and performs no filesystem writes. A separate CLI adapter writes them.

- [ ] Add tests named:

  ```text
  rust_generation_is_byte_for_byte_deterministic
  rust_source_map_covers_every_generated_transition
  generated_machine_matches_all_temperature_boundary_cases
  invalid_reading_preserves_state_and_output
  invalid_reading_appends_one_bounded_quarantine_event
  disconnect_enters_offline_and_enables_alarm
  offline_recovery_uses_the_approved_eighty_degree_boundary
  ```

- [ ] Run the generator tests first. Expected: unresolved backend API.

- [ ] Generate two layers:

  ```text
  src/machine.rs   deterministic state/event reducer with no HTTP concepts
  src/api.rs       JSON envelope parsing and snapshot shaping
  src/main.rs      Axum/Tokio composition root and local bind address
  Cargo.toml       exact dependency versions from one backend manifest constant
  Cargo.lock       deterministic backend asset matching the exact manifest versions
  source-map.json  generated line spans -> Dever stable semantic IDs
  ```

  `machine.rs` must validate the input envelope before dispatch. Invalid type/range values append a bounded quarantine record and do not call the reducer. When a bounded buffer is full, remove the oldest entry because that retention rule is declared in the semantic resource policy; do not invent any other recovery.

- [ ] Generate this exact service boundary. Parse the event request initially as `serde_json::Value` so a wrong-typed reading can become a declared quarantine event instead of disappearing inside framework rejection:

  ```text
  GET  /api/snapshot
  POST /api/events  {"kind":"reading","value":80}
  POST /api/events  {"kind":"reading","value":"broken"}
  POST /api/events  {"kind":"disconnect"}
  ```

  A snapshot contains sequence, last accepted temperature, stable state ID, stable output IDs and values, bounded history, and bounded quarantine events. A quarantine event contains rejected JSON value, violated contract ID, source capability ID, and sequence. It contains no invented replacement reading.

- [ ] Compile and execute generated-runtime tests from a temporary directory under `target/`, never under the trusted example source. Use one test name at a time. Expected: all seven targeted tests pass.

- [ ] Verify generated diagnostics by intentionally compiling one copied output with a known generated-token corruption in the temporary test directory. Assert that `source-map.json` maps the affected generated span to the expected transition stable ID. Remove the corrupted temporary directory through the test harness.

- [ ] Refactor pass: template fragments are small functions grouped by generated file; no single string builder owns machine logic, HTTP wiring, manifest generation, and source maps together.

## Milestone 4: React Backend and Simulator

### Task 8: Generate a React simulator from UI intent without moving domain rules into the browser

**Files:**

- Create: `crates/dever-backend-react/Cargo.toml`
- Create: `crates/dever-backend-react/src/lib.rs`
- Create: `crates/dever-backend-react/src/generator.rs`
- Create: `crates/dever-backend-react/src/source_map.rs`
- Create: `crates/dever-backend-react/src/templates.rs`
- Modify: `Cargo.toml`
- Create: `test/dever-tests/tests/react_backend.rs`

- [ ] Define `generate(&ExecutableProgram) -> Result<GeneratedProject, ReactBackendError>` with the same side-effect-free output shape as the Rust backend. Keep the types separate in each backend rather than adding a premature generic backend framework.

- [ ] Add targeted Rust generator tests named:

  ```text
  react_generation_is_byte_for_byte_deterministic
  generated_client_uses_only_declared_service_operations
  generated_ui_contains_all_approved_controls_and_views
  generated_ui_contains_no_temperature_transition_thresholds
  react_source_map_covers_controls_bindings_and_interactions
  ```

- [ ] Run each test before implementation. Expected: failure.

- [ ] Generate exactly these maintained boundaries:

  ```text
  package.json                 exact locked React/Vite dependency declarations
  package-lock.json            deterministic backend asset emitted with exact dependency versions
  index.html
  tsconfig.json
  vite.config.ts
  src/main.tsx
  src/App.tsx                  screen composition only
  src/api.ts                   generated request/response bindings
  src/scenarios.ts             approved input sequences, never state transitions
  src/styles.css
  source-map.json
  ```

  The browser may display state returned by the service but must never reproduce the 75/80 state rules. The valid slider bounds may be present because they are an input/UI contract, not domain transition logic. Fault buttons send a disconnect event, an out-of-range number, or a wrong-typed JSON value through the generated binding.

- [ ] Implement a restrained simulator layout: current state and alarm are primary signals; controls, scenario run state, history, and quarantine are dense unframed sections; use familiar Lucide icons with tooltips for icon-only controls; ensure narrow screens stack without overlap. Do not add marketing content or decorative assets.

- [ ] Re-run the five Rust generator tests. Expected: each passes.

- [ ] Validate `package.json` and `package-lock.json` consistency with `npm install --package-lock-only --ignore-scripts --offline` in a temporary generated directory after package installation has been explicitly authorized. Expected: no lockfile change. Do not run `npm run build`; rendered interaction is verified in Task 11.

## Milestone 5: Multilingual Natural Semantic Workbench

### Task 9: Add a local project store and narrow workbench API

**Files:**

- Create: `crates/dever-workbench/src/store.rs`
- Create: `crates/dever-workbench/src/api.rs`
- Create: `crates/dever-workbench/src/server.rs`
- Modify: `crates/dever-workbench/src/lib.rs`
- Modify: `crates/dever-workbench/Cargo.toml`
- Create: `crates/dever-cli/Cargo.toml`
- Create: `crates/dever-cli/src/main.rs`
- Modify: `Cargo.toml`
- Create: `test/dever-tests/tests/workbench_api.rs`

- [ ] Add Axum/Tokio only to `dever-workbench` and `dever-cli`. Add the refiner and both backend crates as explicit internal `dever-workbench` dependencies for validation/promotion/generation orchestration. Do not leak HTTP types into core, refiner, IR, or backends.

- [ ] Implement a `ProjectStore` with explicit paths for trusted graph, locale directory, candidate inbox, scrap directory, and build output. A single process lock serializes local writes. Persist approved graph updates by writing a sibling temporary file, syncing it, and atomically renaming it; failure leaves the prior graph intact.

- [ ] Expose only the MVP endpoints:

  ```text
  GET  /api/project?locale=zh-CN
  GET  /api/candidates
  POST /api/candidates
  POST /api/candidates/{id}/validate
  POST /api/candidates/{id}/approve
  POST /api/candidates/{id}/reject
  GET  /api/scrap
  POST /api/generate
  GET  /api/runtime/snapshot
  ```

  External AI tools use the same typed `POST /api/candidates` contract as every other client. There is no model endpoint, prompt storage, provider selection, or AI approval path. The runtime endpoint proxies the explicitly supplied generated-service URL and returns a typed `not_connected` status when no runtime URL was supplied; it must not return an invented empty snapshot.

- [ ] Add focused tests named:

  ```text
  external_client_can_submit_only_a_typed_candidate
  project_response_contains_natural_document_ids_and_behavior_hash
  approval_rejects_a_changed_review_hash
  successful_approval_atomically_replaces_the_trusted_graph
  rejected_candidate_is_visible_only_in_scrap
  generation_writes_only_below_the_configured_build_directory
  ```

- [ ] Run each API test using Axum's in-process service interface; do not start a TCP listener. Expected before implementation: failure, then `1 passed` per test after implementation.

- [ ] Implement a thin CLI with these commands and no business logic:

  ```text
  dever check <project-directory>
  dever render <project-directory> --locale <locale>
  dever refine <project-directory> <candidate-file>
  dever generate <project-directory>
  dever workbench <project-directory> --listen 127.0.0.1:<port> [--runtime <generated-service-url>]
  ```

  CLI handlers only parse arguments, call package APIs, and shape diagnostics. Invalid arguments return non-zero status with one usage message; do not add fallback paths or interactive guessing.

- [ ] Run one targeted CLI contract test for each command through the Rust test crate. Do not run the full workspace test suite.

### Task 10: Build the manually maintained React review surface

**Files:**

- Create: `apps/dever-workbench/package.json`
- Create: `apps/dever-workbench/package-lock.json`
- Create: `apps/dever-workbench/tsconfig.json`
- Create: `apps/dever-workbench/vite.config.ts`
- Create: `apps/dever-workbench/index.html`
- Create: `apps/dever-workbench/src/main.tsx`
- Create: `apps/dever-workbench/src/App.tsx`
- Create: `apps/dever-workbench/src/api/client.ts`
- Create: `apps/dever-workbench/src/domain/types.ts`
- Create: `apps/dever-workbench/src/hooks/useProjectReview.ts`
- Create: `apps/dever-workbench/src/components/ProjectNavigation.tsx`
- Create: `apps/dever-workbench/src/components/SemanticSummary.tsx`
- Create: `apps/dever-workbench/src/components/DecisionTable.tsx`
- Create: `apps/dever-workbench/src/components/StateMachineView.tsx`
- Create: `apps/dever-workbench/src/components/CapabilityEffects.tsx`
- Create: `apps/dever-workbench/src/components/CandidateDiff.tsx`
- Create: `apps/dever-workbench/src/components/ScrapDiagnostics.tsx`
- Create: `apps/dever-workbench/src/components/RuntimeQuarantine.tsx`
- Create: `apps/dever-workbench/src/components/ApprovalBar.tsx`
- Create: `apps/dever-workbench/src/styles.css`
- Create: `test/workbench/project-review.test.tsx`
- Create: `test/workbench/locale-switch.test.tsx`
- Create: `test/workbench/approval.test.tsx`

- [ ] Install only exact versions of `react`, `react-dom`, and `lucide-react`; install exact dev versions of TypeScript, Vite, Vitest, jsdom, and Testing Library packages. Use npm's generated lockfile. Do not add a router, global store, CSS framework, or data-fetching library: the MVP has one work surface and one focused state hook.

- [ ] First write UI tests proving:

  ```text
  project summary and stable IDs render without implementation code
  Chinese/English switch changes labels but not displayed behavior hash
  decision table exposes transitions, capabilities, effects, and resources
  candidate diff groups added/changed/removed semantic objects
  approve button is disabled until validation succeeds
  approval submits the exact displayed review hash
  scrap reasons render stable diagnostic code plus localized arguments
  runtime quarantine shows events or an explicit not-connected state
  narrow viewport content has no horizontal document overflow
  ```

- [ ] Run the new tests one file at a time. Expected before components exist: module resolution failure.

  ```bash
  npm test -- --run ../../test/workbench/project-review.test.tsx
  npm test -- --run ../../test/workbench/locale-switch.test.tsx
  npm test -- --run ../../test/workbench/approval.test.tsx
  ```

  Configure Vitest explicitly to include repository-root `test/workbench` and keep production source free of permanent tests.

- [ ] Implement one quiet operational work surface with persistent project navigation, a compact locale selector, tabbed semantic views, and a bottom approval bar. Keep presentation components stateless. `useProjectReview` alone owns loading, candidate selection, validation, approval, rejection, and refresh side effects. `api/client.ts` alone owns fetch calls and response decoding.

- [ ] Do not display generated Rust/TypeScript in the normal interface. An expert diagnostic link may show stable IDs, IR/source-map diagnostics, and hashes, but it cannot edit trusted state.

- [ ] Re-run the three targeted test files and `npm run typecheck`. Expected: all targeted tests pass and TypeScript reports no errors. Do not run `npm run build`.

- [ ] Refactor pass: repeated rendering comes from typed configuration or focused components; no copied fetch/validation/loading/error flow; no nested cards, oversized headings, clipped buttons, or unexplained icon-only actions.

## Milestone 6: Vertical-Slice Closure

### Task 11: Prove all approved behavior through one explicit acceptance matrix

**Files:**

- Create: `test/dever-tests/tests/acceptance_matrix.rs`
- Create: `test/acceptance/README.md`
- Modify only if a test exposes an in-scope defect: files owned by Tasks 2-10

- [ ] Encode the state-machine matrix as table-driven tests, not repeated test bodies:

  ```text
  Normal  + 79         -> Normal,  alarm off
  Normal  + 80         -> Alarm,   alarm on
  Alarm   + 75         -> Alarm,   alarm on
  Alarm   + 74         -> Normal,  alarm off
  Offline + 79         -> Normal,  alarm off
  Offline + 80         -> Alarm,   alarm on
  any     + disconnect -> Offline, alarm on
  any     + -41        -> unchanged, one quarantine event
  any     + 126        -> unchanged, one quarantine event
  any     + "broken"   -> unchanged, one quarantine event
  ```

- [ ] Add deterministic artifact checks:

  ```text
  generate twice -> identical relative paths and bytes
  edit only zh-CN text -> identical behavior hash and backend bytes
  no AI/network call appears in check, render, refine, approve, lower, or generate
  every generated diagnostic/source-map entry resolves to a trusted stable ID
  Cargo metadata shows no core/refiner/IR/backend/workbench dependency sourced from `examples/`
  build output paths are ignored and absent from `git ls-files`
  ```

- [ ] Run each new Rust acceptance test individually. Do not run `cargo test --workspace`.

- [ ] Run `rg -n '(TODO|FIXME|unimplemented!|todo!|panic!\("not implemented|fallback|default reading|retry)' crates apps examples --glob '!examples/*/build/**'`. Expected: no placeholder implementation or invented fallback/retry. Review legitimate contract text containing “fallback” manually rather than hiding it with an exclusion.

- [ ] Run `git diff --check` and `git status --short`. Expected: no whitespace errors; only planned trusted source/docs/lockfiles are tracked, while `examples/temperature-alarm/build/` is ignored.

- [ ] Before starting any local server or browser automation, explain the exact ports/processes and ask for confirmation because this is service-affecting verification.

- [ ] After confirmation, generate the example, start the generated Rust service and workbench on loopback-only unused ports, and exercise the manual slider, all three automatic scenarios, disconnect, out-of-range, and malformed inputs. Use Playwright screenshots at desktop and mobile widths to verify nonblank rendering, stable layout, visible state/alarm changes, history, and quarantine. Stop both processes before finishing.

- [ ] Record only observed commands/results in `test/acceptance/README.md`. Mark checks as passed, failed, skipped, or blocked; never claim the browser flow passed from unit tests alone.

- [ ] Final self-review against all 14 design acceptance criteria. Any unmet item keeps the MVP incomplete. Optional LLVM, self-hosting, MCU, extra UI frameworks, package registry, authentication, and deployment work remain explicitly out of scope.

## Implementation Handoff

This plan is intentionally sequential: later milestones consume stable contracts from earlier ones. Do not parallelize core model, refiner, and IR ownership. Within a later milestone, UI-only work may proceed independently only after the corresponding HTTP/IR contract is fixed.

At execution time choose one mode:

1. **Subagent-Driven (recommended):** one bounded implementation worker per task, followed by main-agent integration and targeted verification; no more than two agents active and no overlapping file ownership.
2. **Inline Execution:** the main agent executes each checkbox in order, pausing at the Rust-installation gate and the final local-service/browser gate.

In both modes, no commit is created without a separate explicit user request.
