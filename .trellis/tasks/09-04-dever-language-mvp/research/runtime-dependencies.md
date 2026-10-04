# Core 0.1 Runtime Dependency Research

- Checked: 2026-09-03
- Scope: dependencies required by already-approved Decimal and Map semantics

## Decimal128

Selected for the MVP: `dec = "=0.4.11"`.

Evidence:

- The [dec crate documentation](https://docs.rs/dec/latest/dec/) describes `Decimal128` as a 128-bit decimal representation with 34 digits of precision and exposes a configurable `Context` with rounding and exceptional-condition status.
- The [Decimal128 API](https://docs.rs/dec/latest/dec/struct.Decimal128.html) supports the required arithmetic and classification operations.

Reason:

- It directly implements the approved decimal128 contract instead of approximating it with a lower-precision financial decimal.
- Context status lets Dever convert overflow, division by zero and invalid Decimal states into language-level runtime faults.
- A reusable Context avoids the per-operation default-context overhead documented by the crate.

Constraint:

- `dec` is a safe Rust wrapper over libdecnumber and therefore introduces a native C build dependency. It must remain isolated behind `dever_runtime::number::DecimalValue`.
- Dever conformance tests, not the crate's operator defaults or special-value surface, define public behavior.
- A future pure-Rust replacement is allowed only when it passes the same conformance suite; this does not change `.dever` semantics.

Rejected for Core 0.1:

- `rust_decimal`: pure Rust, but its 96-bit coefficient does not provide the approved 34-digit decimal128 contract.
- Hand-written decimal arithmetic: correct rounding, exponent handling and status behavior are too risky for the language MVP.
- Newly published decimal128 implementations: potentially useful later, but less established than the selected reference implementation.

## Insertion-Ordered Map

Selected for the MVP: `indexmap = "=2.14.0"`.

Evidence:

- The [IndexMap project](https://github.com/indexmap-rs/indexmap) documents hash-based lookup with iteration independent of hash order.
- The [IndexMap API](https://docs.rs/indexmap/latest/indexmap/map/struct.IndexMap.html) documents that insertion appends new keys, overwriting preserves the existing position, and order-preserving removal is available.

Reason:

- Those operations match the approved Dever Map contract without maintaining a duplicate vector-plus-index implementation.
- Lookup remains hash based while iteration is deterministic.

Constraint:

- Dever wraps IndexMap and exposes only `get`, `put`, `remove` and `entries`.
- Removal uses the order-preserving operation rather than swap removal.
- Dependency-specific index access never appears in `.dever` or public diagnostics.

## Dependency Discipline

These are planned pins; the initial parser milestone has no external dependencies. Verify availability and pin each version when the runtime first consumes it. No dependency becomes a public language contract. Upgrading either dependency requires running the focused Decimal/Map conformance tests and confirming that serialized/displayed Dever values remain unchanged.
