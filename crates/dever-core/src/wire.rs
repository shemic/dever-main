//! One checked schema owns eligibility and concrete codec generation for all wire boundaries.
use std::collections::{BTreeMap, BTreeSet};

use crate::hir::Program;
use crate::types::{Definition, DefinitionKind, Field, Shape, Type};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Policy {
    Output,
    ApiInput,
    SettingInput,
    Job,
    Command,
}

impl Policy {
    pub(crate) fn external(ecosystem: crate::syntax::ExternalEcosystem) -> Self {
        match ecosystem {
            crate::syntax::ExternalEcosystem::Command => Self::Command,
            _ => Self::Output,
        }
    }

    pub(crate) fn permits_secret(self) -> bool {
        matches!(self, Self::ApiInput | Self::SettingInput)
    }
}

#[derive(Clone, Debug)]
pub struct Schema {
    pub(crate) policy: Policy,
    pub(crate) nodes: Vec<Node>,
    pub(crate) root: usize,
    identity: String,
}

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Bytes,
    Bool,
    Int,
    Float,
    Decimal,
    Text,
    Id,
    ModelId {
        id: usize,
        name: String,
    },
    Uuid,
    DateTime,
    Date,
    Time,
    Duration,
    Json,
    Secret,
    Nullable(usize),
    List(usize),
    Record {
        id: usize,
        name: String,
        fields: Vec<(String, usize)>,
    },
}

impl Schema {
    /// HTTP query conversion precedes decoding the concrete wire value.
    pub(crate) fn query_kind(&self) -> &'static str {
        let node = match &self.nodes[self.root] {
            Node::Nullable(inner) => &self.nodes[*inner],
            node => node,
        };
        match node {
            Node::Int | Node::ModelId { .. } | Node::Duration => "Int",
            Node::Bool => "Bool",
            Node::Float => "Float",
            _ => "Text",
        }
    }

    /// Deterministic protocol identity; separate from the persistent Model fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.identity
    }

    pub(crate) fn worker_type(&self) -> serde_json::Value {
        self.worker_node(self.root)
    }

    fn worker_node(&self, id: usize) -> serde_json::Value {
        use serde_json::json;
        match &self.nodes[id] {
            Node::Bytes => json!({"type": "bytes", "encoding": "base64"}),
            Node::Bool => json!({"type": "bool"}),
            Node::Int => json!({"type": "int64"}),
            Node::Float => json!({"type": "float64"}),
            Node::Decimal => json!({"type": "decimal"}),
            Node::Text => json!({"type": "text"}),
            Node::Id => json!({"type": "id"}),
            Node::ModelId { name, .. } => json!({"type": "model_id", "name": name}),
            Node::Uuid => json!({"type": "uuid"}),
            Node::DateTime => json!({"type": "datetime"}),
            Node::Date => json!({"type": "date"}),
            Node::Time => json!({"type": "time"}),
            Node::Duration => json!({"type": "duration"}),
            Node::Json => json!({"type": "json"}),
            Node::Secret => json!({"type": "secret"}),
            Node::Nullable(child) => json!({"type": "nullable", "value": self.worker_node(*child)}),
            Node::List(child) => json!({"type": "list", "value": self.worker_node(*child)}),
            Node::Record { name, fields, .. } => json!({
                "type": "record", "name": name,
                "fields": fields.iter().map(|(name, child)| (name.clone(), self.worker_node(*child)))
                    .collect::<BTreeMap<_, _>>()
            }),
        }
    }

    pub(crate) fn from_field(
        field: &Field,
        types: &[Definition],
        policy: Policy,
    ) -> Result<Self, String> {
        validate_field(field)?;
        Self::build(&field.ty, types, policy)
    }

    pub(crate) fn build(ty: &Type, types: &[Definition], policy: Policy) -> Result<Self, String> {
        let mut builder = Builder {
            types,
            policy,
            nodes: Vec::new(),
            cached: BTreeMap::new(),
            visiting: BTreeSet::new(),
        };
        let root = builder.node(ty, 0)?;
        // A memoized child can occur at a deeper path than its first occurrence.
        // Compute the longest DAG path after interning, rather than trusting visit depth.
        let mut heights = Vec::with_capacity(builder.nodes.len());
        for node in &builder.nodes {
            let height = match node {
                Node::Nullable(child) | Node::List(child) => heights[*child] + 1,
                Node::Record { fields, .. } => fields
                    .iter()
                    .map(|(_, child)| heights[*child] + 1)
                    .max()
                    .unwrap_or(1),
                _ => 0,
            };
            heights.push(height);
        }
        if heights[root] > 64 {
            return Err("wire schema exceeds depth limit".into());
        }
        let mut identity = format!("wire-v1:{policy:?}:");
        for node in &builder.nodes {
            match node {
                Node::Record { name, fields, .. } => {
                    identity.push_str(&format!("Record({name:?},{fields:?});"))
                }
                Node::ModelId { name, .. } => identity.push_str(&format!("ModelId({name:?});")),
                other => identity.push_str(&format!("{other:?};")),
            }
        }
        Ok(Self {
            policy,
            nodes: builder.nodes,
            root,
            identity,
        })
    }
}

impl Program {
    /// Inspect a checked nominal type without reinterpreting source declarations.
    pub fn wire_schema(&self, name: &str, policy: Policy) -> Result<Schema, String> {
        let id = self
            .types
            .iter()
            .position(|ty| ty.name == name)
            .ok_or_else(|| format!("unknown wire type {name}"))?;
        Schema::build(&Type::Named(id), &self.types, policy)
    }
}

struct Builder<'a> {
    types: &'a [Definition],
    policy: Policy,
    nodes: Vec<Node>,
    cached: BTreeMap<String, usize>,
    visiting: BTreeSet<usize>,
}

impl Builder<'_> {
    fn node(&mut self, ty: &Type, depth: usize) -> Result<usize, String> {
        if depth > 64 {
            return Err("wire schema exceeds depth limit".into());
        }
        let key = ty.label(self.types);
        if let Some(id) = self.cached.get(&key) {
            return Ok(*id);
        }
        if !self.policy.permits_secret() && !ty.observable(self.types) {
            return Err(
                "wire output and Job payload cannot contain Secret or private Model data".into(),
            );
        }
        let node = match ty {
            Type::Bytes if self.policy == Policy::Command => Node::Bytes,
            Type::Bool => Node::Bool,
            Type::Int => Node::Int,
            Type::Float => Node::Float,
            Type::Decimal => Node::Decimal,
            Type::Text => Node::Text,
            Type::Id => Node::Id,
            Type::Uuid => Node::Uuid,
            Type::DateTime => Node::DateTime,
            Type::Date => Node::Date,
            Type::Time => Node::Time,
            Type::Duration => Node::Duration,
            Type::Json => Node::Json,
            Type::Secret if self.policy.permits_secret() => Node::Secret,
            Type::Nullable(inner) => Node::Nullable(self.node(inner, depth + 1)?),
            Type::List(inner) => Node::List(self.node(inner, depth + 1)?),
            Type::Named(id) if self.types[*id].kind == DefinitionKind::ModelId => Node::ModelId {
                id: *id,
                name: self.types[*id].name.clone(),
            },
            Type::Named(id) => {
                let definition = &self.types[*id];
                if !self.visiting.insert(*id) {
                    return Err(format!("unsupported wire type {key}"));
                }
                let Shape::Record(fields) = &definition.shape else {
                    return Err("wire Choice is not supported".into());
                };
                let mut wire_fields = Vec::new();
                for field in fields {
                    validate_field(field)?;
                    wire_fields.push((field.name.clone(), self.node(&field.ty, depth + 1)?));
                }
                self.visiting.remove(id);
                Node::Record {
                    id: *id,
                    name: definition.name.clone(),
                    fields: wire_fields,
                }
            }
            _ => return Err(format!("unsupported wire type {key}")),
        };
        let id = self.nodes.len();
        self.nodes.push(node);
        self.cached.insert(key, id);
        Ok(id)
    }
}

fn validate_field(field: &Field) -> Result<(), String> {
    if field.private || !field.bounds.is_empty() {
        return Err("wire fields must be public and unconstrained".into());
    }
    Ok(())
}
