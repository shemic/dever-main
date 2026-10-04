use crate::diagnostic::Diagnostic;
use crate::syntax::{
    self, Declaration, InputKind, Name, Package, Pattern, StatementKind, TypeShape,
};

use super::Checked;

pub(super) fn check(package: &Package) -> Checked<()> {
    for name in &package.name {
        snake(name)?;
    }
    for declaration in &package.declarations {
        match declaration {
            Declaration::Api(binding) => {
                snake(&binding.action)?;
                for segment in &binding.target {
                    snake(segment)?;
                }
            }
            Declaration::Rest(rest) => {
                for segment in rest.model.iter().flatten() {
                    snake(segment)?;
                }
            }
            Declaration::External(external) => {
                for capability in &external.capabilities {
                    snake(capability)?;
                }
            }
            Declaration::Type(ty) => {
                camel(&ty.name)?;
                if matches!(
                    ty.name.text.as_str(),
                    "Bool"
                        | "Int"
                        | "Decimal"
                        | "Float"
                        | "Text"
                        | "Id"
                        | "Bytes"
                        | "Secret"
                        | "Uuid"
                        | "DateTime"
                        | "Date"
                        | "Time"
                        | "Duration"
                        | "Json"
                        | "List"
                        | "Map"
                        | "MapEntry"
                        | "Stream"
                        | "AsyncStream"
                        | "Channel"
                        | "Task"
                        | "Group"
                ) {
                    return Err(Diagnostic::error(
                        "C002",
                        "type name is reserved by the language",
                        ty.name.span,
                    ));
                }
                match &ty.shape {
                    TypeShape::Record(fields) => check_fields(fields)?,
                    TypeShape::Choice(variants) => {
                        for variant in variants {
                            camel(&variant.name)?;
                            check_fields(&variant.payload)?;
                        }
                    }
                }
            }
            Declaration::Function(function) => {
                for segment in function.name.text.split('.') {
                    snake(&Name {
                        text: segment.into(),
                        span: function.name.span,
                    })?;
                }
                for input in &function.inputs {
                    snake(&input.name)?;
                    match &input.kind {
                        InputKind::Value(Pattern::Variant { bindings, .. }) => {
                            for binding in bindings {
                                if binding.text != "_" {
                                    snake(binding)?;
                                }
                            }
                        }
                        InputKind::Handler(signature) => {
                            check_fields(&signature.inputs)?;
                            check_fields(&signature.outputs)?;
                        }
                        _ => {}
                    }
                }
                check_fields(&function.outputs)?;
                for statement in &function.body {
                    if let StatementKind::Assign { target, .. } = &statement.kind {
                        for name in target {
                            snake(name)?;
                        }
                    }
                }
            }
            Declaration::Database(binding) => snake(&binding.name)?,
            Declaration::Schedule(schedule) => snake(&schedule.target)?,
            Declaration::ModelIndex(index) => {
                for field in &index.fields {
                    snake(field)?;
                }
            }
            Declaration::Relation(relation) => {
                snake(&relation.name)?;
                for segment in &relation.field {
                    snake(segment)?;
                }
            }
            Declaration::Seed(seed) => {
                for row in &seed.rows {
                    for field in row {
                        snake(&field.name)?;
                    }
                }
            }
            Declaration::Migration(migration) => {
                snake(&migration.name)?;
                for operation in &migration.operations {
                    match operation {
                        syntax::MigrationOperation::Drop(field) => snake(field)?,
                        syntax::MigrationOperation::Sql(_) => {}
                    }
                }
            }
            Declaration::ModelSql(sql) => {
                snake(&sql.name)?;
                check_fields(&sql.inputs)?;
                check_fields(&sql.outputs)?;
            }
        }
    }
    Ok(())
}

fn check_fields(fields: &[syntax::Field]) -> Checked<()> {
    for field in fields {
        snake(&field.name)?;
    }
    Ok(())
}

fn snake(name: &Name) -> Checked<()> {
    if name
        .text
        .starts_with(|character: char| character.is_ascii_lowercase())
        && name.text.split('_').all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
    {
        return Ok(());
    }
    Err(Diagnostic::error(
        "C010",
        "name must use ASCII lower_snake_case",
        name.span,
    ))
}

fn camel(name: &Name) -> Checked<()> {
    if name
        .text
        .starts_with(|character: char| character.is_ascii_uppercase())
        && name
            .text
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Ok(());
    }
    Err(Diagnostic::error(
        "C010",
        "type and variant names must use ASCII UpperCamelCase",
        name.span,
    ))
}
