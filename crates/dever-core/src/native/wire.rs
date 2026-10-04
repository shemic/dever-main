use std::fmt::Write;

use super::rust_string;
use crate::wire::{Node, Schema};

/// Emit concrete helpers into the checked program's generated Rust module.
/// The index is local to that module; callers cache schemas by fingerprint.
pub fn emit(schema: &Schema, index: usize) -> String {
    emit_named(schema, &format!("wire_{index}"))
}

pub(super) fn emit_named(schema: &Schema, prefix: &str) -> String {
    let mut output = String::new();
    let root = schema.root;
    let ty = rust_type(schema, root);
    writeln!(output, "fn {prefix}_decode(text: &str) -> Result<{ty}, String> {{ let node = dever_runtime::wire::parse(text)?; {prefix}_d{root}(&node) }}").expect("string formatting");
    if !schema.policy.permits_secret() {
        writeln!(output, "fn {prefix}_encode(value: &{ty}) -> Result<dever_runtime::wire::Encoded, String> {{ let mut writer = dever_runtime::wire::Encoder::default(); {prefix}_e{root}(value, &mut writer)?; writer.finish() }}").expect("string formatting");
    }
    for (id, node) in schema.nodes.iter().enumerate() {
        let ty = rust_type(schema, id);
        writeln!(output, "fn {prefix}_d{id}(node: &dever_runtime::wire::Node<'_>) -> Result<{ty}, String> {{ {} }}", decode(schema, node, prefix)).expect("string formatting");
        if !schema.policy.permits_secret() {
            writeln!(output, "fn {prefix}_e{id}(value: &{ty}, writer: &mut dever_runtime::wire::Encoder) -> Result<(), String> {{ {} }}", encode(node, prefix)).expect("string formatting");
        }
    }
    output
}

fn rust_type(schema: &Schema, id: usize) -> String {
    match &schema.nodes[id] {
        Node::Bytes => "dever_runtime::bytes::Bytes".into(),
        Node::Bool => "bool".into(),
        Node::Int | Node::DateTime | Node::Date | Node::Time | Node::Duration => "i64".into(),
        Node::Float => "f64".into(),
        Node::Decimal => "dever_runtime::number::DecimalValue".into(),
        Node::Text | Node::Json => "String".into(),
        Node::Id => "dever_runtime::Id".into(),
        Node::Uuid => "dever_runtime::orm::Uuid".into(),
        Node::Secret => "dever_runtime::secret::Secret".into(),
        Node::Nullable(inner) => format!("Option<{}>", rust_type(schema, *inner)),
        Node::List(inner) => format!(
            "dever_runtime::collections::List<{}>",
            rust_type(schema, *inner)
        ),
        Node::Record { id, .. } | Node::ModelId { id, .. } => format!("T{id}"),
    }
}

fn decode(schema: &Schema, node: &Node, prefix: &str) -> String {
    match node {
        Node::Bytes => "node.bytes()".into(),
        Node::Bool => "node.boolean()".into(),
        Node::Int | Node::Duration => "node.int()".into(),
        Node::ModelId { id, .. } => format!("Ok(T{id}(node.int()?))"),
        Node::Float => "node.float()".into(),
        Node::Text => "Ok(node.text()?.to_owned())".into(),
        Node::Json => "Ok(node.raw().to_owned())".into(),
        Node::Secret => {
            "Ok(dever_runtime::secret::Secret::from_input(node.text()?.as_bytes().to_vec()))".into()
        }
        Node::Id => "Ok(dever_runtime::Id(node.text()?.to_owned()))".into(),
        Node::Uuid => {
            "dever_runtime::orm::Uuid::parse(node.text()?).map_err(|error| error.to_string())"
                .into()
        }
        Node::Decimal => {
            "dever_runtime::number::DecimalValue::parse(node.text()?).map_err(str::to_owned)".into()
        }
        Node::DateTime => "dever_runtime::time::parse_datetime(node.text()?)".into(),
        Node::Date => "dever_runtime::time::parse_date(node.text()?)".into(),
        Node::Time => "dever_runtime::time::parse_time(node.text()?)".into(),
        Node::Nullable(inner) => format!(
            "if node.is_null() {{ Ok(None) }} else {{ Ok(Some({prefix}_d{inner}(node)?)) }}"
        ),
        Node::List(inner) => format!(
            "Ok(dever_runtime::collections::List::new(node.list()?.iter().map({prefix}_d{inner}).collect::<Result<Vec<_>, String>>()?))"
        ),
        Node::Record { id, fields, .. } => {
            let names = fields
                .iter()
                .map(|(name, _)| rust_string(name))
                .collect::<Vec<_>>()
                .join(", ");
            let mut body = format!("let fields = node.fields(&[{names}])?; Ok(T{id} {{");
            for (index, (name, child)) in fields.iter().enumerate() {
                let name = rust_string(name);
                if matches!(schema.nodes[*child], Node::Nullable(_)) {
                    write!(body, "f{index}: match fields.get({name}) {{ Some(value) => {prefix}_d{child}(value)?, None => None }},").expect("string formatting");
                } else {
                    write!(body, "f{index}: {prefix}_d{child}(fields.get({name}).ok_or(\"missing required wire field\")?)?,").expect("string formatting");
                }
            }
            body.push_str("})");
            body
        }
    }
}

fn encode(node: &Node, prefix: &str) -> String {
    match node {
        Node::Bytes => "writer.bytes(value)".into(),
        Node::Bool => "writer.boolean(*value)".into(),
        Node::Int | Node::Duration => "writer.int(*value)".into(),
        Node::ModelId { .. } => "writer.int(value.0)".into(),
        Node::Float => "writer.float(*value)".into(),
        Node::Text => "writer.text(value)".into(),
        Node::Json => "writer.json(value)".into(),
        Node::Id => "writer.text(&value.0)".into(),
        Node::Uuid | Node::Decimal => "writer.text(&value.to_string())".into(),
        Node::DateTime => "writer.text(&dever_runtime::time::format_datetime(*value)?)".into(),
        Node::Date => "writer.text(&dever_runtime::time::format_date(*value)?)".into(),
        Node::Time => "writer.text(&dever_runtime::time::format_time(*value)?)".into(),
        Node::Nullable(inner) => format!(
            "match value {{ Some(value) => {prefix}_e{inner}(value, writer), None => writer.null() }}"
        ),
        Node::List(inner) => format!(
            "writer.begin_array()?; for value in value.values() {{ {prefix}_e{inner}(value, writer)?; }} writer.end()"
        ),
        Node::Record { fields, .. } => {
            let mut body = "writer.begin_object()?;".to_owned();
            for (index, (name, child)) in fields.iter().enumerate() {
                write!(
                    body,
                    "writer.key({})?; {prefix}_e{child}(&value.f{index}, writer)?;",
                    rust_string(name)
                )
                .expect("string formatting");
            }
            body.push_str("writer.end()");
            body
        }
        Node::Secret => unreachable!("input-only schema never emits an encoder"),
    }
}
