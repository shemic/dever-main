# Package Distribution Research

## Question

How should a Dever project consume a component downloaded from GitHub or another Git host without leaking transport details into `.dever` source or losing reproducible builds?

## Primary Evidence

- Go defines a module as a versioned collection of packages. The module path is also the prefix of package import paths and usually identifies where the module can be downloaded. Downloaded source is shared through a module cache, and hashes are verified against `go.sum`.
  - https://go.dev/ref/mod
  - https://go.dev/doc/modules/gomod-ref
- Cargo permits registry, Git and local path sources. Git dependencies may select a tag or revision, and Cargo locks the resolved commit in `Cargo.lock` until an explicit update.
  - https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html
- Cargo and Go both separate downloaded source caches from project build artifacts. Go also supports optional vendoring for a fully project-local source tree.
  - https://go.dev/ref/mod#module-cache
  - https://go.dev/ref/mod#vendoring
  - https://doc.rust-lang.org/cargo/reference/config.html#cache

## Alternatives

### Git URL as package identity

Example: `github.com.shemic.contact.phone.parse(raw)`.

This is easy to locate but couples every source reference and type identity to the current hosting provider. Repository transfer, self-hosting or registry mirroring then becomes a source-breaking rename. It also creates long names that obscure the business API.

### Project-local copied dependencies

Example: clone every dependency under `vendor/`.

This is transparent and offline-friendly but duplicates source across projects, increases repository size and makes accidental dependency edits likely. It is useful as an explicit export mode, not the default store.

### Stable identity plus independent source

Example identity: `shemic.contact`; example source: `https://github.com/shemic/contact.git` at an exact resolved commit.

This preserves short, stable `.dever` names while allowing registry, Git, mirrors and local development to use the same package identity. It requires dependency metadata and identity verification, but those responsibilities belong to the package manager rather than business source.

## Recommendation

Use the third model:

1. `component id` is the stable namespace and type-identity root.
2. `source` is a replaceable registry, Git or local transport selected outside `.dever`.
3. `dever.mod` records direct dependency intent and source requirements.
4. `dever.lock` records the complete graph, full Git commits and normalized content hashes.
5. A global immutable content-addressed store deduplicates verified source across projects.
6. `check`, `build` and `run` never perform implicit network access; explicit `add`, `update` and `fetch` do.
7. Package declarations must remain beneath the component root, preventing one dependency from supplying another component's namespace.

This deliberately combines Go's small module/package model and shared verified cache with Cargo's source flexibility and exact Git locking, while avoiding Go's coupling between source-level identity and repository location.
