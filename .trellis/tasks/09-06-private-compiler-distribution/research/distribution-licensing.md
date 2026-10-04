# Distribution Licensing And Implementation Disclosure

## Confirmed Facts

- The Rust project and standard library are distributed under `MIT OR Apache-2.0`: <https://rust-lang.org/policies/licenses/> and <https://github.com/rust-lang/rust/blob/main/COPYRIGHT>.
- A public compiler binary built from the current Rust implementation can contain Rust standard-library code. Its exact notice obligations require a release-license audit; the project must not assume that deleting all Rust attribution is permitted.
- LLVM is `Apache-2.0 WITH LLVM-exception`. LLVM states that binaries including LLVM must reproduce its copyright notice, while permitting closed-source and commercial use: <https://llvm.org/docs/DeveloperPolicy.html#copyright-license-and-patents>.
- Apple licenses prohibit separately using Apple SDKs or running Apple Software on non-Apple-branded hardware, and generally prohibit redistributing Apple Software in whole or part without permission: <https://www.apple.com/legal/sla/docs/xcode.pdf>.

## Consequences

Hiding Cargo files, symbols and implementation-specific diagnostics is a product-surface concern. Required third-party notices are a separate compliance concern and must remain truthful.

There are two viable disclosure boundaries:

1. Ship the Rust bootstrap compiler initially. Normal product surfaces mention only Dever, but a compliant third-party notice may identify Rust and LLVM. Professional binary identification is already outside the task scope.
2. Never ship the Rust bootstrap compiler publicly. Use it only as the private stage-1 seed, complete enough Dever language and runtime capabilities to build a stage-2 compiler, and publicly distribute the verified stage-2/stage-3 Dever compiler. LLVM attribution can remain if LLVM is embedded, but Rust is absent from the public executable and notices.

The second boundary matches a strict “no public Rust trace” requirement but makes self-hosting a release prerequisite rather than a later roadmap item. It materially expands the first public release.

## Rule

Do not use renamed tools, stripped license files, misleading notices, obfuscation or packaging tricks as a substitute for the chosen disclosure boundary.
