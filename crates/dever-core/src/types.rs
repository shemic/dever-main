use std::collections::VecDeque;
use std::sync::OnceLock;

use crate::source::Span;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Type {
    Unit,
    Bool,
    Int,
    Decimal,
    Float,
    Text,
    Id,
    Bytes,
    Secret,
    Upload,
    Uuid,
    DateTime,
    Date,
    Time,
    Duration,
    Json,
    File,
    Socket,
    Listener,
    HttpReply,
    WebSocket,
    ClientTls,
    ServerTls,
    HttpClient,
    Stream(Box<Type>),
    AsyncStream(Box<Type>),
    /// Database-backed stream. It is rendered as AsyncStream in the source contract.
    RowStream(Box<Type>),
    Related(Box<Type>),
    Channel(Box<Type>),
    Task(Vec<Field>),
    Group,
    Named(usize),
    Nullable(Box<Type>),
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    MapEntry(Box<Type>, Box<Type>),
    Outputs(Vec<Field>),
}

impl Type {
    pub fn nullable(self) -> Self {
        if matches!(self, Self::Nullable(_)) {
            self
        } else {
            Self::Nullable(Box::new(self))
        }
    }

    pub fn base(&self) -> &Self {
        match self {
            Self::Nullable(base) => base,
            _ => self,
        }
    }

    pub fn numeric(&self) -> bool {
        matches!(self, Self::Int | Self::Decimal | Self::Float)
    }

    pub fn value(&self) -> bool {
        !matches!(self, Self::Unit | Self::Outputs(_))
    }

    pub fn comparable(&self, definitions: &[Definition]) -> bool {
        self.properties(definitions).comparable
    }

    pub fn transferable(&self, definitions: &[Definition]) -> bool {
        self.properties(definitions).transferable
    }

    pub fn movable(&self, definitions: &[Definition]) -> bool {
        // 资源的最后别名必须保留到源码作用域结束，避免提前发出可观察的 EOF。
        self.properties(definitions).movable
    }

    pub fn observable(&self, definitions: &[Definition]) -> bool {
        let (secret, private_model) = self.sensitivity(definitions);
        !secret && !private_model
    }

    pub fn contains_private_model(&self, definitions: &[Definition]) -> bool {
        self.sensitivity(definitions).1
    }

    fn sensitivity(&self, definitions: &[Definition]) -> (bool, bool) {
        let mut sensitive = (false, false);
        visit_type(self, &mut |ty| match ty {
            Self::Secret | Self::Upload => sensitive.0 = true,
            Self::Named(id) => {
                let properties = definitions[*id]
                    .properties
                    .get()
                    .expect("cached type properties");
                sensitive.0 |= properties.secret;
                sensitive.1 |= properties.private_model;
            }
            _ => {}
        });
        sensitive
    }

    fn properties(&self, definitions: &[Definition]) -> Properties {
        match self {
            Self::Named(id) => *definitions[*id]
                .properties
                .get()
                .expect("validated nominal type properties"),
            Self::Nullable(base) | Self::List(base) => base.properties(definitions),
            // Related is an ORM-owned indirection. Its payload is always a Model,
            // nullable Model, or bounded List<Model>, so recursive Model graphs do
            // not participate in nominal layout/property cycles.
            Self::Related(_) => Properties::ALL,
            Self::Map(key, value) | Self::MapEntry(key, value) => key
                .properties(definitions)
                .intersection(value.properties(definitions)),
            Self::Channel(element) | Self::AsyncStream(element) => Properties {
                comparable: false,
                transferable: element.transferable(definitions),
                movable: false,
                ..Properties::ALL
            },
            Self::RowStream(element) => Properties {
                comparable: false,
                transferable: element.transferable(definitions),
                movable: false,
                ..Properties::ALL
            },
            Self::Task(_) | Self::Group | Self::Upload => Properties {
                comparable: false,
                transferable: false,
                movable: true,
                ..Properties::ALL
            },
            Self::File
            | Self::Socket
            | Self::Listener
            | Self::HttpReply
            | Self::WebSocket
            | Self::ClientTls
            | Self::ServerTls
            | Self::HttpClient => Properties {
                comparable: false,
                transferable: true,
                movable: false,
                ..Properties::ALL
            },
            Self::Stream(_) => Properties {
                comparable: false,
                transferable: false,
                movable: false,
                ..Properties::ALL
            },
            Self::Unit | Self::Outputs(_) => Properties {
                comparable: false,
                transferable: false,
                movable: match self {
                    Self::Outputs(fields) => {
                        fields.iter().all(|field| field.ty.movable(definitions))
                    }
                    _ => true,
                },
                ..Properties::ALL
            },
            Self::Secret => Properties {
                comparable: false,
                secret: true,
                ..Properties::ALL
            },
            _ => Properties::ALL,
        }
    }

    pub fn label(&self, types: &[Definition]) -> String {
        match self {
            Self::Named(id) => types[*id].name.clone(),
            Self::Nullable(base) => format!("{}?", base.label(types)),
            Self::List(element) => format!("List<{}>", element.label(types)),
            Self::Map(key, value) => format!("Map<{}, {}>", key.label(types), value.label(types)),
            Self::MapEntry(key, value) => {
                format!("MapEntry<{}, {}>", key.label(types), value.label(types))
            }
            Self::Stream(element) => format!("Stream<{}>", element.label(types)),
            Self::AsyncStream(element) => format!("AsyncStream<{}>", element.label(types)),
            Self::RowStream(element) => format!("AsyncStream<{}>", element.label(types)),
            Self::Related(element) => format!("Related<{}>", element.label(types)),
            Self::Channel(element) => format!("Channel<{}>", element.label(types)),
            Self::Task(fields) => format!(
                "Task<({})>",
                fields
                    .iter()
                    .map(|field| format!("{}: {}", field.name, field.ty.label(types)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Group => "Group".into(),
            Self::Outputs(_) => "named outputs".into(),
            _ => format!("{self:?}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HandlerSignature {
    pub parameters: Vec<Type>,
    pub bounds: Vec<crate::hir::Domain>,
    pub outputs: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Parameter {
    Value(Type),
    Handler(HandlerSignature),
}

impl Parameter {
    pub fn value_type(&self) -> Option<&Type> {
        match self {
            Self::Value(ty) => Some(ty),
            Self::Handler(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Field {
    pub name: String,
    pub ty: Type,
    pub private: bool,
    pub bounds: crate::hir::Domain,
}

#[derive(Clone, Debug)]
pub(crate) struct Variant {
    pub name: String,
    pub fields: Vec<Field>,
    pub error: bool,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum Shape {
    Record(Vec<Field>),
    Choice(Vec<Variant>),
}

#[derive(Clone, Debug)]
pub(crate) struct Definition {
    pub name: String,
    pub owner: usize,
    pub public: bool,
    pub shape: Shape,
    pub span: Span,
    pub properties: OnceLock<Properties>,
    pub kind: DefinitionKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DefinitionKind {
    Regular,
    Model,
    ModelId,
    ModelChoice,
    ModelPage,
    ModelCursor,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Properties {
    comparable: bool,
    transferable: bool,
    movable: bool,
    secret: bool,
    private_model: bool,
}

impl Properties {
    const ALL: Self = Self {
        comparable: true,
        transferable: true,
        movable: true,
        secret: false,
        private_model: false,
    };

    fn intersection(self, other: Self) -> Self {
        Self {
            comparable: self.comparable && other.comparable,
            transferable: self.transferable && other.transferable,
            movable: self.movable && other.movable,
            secret: self.secret || other.secret,
            private_model: self.private_model || other.private_model,
        }
    }
}

pub(crate) fn cache_properties(definitions: &[Definition], dependencies: &[Vec<(usize, Span)>]) {
    let sensitivity = nominal_sensitivity(definitions);
    // 类型环已经被拒绝。按依赖顺序计算一次，避免展开共享 DAG 或递归宿主调用栈。
    let mut callers = vec![Vec::new(); definitions.len()];
    let mut pending: Vec<_> = dependencies.iter().map(Vec::len).collect();
    for (caller, references) in dependencies.iter().enumerate() {
        for (callee, _) in references {
            callers[*callee].push(caller);
        }
    }
    let mut ready: VecDeque<_> = pending
        .iter()
        .enumerate()
        .filter_map(|(id, count)| (*count == 0).then_some(id))
        .collect();
    while let Some(id) = ready.pop_front() {
        let mut properties = Properties::ALL;
        let mut include = |field: &Field| {
            properties = properties.intersection(field.ty.properties(definitions));
        };
        match &definitions[id].shape {
            Shape::Record(fields) => fields.iter().for_each(&mut include),
            Shape::Choice(variants) => variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .for_each(&mut include),
        }
        (properties.secret, properties.private_model) = sensitivity[id];
        definitions[id]
            .properties
            .set(properties)
            .expect("type properties initialized once after validation");
        for caller in &callers[id] {
            pending[*caller] -= 1;
            if pending[*caller] == 0 {
                ready.push_back(*caller);
            }
        }
    }
}

/// Walk container structure, stopping at nominal identities. Related edges participate
/// in sensitivity propagation without expanding cyclic ORM association graphs.
pub(crate) fn visit_type(ty: &Type, visit: &mut impl FnMut(&Type)) {
    visit(ty);
    match ty {
        Type::Nullable(inner)
        | Type::List(inner)
        | Type::Related(inner)
        | Type::Stream(inner)
        | Type::AsyncStream(inner)
        | Type::RowStream(inner)
        | Type::Channel(inner) => visit_type(inner, visit),
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            visit_type(key, visit);
            visit_type(value, visit);
        }
        Type::Task(fields) | Type::Outputs(fields) => {
            for field in fields {
                visit_type(&field.ty, visit);
            }
        }
        _ => {}
    }
}

fn nominal_sensitivity(definitions: &[Definition]) -> Vec<(bool, bool)> {
    let mut sensitivity = vec![(false, false); definitions.len()];
    let mut callers = vec![Vec::new(); definitions.len()];
    for (id, definition) in definitions.iter().enumerate() {
        let mut include = |field: &Field| {
            sensitivity[id].1 |= definition.kind == DefinitionKind::Model && field.private;
            visit_type(&field.ty, &mut |ty| match ty {
                Type::Secret => sensitivity[id].0 = true,
                Type::Named(target) => callers[*target].push(id),
                _ => {}
            });
        };
        match &definition.shape {
            Shape::Record(fields) => fields.iter().for_each(&mut include),
            Shape::Choice(variants) => variants
                .iter()
                .flat_map(|v| &v.fields)
                .for_each(&mut include),
        }
    }
    let mut ready: VecDeque<_> = sensitivity
        .iter()
        .enumerate()
        .filter_map(|(id, (secret, private))| (*secret || *private).then_some(id))
        .collect();
    while let Some(id) = ready.pop_front() {
        for caller in &callers[id] {
            let combined = (
                sensitivity[*caller].0 || sensitivity[id].0,
                sensitivity[*caller].1 || sensitivity[id].1,
            );
            if combined != sensitivity[*caller] {
                sensitivity[*caller] = combined;
                ready.push_back(*caller);
            }
        }
    }
    sensitivity
}

pub(crate) fn output_type(fields: &[Field]) -> Type {
    match fields {
        [] => Type::Unit,
        [field] => field.ty.clone(),
        _ => Type::Outputs(fields.to_vec()),
    }
}
