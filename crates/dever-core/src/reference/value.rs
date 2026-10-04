use crate::{
    hir::{Constant, Program},
    types::Shape,
};
use dever_runtime::{
    Id,
    bytes::Bytes,
    collections::{List, Map},
    number::DecimalValue,
    orm::Uuid,
    render::Render,
    resource::{FileHandle, Stream},
};

#[derive(Clone, Debug)]
pub(super) enum Value {
    Unit,
    Null,
    Bool(bool),
    Int(i64),
    Decimal(DecimalValue),
    Float(f64),
    Text(String),
    Id(Id),
    Uuid(Uuid),
    Bytes(Bytes),
    Secret(dever_runtime::secret::Secret),
    Record(usize, Vec<Value>),
    Variant(usize, usize, Vec<Value>),
    Fields(Vec<Value>),
    List(List<Value>),
    Map(Map<Key, Value>),
    File(FileHandle),
    Stream(Stream<Value>),
}

/// Only statically eligible Map keys cross the runtime's Eq + Hash boundary.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) enum Key {
    Bool(bool),
    Int(i64),
    Text(String),
    Id(Id),
    Variant(usize, usize),
}

impl Key {
    pub fn from_value(value: &Value) -> Self {
        match value {
            Value::Bool(v) => Self::Bool(*v),
            Value::Int(v) => Self::Int(*v),
            Value::Text(v) => Self::Text(v.clone()),
            Value::Id(v) => Self::Id(v.clone()),
            Value::Variant(id, variant, fields) if fields.is_empty() => {
                Self::Variant(*id, *variant)
            }
            _ => unreachable!("checked Map key"),
        }
    }
    pub fn value(&self) -> Value {
        match self {
            Self::Bool(v) => Value::Bool(*v),
            Self::Int(v) => Value::Int(*v),
            Self::Text(v) => Value::Text(v.clone()),
            Self::Id(v) => Value::Id(v.clone()),
            Self::Variant(id, variant) => Value::Variant(*id, *variant, vec![]),
        }
    }
}

impl Value {
    pub fn secret(&self) -> &dever_runtime::secret::Secret {
        match self {
            Self::Secret(secret) => secret,
            _ => unreachable!("checked Secret"),
        }
    }
    pub fn file(&self) -> &FileHandle {
        match self {
            Self::File(file) => file,
            _ => unreachable!("checked File"),
        }
    }

    pub fn constant(value: &Constant) -> Self {
        match value {
            Constant::Null => Self::Null,
            Constant::Bool(v) => Self::Bool(*v),
            Constant::Int(v) => Self::Int(*v),
            Constant::Decimal(v) => Self::Decimal(*v),
            Constant::Float(v) => Self::Float(*v),
            Constant::Text(v) => Self::Text(v.clone()),
        }
    }
    pub fn fields(&self) -> &[Self] {
        match self {
            Self::Record(_, fields) | Self::Variant(_, _, fields) | Self::Fields(fields) => fields,
            _ => unreachable!("checked fields"),
        }
    }
    pub fn fields_mut(&mut self) -> &mut Vec<Self> {
        match self {
            Self::Record(_, fields) | Self::Fields(fields) => fields,
            _ => unreachable!("checked mutable fields"),
        }
    }
    pub fn int(&self) -> i64 {
        match self {
            Self::Int(v) => *v,
            _ => unreachable!("checked Int"),
        }
    }
    pub fn float(&self) -> f64 {
        match self {
            Self::Float(v) => *v,
            _ => unreachable!("checked Float"),
        }
    }
    pub fn text(&self) -> &str {
        match self {
            Self::Text(v) => v,
            _ => unreachable!("checked Text"),
        }
    }
    pub fn bytes(&self) -> &Bytes {
        match self {
            Self::Bytes(v) => v,
            _ => unreachable!("checked Bytes"),
        }
    }
    pub fn uuid(&self) -> Uuid {
        match self {
            Self::Uuid(v) => *v,
            _ => unreachable!("checked Uuid"),
        }
    }
    pub fn boolean(&self) -> bool {
        match self {
            Self::Bool(v) => *v,
            _ => unreachable!("checked Bool"),
        }
    }
    pub fn decimal(&self) -> DecimalValue {
        match self {
            Self::Decimal(v) => *v,
            _ => unreachable!("checked Decimal"),
        }
    }
    pub fn render(&self, program: &Program) -> String {
        let joined = |values: &[Value]| {
            values
                .iter()
                .map(|v| v.render(program))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self {
            Self::Unit => "()".into(),
            Self::Null => "null".into(),
            Self::Bool(v) => v.render(),
            Self::Int(v) => v.render(),
            Self::Decimal(v) => v.render(),
            Self::Float(v) => v.render(),
            Self::Text(v) => v.clone(),
            Self::Id(v) => v.render(),
            Self::Uuid(v) => v.to_string(),
            Self::Bytes(v) => v.render(),
            Self::Secret(_) => unreachable!("Secret cannot cross a checked render boundary"),
            Self::List(v) => format!("[{}]", joined(v.values())),
            Self::Map(v) => format!(
                "{{{}}}",
                v.pairs()
                    .map(|(key, value)| format!(
                        "{} = {}",
                        key.value().render(program),
                        value.render(program)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Fields(v) => format!(
                "{{key = {}, value = {}}}",
                v[0].render(program),
                v[1].render(program)
            ),
            Self::Record(id, values) => {
                let definition = &program.types[*id];
                let Shape::Record(fields) = &definition.shape else {
                    unreachable!()
                };
                let fields = fields
                    .iter()
                    .zip(values)
                    .map(|(field, value)| format!("{} = {}", field.name, value.render(program)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{} {{ {fields} }}", definition.name)
            }
            Self::Variant(id, variant, values) => {
                let definition = &program.types[*id];
                let Shape::Choice(variants) = &definition.shape else {
                    unreachable!()
                };
                let name = format!("{}.{}", definition.name, variants[*variant].name);
                if values.is_empty() {
                    name
                } else {
                    format!("{name}({})", joined(values))
                }
            }
            Self::File(_) => "<resource>".into(),
            Self::Stream(_) => "<stream>".into(),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unit, Self::Unit) | (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Decimal(a), Self::Decimal(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b,
            (Self::Text(a), Self::Text(b)) => a == b,
            (Self::Id(a), Self::Id(b)) => a == b,
            (Self::Uuid(a), Self::Uuid(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Map(a), Self::Map(b)) => a == b,
            (Self::Fields(a), Self::Fields(b)) => a == b,
            (Self::Record(a, av), Self::Record(b, bv)) => a == b && av == bv,
            (Self::Variant(a, ai, av), Self::Variant(b, bi, bv)) => a == b && ai == bi && av == bv,
            _ => false,
        }
    }
}
