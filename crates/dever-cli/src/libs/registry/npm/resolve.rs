use super::*;
use crate::libs::registry::{MAX_PACKAGES, MAX_RESOLUTION_ATTEMPTS, RegistryResolver, SearchError};

#[derive(Clone)]
struct Request {
    consumer: Option<String>,
    edge: Edge,
}

#[derive(Clone, Default)]
struct State {
    nodes: BTreeMap<String, Instance>,
    pending: VecDeque<Request>,
}

struct Catalog<'a, 'r> {
    resolver: &'a RegistryResolver<'r>,
    runtime: &'a RegistryRuntime,
    indexes: BTreeMap<String, Vec<String>>,
    metadata: BTreeMap<(String, String), Value>,
    archives: BTreeMap<LibSpec, (LockedLib, Archive)>,
    attempts: usize,
    archive_bytes: usize,
}

pub(crate) fn resolve(
    resolver: &RegistryResolver<'_>,
    requested: &[LibSpec],
    runtime: &RegistryRuntime,
) -> Result<(Vec<LockedLib>, Environment), String> {
    let roots = roots(requested);
    let mut state = State::default();
    for spec in &roots {
        state.pending.push_back(Request {
            consumer: None,
            edge: Edge {
                name: spec.name.clone(),
                requirement: format!("={}", spec.version),
                kind: Kind::Dependency,
                target: None,
                omission: None,
            },
        });
    }
    let mut catalog = Catalog {
        resolver,
        runtime,
        indexes: BTreeMap::new(),
        metadata: BTreeMap::new(),
        archives: BTreeMap::new(),
        attempts: 0,
        archive_bytes: 0,
    };
    let mut state = catalog.search(state).map_err(SearchError::message)?;
    let mut pending = roots
        .iter()
        .map(|root| child("", &root.name))
        .collect::<VecDeque<_>>();
    let mut reached = BTreeSet::new();
    while let Some(path) = pending.pop_front() {
        if !reached.insert(path.clone()) {
            continue;
        }
        pending.extend(
            state.nodes[&path]
                .edges
                .iter()
                .filter_map(|edge| edge.target.clone()),
        );
    }
    state.nodes.retain(|path, _| reached.contains(path));
    let used = state
        .nodes
        .values()
        .map(|node| node.source.archive.clone())
        .collect::<BTreeSet<_>>();
    let libraries = used
        .into_iter()
        .map(|spec| {
            catalog
                .archives
                .remove(&spec)
                .expect("resolved npm archive")
                .0
        })
        .collect();
    Ok((
        libraries,
        Environment {
            build: None,
            roots,
            instances: state.nodes.into_values().collect(),
        },
    ))
}

impl Catalog<'_, '_> {
    fn archive(&mut self, spec: &LibSpec) -> Result<(), SearchError> {
        if self.archives.contains_key(spec) {
            return Ok(());
        }
        let document = self
            .resolver
            .registry_metadata(
                Ecosystem::Npm,
                &spec.name,
                &spec.version,
                &mut self.metadata,
            )
            .map_err(SearchError::Fatal)?;
        let bytes = self
            .resolver
            .npm_archive(spec, &document)
            .map_err(SearchError::Fatal)?;
        let archive = Archive::parse(spec, &bytes).map_err(SearchError::Fatal)?;
        self.archive_bytes = self
            .archive_bytes
            .checked_add(
                archive
                    .files
                    .iter()
                    .map(|(_, bytes)| bytes.len())
                    .sum::<usize>(),
            )
            .ok_or_else(|| SearchError::Fatal("npm archive cache size overflow".into()))?;
        if self.archive_bytes > 256 * 1024 * 1024 || self.archives.len() >= MAX_PACKAGES {
            return Err(SearchError::Fatal(
                "npm archive cache exceeds its 256 MiB/package budget".into(),
            ));
        }
        let lib = self
            .resolver
            .lock_archive(
                spec,
                Vec::new(),
                self.runtime,
                "tgz",
                &bytes,
                "npm-instances-v1",
            )
            .map_err(SearchError::Fatal)?;
        self.archives.insert(spec.clone(), (lib, archive));
        Ok(())
    }

    fn metadata(&self, node: &Instance) -> &Metadata {
        &self.archives[&node.source.archive].1.packages[&node.source.prefix]
    }

    fn bind(
        state: &mut State,
        request: &Request,
        target: Option<String>,
        omission: Option<Omission>,
    ) {
        if let Some(consumer) = &request.consumer {
            let edge = state
                .nodes
                .get_mut(consumer)
                .expect("pending npm consumer")
                .edges
                .iter_mut()
                .find(|edge| edge.name == request.edge.name && edge.kind == request.edge.kind)
                .expect("pending npm edge");
            edge.target = target;
            edge.omission = omission;
        }
    }

    fn add(&self, state: &mut State, path: &str, spec: &LibSpec) -> Result<(), SearchError> {
        for (prefix, metadata) in &self.archives[spec].1.packages {
            let relative = prefix
                .strip_prefix("package/")
                .unwrap()
                .trim_end_matches('/');
            let location = if relative.is_empty() {
                path.into()
            } else {
                format!("{path}/{relative}")
            };
            let instance = Instance {
                path: location.clone(),
                spec: metadata.spec.clone(),
                source: Source {
                    archive: spec.clone(),
                    prefix: prefix.clone(),
                },
                bundled: if relative.is_empty() {
                    self.archives[spec].1.bundled()
                } else {
                    BTreeMap::new()
                },
                edges: metadata.edges.clone(),
            };
            if state.nodes.insert(location.clone(), instance).is_some() {
                return Err(SearchError::Conflict(
                    "npm package installation paths collide".into(),
                ));
            }
            for edge in &metadata.edges {
                if edge.kind != Kind::OptionalPeer {
                    state.pending.push_back(Request {
                        consumer: Some(location.clone()),
                        edge: edge.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn prune_bundled_orphans(&self, state: &mut State, root: &str) {
        let mut pending = VecDeque::from([root.to_owned()]);
        let mut reached = BTreeSet::new();
        while let Some(path) = pending.pop_front() {
            if !reached.insert(path.clone()) {
                continue;
            }
            let Some(node) = state.nodes.get(&path) else {
                continue;
            };
            for edge in &self.metadata(node).edges {
                if let Some(found) = visible(
                    &state.nodes,
                    &path,
                    &edge.name,
                    matches!(edge.kind, Kind::Peer | Kind::OptionalPeer),
                ) {
                    pending.push_back(found.path.clone());
                }
            }
        }
        let prefix = format!("{root}/");
        state.nodes.retain(|path, node| {
            node.source.prefix == "package/" || !path.starts_with(&prefix) || reached.contains(path)
        });
    }

    fn search(&mut self, mut state: State) -> Result<State, SearchError> {
        self.attempts += 1;
        if self.attempts > MAX_RESOLUTION_ATTEMPTS
            || state.nodes.len() > MAX_PACKAGES
            || state.nodes.keys().any(|path| path.len() > 4096)
        {
            return Err(SearchError::Fatal(
                "npm instance graph exceeds its resolution budget".into(),
            ));
        }
        while let Some(request) = state.pending.pop_front() {
            if request
                .consumer
                .as_ref()
                .is_some_and(|consumer| !state.nodes.contains_key(consumer))
            {
                continue;
            }
            let consumer = request.consumer.as_deref().unwrap_or("");
            let peer = request.edge.kind == Kind::Peer;
            let scope = if peer { parent(consumer) } else { consumer };
            let location = child(scope, &request.edge.name);
            let rule = request
                .edge
                .requirement
                .parse::<Range>()
                .map_err(|error| SearchError::Fatal(error.to_string()))?;
            let existing = visible(&state.nodes, consumer, &request.edge.name, peer).cloned();
            if let Some(found) = &existing {
                let eligible = self
                    .metadata(found)
                    .eligible(self.runtime)
                    .map_err(SearchError::Fatal)?;
                if !eligible
                    && request.edge.kind == Kind::Optional
                    && found.source.prefix != "package/"
                {
                    let prefix = format!("{}/", found.path);
                    state
                        .nodes
                        .retain(|path, _| path != &found.path && !path.starts_with(&prefix));
                    let relative = found
                        .source
                        .prefix
                        .strip_prefix("package/")
                        .unwrap()
                        .trim_end_matches('/');
                    let owner = found
                        .path
                        .strip_suffix(relative)
                        .unwrap()
                        .trim_end_matches('/');
                    self.prune_bundled_orphans(&mut state, owner);
                    Self::bind(&mut state, &request, None, Some(Omission::Target));
                    continue;
                }
                // Ordinary dependencies get a private nested slot. Reuse an
                // ancestor only to close a real package cycle, never a sibling's
                // independently resolved peer context.
                let bundled = state.nodes.get(consumer).is_some_and(|node| {
                    node.source.prefix != "package/" && node.source.archive == found.source.archive
                });
                let reusable = peer
                    || bundled
                    || found.path == location
                    || consumer == found.path
                    || consumer.starts_with(&format!("{}/", found.path));
                if reusable
                    && eligible
                    && Version::from_str(&found.spec.version)
                        .is_ok_and(|version| version.satisfies(&rule))
                {
                    Self::bind(&mut state, &request, Some(found.path.clone()), None);
                    continue;
                }
                if found.path == location {
                    return Err(SearchError::Conflict(format!(
                        "npm dependency/peer conflict for {} at {location}",
                        request.edge.name
                    )));
                }
            }
            if let Some(node) = request
                .consumer
                .as_ref()
                .and_then(|consumer| state.nodes.get(consumer))
                && node.source.prefix != "package/"
                && !peer
            {
                if request.edge.kind == Kind::Optional && existing.is_none() {
                    Self::bind(&mut state, &request, None, Some(Omission::Unavailable));
                    continue;
                }
                return Err(SearchError::Fatal(
                    "npm bundled dependency closure is incomplete".into(),
                ));
            }
            let versions = if let Some(versions) = self.indexes.get(&request.edge.name) {
                versions.clone()
            } else {
                let versions = self
                    .resolver
                    .registry_versions(Ecosystem::Npm, &request.edge.name)
                    .map_err(SearchError::Fatal)?;
                self.indexes
                    .insert(request.edge.name.clone(), versions.clone());
                versions
            };
            let mut reason = Omission::Unavailable;
            for version in versions.into_iter().filter(|version| {
                Version::from_str(version).is_ok_and(|version| version.satisfies(&rule))
            }) {
                self.attempts += 1;
                if self.attempts > MAX_RESOLUTION_ATTEMPTS {
                    return Err(SearchError::Fatal(
                        "npm resolution exceeds its candidate budget".into(),
                    ));
                }
                let spec = LibSpec {
                    ecosystem: Ecosystem::Npm,
                    name: request.edge.name.clone(),
                    version,
                };
                self.archive(&spec)?;
                if !self.archives[&spec].1.packages["package/"]
                    .eligible(self.runtime)
                    .map_err(SearchError::Fatal)?
                {
                    reason = Omission::Target;
                    continue;
                }
                reason = Omission::Conflict;
                let mut candidate = state.clone();
                self.add(&mut candidate, &location, &spec)?;
                Self::bind(&mut candidate, &request, Some(location.clone()), None);
                match self.search(candidate) {
                    Ok(resolved) => return Ok(resolved),
                    Err(SearchError::Conflict(_)) => {}
                    Err(fatal) => return Err(fatal),
                }
            }
            if request.edge.kind == Kind::Optional && existing.is_none() {
                Self::bind(&mut state, &request, None, Some(reason));
                continue;
            }
            return Err(SearchError::Conflict(format!(
                "no npm version satisfies {} at {location}",
                request.edge.name
            )));
        }
        let edges = state
            .nodes
            .values()
            .flat_map(|node| {
                node.edges.iter().map(|edge| Request {
                    consumer: Some(node.path.clone()),
                    edge: edge.clone(),
                })
            })
            .collect::<Vec<_>>();
        for request in edges {
            let found = visible(
                &state.nodes,
                request.consumer.as_deref().unwrap(),
                &request.edge.name,
                matches!(request.edge.kind, Kind::Peer | Kind::OptionalPeer),
            );
            if let Some(found) = found {
                let range = request
                    .edge
                    .requirement
                    .parse::<Range>()
                    .map_err(|error| SearchError::Fatal(error.to_string()))?;
                if !Version::from_str(&found.spec.version)
                    .is_ok_and(|version| version.satisfies(&range))
                {
                    return Err(SearchError::Conflict(
                        "npm dependency or peer conflicts with final Node lookup".into(),
                    ));
                }
                let target = found.path.clone();
                Self::bind(&mut state, &request, Some(target), None);
            } else if request.edge.kind == Kind::OptionalPeer {
                Self::bind(
                    &mut state,
                    &request,
                    None,
                    Some(Omission::OptionalPeerAbsent),
                );
            } else if request.edge.kind != Kind::Optional || request.edge.omission.is_none() {
                return Err(SearchError::Conflict(
                    "npm required dependency is absent from final Node lookup".into(),
                ));
            }
        }
        Ok(state)
    }
}
