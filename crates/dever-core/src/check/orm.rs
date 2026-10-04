use std::collections::BTreeSet;

use crate::diagnostic::Diagnostic;
use crate::hir::{
    Expression as Value, ExpressionKind as Expr, ModelCondition, ModelOperation, ModelOrder,
    QueryPlan, TextCondition,
};
use crate::model::{ModelFieldType, ModelSchema, ModelSqlCardinality, ModelSqlParameter};
use crate::source::{SourceLayout, SourceRole, Span};
use crate::syntax::{BinaryOperator, Expression, ExpressionKind as Ast, MapEntry};
use crate::types::{Shape, Type};

use super::{Checked, body::Body, path_name};

const MAX_CREATE_MANY_ROWS: usize = 256;
const MAX_QUERY_VALUES: usize = 100;
const MAX_SQLITE_PARAMETERS: usize = 999;

#[derive(Clone, Copy, Eq, PartialEq)]
enum QueryKind {
    Plain,
    Included,
    Cursor,
    Stream,
}

impl Body<'_, '_> {
    pub(super) fn model_call(
        &mut self,
        name: &str,
        arguments: &[Expression],
        span: Span,
    ) -> Checked<Option<Value>> {
        let Some((model_name, operation_name)) = name.rsplit_once('.') else {
            return Ok(None);
        };
        let built_in = matches!(
            operation_name,
            "create"
                | "create_many"
                | "get"
                | "first"
                | "list"
                | "cursor"
                | "count"
                | "exists"
                | "stream"
                | "update"
                | "delete"
                | "upsert"
        );
        let Some((model_index, model)) = self.resolve_model(model_name, span)? else {
            return Ok(None);
        };
        let model = model.clone();
        let sql_operation = model
            .sql
            .iter()
            .position(|operation| operation.name == operation_name);
        if !built_in && sql_operation.is_none() {
            return Ok(None);
        }
        let operation = match operation_name {
            "create" => {
                arity(name, arguments, 1, span)?;
                ModelOperation::Create {
                    values: self.write_values(&model, &arguments[0], true)?,
                }
            }
            "create_many" => {
                arity(name, arguments, 1, span)?;
                ModelOperation::CreateMany {
                    rows: self.create_many_values(&model, &arguments[0])?,
                }
            }
            "get" => {
                arity(name, arguments, 1, span)?;
                let id = self.expression(&arguments[0], Some(&self.id_type(&model)))?;
                ModelOperation::Get { id: Box::new(id) }
            }
            "first" | "list" | "cursor" | "count" | "exists" | "stream" => {
                if arguments.len() > 1 {
                    return Err(Diagnostic::error(
                        "C005",
                        format!("'{name}' expects zero or one query block"),
                        span,
                    ));
                }
                let query = match arguments.first() {
                    Some(query) => self.query(
                        &model,
                        query,
                        match operation_name {
                            "first" | "list" => QueryKind::Included,
                            "cursor" => QueryKind::Cursor,
                            "stream" => QueryKind::Stream,
                            _ => QueryKind::Plain,
                        },
                    )?,
                    None => default_query(span),
                };
                match operation_name {
                    "first" => ModelOperation::First { query },
                    "list" => ModelOperation::List { query },
                    "cursor" => ModelOperation::Cursor { query },
                    "count" => ModelOperation::Count { query },
                    "exists" => ModelOperation::Exists { query },
                    "stream" => ModelOperation::Stream { query },
                    _ => unreachable!(),
                }
            }
            "update" => {
                arity(name, arguments, 2, span)?;
                let query = self.target_query(&model, &arguments[0])?;
                let values = self.write_values(&model, &arguments[1], false)?;
                if values.is_empty() {
                    return Err(Diagnostic::error(
                        "C005",
                        "update requires at least one changed field",
                        arguments[1].span,
                    ));
                }
                ModelOperation::Update { query, values }
            }
            "delete" => {
                arity(name, arguments, 1, span)?;
                ModelOperation::Delete {
                    query: self.target_query(&model, &arguments[0])?,
                }
            }
            "upsert" => {
                arity(name, arguments, 3, span)?;
                let key = self.write_values(&model, &arguments[0], false)?;
                self.validate_upsert_key(&model, &key, arguments[0].span)?;
                let create = self.write_values(&model, &arguments[1], false)?;
                validate_distinct_fields(&key, &create, arguments[1].span)?;
                self.validate_required_create(
                    &model,
                    key.iter().chain(&create),
                    arguments[1].span,
                )?;
                let update = self.write_values(&model, &arguments[2], false)?;
                if update.is_empty() {
                    return Err(Diagnostic::error(
                        "C005",
                        "upsert requires at least one changed field",
                        arguments[2].span,
                    ));
                }
                ModelOperation::Upsert {
                    key,
                    create,
                    update,
                }
            }
            _ => {
                let operation = sql_operation.expect("resolved Model SQL operation");
                let declaration = &model.sql[operation];
                arity(name, arguments, declaration.parameters.len(), span)?;
                let arguments = arguments
                    .iter()
                    .zip(&declaration.parameters)
                    .map(|(argument, parameter)| {
                        let expected = self.sql_parameter_type(parameter);
                        self.expression(argument, Some(&expected))
                    })
                    .collect::<Checked<Vec<_>>>()?;
                ModelOperation::Sql {
                    operation,
                    arguments,
                    read_only: super::sql_read::proven(&model, declaration),
                }
            }
        };
        let ty = match &operation {
            ModelOperation::Create { .. }
            | ModelOperation::Get { .. }
            | ModelOperation::Upsert { .. } => Type::Named(model.record),
            ModelOperation::First { .. } => Type::Named(model.record).nullable(),
            ModelOperation::List { .. } => Type::Named(model.page),
            ModelOperation::Cursor { .. } => Type::Named(model.cursor),
            ModelOperation::Stream { .. } => Type::RowStream(Box::new(Type::Named(model.record))),
            ModelOperation::CreateMany { .. }
            | ModelOperation::Count { .. }
            | ModelOperation::Update { .. }
            | ModelOperation::Delete { .. } => Type::Int,
            ModelOperation::Exists { .. } => Type::Bool,
            ModelOperation::Sql { operation, .. } => {
                let result = model.sql[*operation].result;
                let row = Type::Named(result.record);
                match result.cardinality {
                    ModelSqlCardinality::One => row,
                    ModelSqlCardinality::Optional => row.nullable(),
                    ModelSqlCardinality::Many => Type::List(Box::new(row)),
                }
            }
        };
        Ok(Some(Value {
            kind: Expr::ModelOperation {
                model: model_index,
                operation: Box::new(operation),
            },
            ty,
            span,
        }))
    }

    fn resolve_model(&self, name: &str, span: Span) -> Checked<Option<(usize, &ModelSchema)>> {
        let caller = &self.context.symbols.packages[self.context.owner];
        let candidates = match &caller.layout {
            SourceLayout::Role {
                component, domain, ..
            }
            | SourceLayout::Test {
                component, domain, ..
            } => {
                let local = if name == "model" {
                    Some(format!("{component}.{domain}.model"))
                } else {
                    name.strip_prefix("model.")
                        .map(|rest| format!("{component}.{domain}.model.{rest}"))
                };
                local
                    .into_iter()
                    .chain(std::iter::once(name.to_owned()))
                    .collect::<Vec<_>>()
            }
            SourceLayout::Main | SourceLayout::Loose => vec![name.to_owned()],
        };
        let matches = self
            .context
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| {
                candidates.contains(&model.package)
                    || matches!(&caller.layout, SourceLayout::Loose) && model.table == name
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(None),
            [(index, model)] => {
                let target = &self.context.symbols.packages[self.context.types[model.record].owner];
                if !matches!(&caller.layout, SourceLayout::Loose)
                    && (caller.layout.role() != Some(SourceRole::App)
                        || caller.layout.domain() != target.layout.domain())
                {
                    return Err(Diagnostic::error(
                        "C006",
                        "Model operations are private to their owning domain App",
                        span,
                    ));
                }
                Ok(Some((*index, *model)))
            }
            _ => Err(Diagnostic::error(
                "C004",
                format!("Model name '{name}' is ambiguous; use its full package name"),
                span,
            )),
        }
    }

    fn write_values(
        &mut self,
        model: &ModelSchema,
        source: &Expression,
        creating: bool,
    ) -> Checked<Vec<(usize, Value)>> {
        let entries = map_entries(source, if creating { "create" } else { "update" })?;
        let fields = self.model_fields(model).to_vec();
        let mut seen = BTreeSet::new();
        let mut values = Vec::new();
        for entry in entries {
            let name = map_key(entry)?;
            let Some((index, field)) = fields
                .iter()
                .enumerate()
                .find(|(_, field)| field.name == name)
            else {
                return Err(Diagnostic::error(
                    "C004",
                    format!("unknown Model field '{name}'"),
                    entry.key.span,
                ));
            };
            let field_type = field.ty.clone();
            if model.fields[index].generated {
                return Err(Diagnostic::error(
                    "C005",
                    format!("generated Model field '{name}' cannot be written"),
                    entry.key.span,
                ));
            }
            if !seen.insert(index) {
                return Err(Diagnostic::error(
                    "C002",
                    format!("duplicate Model field '{name}'"),
                    entry.span,
                ));
            }
            values.push((index, self.expression(&entry.value, Some(&field_type))?));
        }
        if creating {
            self.validate_required_create(model, values.iter(), source.span)?;
        }
        Ok(values)
    }

    fn create_many_values(
        &mut self,
        model: &ModelSchema,
        source: &Expression,
    ) -> Checked<Vec<Vec<(usize, Value)>>> {
        let Ast::List(items) = &source.kind else {
            return Err(Diagnostic::error(
                "C005",
                "create_many requires a static List of anonymous create blocks",
                source.span,
            ));
        };
        if items.is_empty() || items.len() > MAX_CREATE_MANY_ROWS {
            return Err(Diagnostic::error(
                "C005",
                format!("create_many requires between 1 and {MAX_CREATE_MANY_ROWS} rows"),
                source.span,
            ));
        }
        let mut rows = Vec::with_capacity(items.len());
        for item in items {
            rows.push(self.write_values(model, item, true)?);
        }
        let fields = rows[0].iter().map(|(field, _)| *field).collect::<Vec<_>>();
        if rows
            .iter()
            .skip(1)
            .any(|row| row.iter().map(|(field, _)| *field).collect::<Vec<_>>() != fields)
        {
            return Err(Diagnostic::error(
                "C005",
                "create_many rows must write the same fields in the same order",
                source.span,
            ));
        }
        let generated = model
            .fields
            .iter()
            .filter(|field| {
                field.generated && matches!(field.ty, crate::model::ModelFieldType::Uuid)
            })
            .count();
        if items.len().saturating_mul(fields.len() + generated) > MAX_SQLITE_PARAMETERS {
            return Err(Diagnostic::error(
                "C005",
                format!("create_many exceeds the {MAX_SQLITE_PARAMETERS}-parameter batch limit"),
                source.span,
            ));
        }
        Ok(rows)
    }

    fn validate_required_create<'a>(
        &self,
        model: &ModelSchema,
        values: impl Iterator<Item = &'a (usize, Value)>,
        span: Span,
    ) -> Checked<()> {
        let present = values.map(|(field, _)| *field).collect::<BTreeSet<_>>();
        for (index, field) in model.fields.iter().enumerate() {
            if !field.generated
                && !field.nullable
                && field.default.is_none()
                && !present.contains(&index)
            {
                return Err(Diagnostic::error(
                    "C005",
                    format!("create is missing required Model field '{}'", field.name),
                    span,
                ));
            }
        }
        Ok(())
    }

    fn validate_upsert_key(
        &self,
        model: &ModelSchema,
        key: &[(usize, Value)],
        span: Span,
    ) -> Checked<()> {
        if key.is_empty() {
            return Err(Diagnostic::error(
                "C005",
                "upsert key cannot be empty",
                span,
            ));
        }
        let names = key
            .iter()
            .map(|(field, _)| model.fields[*field].name.as_str())
            .collect::<BTreeSet<_>>();
        if !model.indexes.iter().any(|index| {
            index.unique
                && index.fields.len() == names.len()
                && index
                    .fields
                    .iter()
                    .all(|field| names.contains(field.as_str()))
        }) {
            return Err(Diagnostic::error(
                "C005",
                "upsert key must exactly match a unique Model index",
                span,
            ));
        }
        Ok(())
    }

    fn target_query(&mut self, model: &ModelSchema, source: &Expression) -> Checked<QueryPlan> {
        if matches!(source.kind, Ast::Map(_)) {
            self.query(model, source, QueryKind::Plain)
        } else {
            let id = self.expression(source, Some(&self.id_type(model)))?;
            Ok(QueryPlan {
                condition: Some(ModelCondition::Compare {
                    field: 0,
                    operator: BinaryOperator::Equal,
                    value: id,
                }),
                ..default_query(source.span)
            })
        }
    }

    fn query(
        &mut self,
        model: &ModelSchema,
        source: &Expression,
        kind: QueryKind,
    ) -> Checked<QueryPlan> {
        let entries = map_entries(source, "query")?;
        let mut query = default_query(source.span);
        let mut seen = BTreeSet::new();
        for entry in entries {
            let name = map_key(entry)?;
            if !seen.insert(name.clone()) {
                return Err(Diagnostic::error(
                    "C002",
                    format!("duplicate query option '{name}'"),
                    entry.span,
                ));
            }
            match name.as_str() {
                "where" => query.condition = Some(self.condition(model, &entry.value)?),
                "order" => query.order = self.order(model, &entry.value)?,
                "with" if matches!(kind, QueryKind::Included | QueryKind::Cursor) => {
                    query.relations = self.relations(model, &entry.value)?
                }
                "after" if kind == QueryKind::Cursor => {
                    query.after = Some(
                        self.expression(&entry.value, Some(&Type::Named(model.record).nullable()))?,
                    )
                }
                "page" => query.page = self.expression(&entry.value, Some(&Type::Int))?,
                "size" => query.size = self.expression(&entry.value, Some(&Type::Int))?,
                _ => {
                    return Err(Diagnostic::error(
                        "C004",
                        format!("unknown query option '{name}'"),
                        entry.key.span,
                    ));
                }
            }
        }
        if !query.order.iter().any(|order| order.field == 0) {
            query.order.push(ModelOrder {
                field: 0,
                descending: true,
            });
        }
        if kind == QueryKind::Cursor
            && query
                .order
                .iter()
                .any(|order| model.fields[order.field].nullable)
        {
            return Err(Diagnostic::error(
                "C005",
                "cursor ordering does not support nullable Model fields",
                source.span,
            ));
        }
        if kind == QueryKind::Stream && seen.contains("page") {
            return Err(Diagnostic::error(
                "C005",
                "stream traverses the full query and does not accept page",
                source.span,
            ));
        }
        Ok(query)
    }

    fn relations(&self, model: &ModelSchema, source: &Expression) -> Checked<Vec<usize>> {
        let Ast::List(items) = &source.kind else {
            return Err(Diagnostic::error(
                "C005",
                "with requires a static List of relation names",
                source.span,
            ));
        };
        let mut seen = BTreeSet::new();
        let mut relations = Vec::new();
        for item in items {
            let Ast::Name(path) = &item.kind else {
                return Err(Diagnostic::error(
                    "C005",
                    "with entries must be bare relation names",
                    item.span,
                ));
            };
            if path.len() != 1 {
                return Err(Diagnostic::error(
                    "C005",
                    "nested relation loading is not supported",
                    item.span,
                ));
            }
            let Some(index) = model
                .relations
                .iter()
                .position(|relation| relation.name == path[0].text)
            else {
                return Err(Diagnostic::error(
                    "C004",
                    format!("unknown Model relation '{}'", path[0].text),
                    item.span,
                ));
            };
            if !seen.insert(index) {
                return Err(Diagnostic::error(
                    "C002",
                    format!("with cannot repeat relation '{}'", path[0].text),
                    item.span,
                ));
            }
            if model.relations[index].logical {
                return Err(Diagnostic::error(
                    "C014",
                    format!(
                        "logical cross-scope relation '{}' cannot be loaded with 'with'",
                        path[0].text
                    ),
                    item.span,
                ));
            }
            relations.push(index);
        }
        Ok(relations)
    }

    fn condition(&mut self, model: &ModelSchema, source: &Expression) -> Checked<ModelCondition> {
        if let Ast::Group(value) = &source.kind {
            return self.condition(model, value);
        }
        if let Ast::Unary {
            operator: crate::syntax::UnaryOperator::Not,
            value,
        } = &source.kind
        {
            return Ok(ModelCondition::Not(Box::new(self.condition(model, value)?)));
        }
        if let Ast::Call {
            function,
            arguments,
        } = &source.kind
        {
            let Ast::Name(path) = &function.kind else {
                return Err(invalid_condition(source.span));
            };
            if path.len() == 1 {
                return self.condition_call(model, &path[0].text, arguments, source.span);
            }
            return Err(invalid_condition(source.span));
        }
        let Ast::Binary {
            left,
            operator,
            right,
        } = &source.kind
        else {
            return Err(invalid_condition(source.span));
        };
        if matches!(operator, BinaryOperator::And | BinaryOperator::Or) {
            let left = Box::new(self.condition(model, left)?);
            let right = Box::new(self.condition(model, right)?);
            return Ok(if *operator == BinaryOperator::And {
                ModelCondition::And(left, right)
            } else {
                ModelCondition::Or(left, right)
            });
        }
        if !matches!(
            operator,
            BinaryOperator::Equal
                | BinaryOperator::NotEqual
                | BinaryOperator::Less
                | BinaryOperator::LessEqual
                | BinaryOperator::Greater
                | BinaryOperator::GreaterEqual
        ) {
            return Err(Diagnostic::error(
                "C005",
                "unsupported Model comparison",
                source.span,
            ));
        }
        let field = field_name(left).ok_or_else(|| {
            Diagnostic::error(
                "C005",
                "the left side of a Model comparison must be a field",
                left.span,
            )
        })?;
        let fields = self.model_fields(model).to_vec();
        let Some((index, definition)) = fields
            .iter()
            .enumerate()
            .find(|(_, definition)| definition.name == field)
        else {
            return Err(Diagnostic::error(
                "C004",
                format!("unknown Model field '{field}'"),
                left.span,
            ));
        };
        if matches!(right.kind, Ast::Literal(crate::syntax::Literal::Null))
            && !matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual)
        {
            return Err(Diagnostic::error(
                "C005",
                "null only supports equality comparisons",
                right.span,
            ));
        }
        let expected = definition.ty.clone();
        let value = self.expression(right, Some(&expected))?;
        Ok(ModelCondition::Compare {
            field: index,
            operator: *operator,
            value,
        })
    }

    fn condition_call(
        &mut self,
        model: &ModelSchema,
        name: &str,
        arguments: &[Expression],
        span: Span,
    ) -> Checked<ModelCondition> {
        let expected = match name {
            "in" | "contains" | "starts_with" | "ends_with" => 2,
            "between" => 3,
            _ => return Err(invalid_condition(span)),
        };
        if arguments.len() != expected {
            return Err(Diagnostic::error(
                "C005",
                format!("where '{name}' expects {expected} arguments"),
                span,
            ));
        }
        let field_name = field_name(&arguments[0]).ok_or_else(|| {
            Diagnostic::error(
                "C005",
                format!("the first '{name}' argument must be a Model field"),
                arguments[0].span,
            )
        })?;
        let fields = self.model_fields(model).to_vec();
        let Some((field, definition)) = fields
            .iter()
            .enumerate()
            .find(|(_, definition)| definition.name == field_name)
        else {
            return Err(Diagnostic::error(
                "C004",
                format!("unknown Model field '{field_name}'"),
                arguments[0].span,
            ));
        };
        let field_type = definition.ty.clone();
        match name {
            "in" => {
                let Ast::List(items) = &arguments[1].kind else {
                    return Err(Diagnostic::error(
                        "C005",
                        "where 'in' requires a static List as its second argument",
                        arguments[1].span,
                    ));
                };
                if items.is_empty() || items.len() > MAX_QUERY_VALUES {
                    return Err(Diagnostic::error(
                        "C005",
                        format!("where 'in' requires between 1 and {MAX_QUERY_VALUES} values"),
                        arguments[1].span,
                    ));
                }
                let values = items
                    .iter()
                    .map(|item| self.expression(item, Some(&field_type)))
                    .collect::<Checked<Vec<_>>>()?;
                Ok(ModelCondition::In { field, values })
            }
            "between" => {
                let lower = self.expression(&arguments[1], Some(&field_type))?;
                let upper = self.expression(&arguments[2], Some(&field_type))?;
                if !field_type.base().numeric()
                    && !matches!(
                        field_type.base(),
                        Type::DateTime | Type::Date | Type::Time | Type::Duration
                    )
                {
                    return Err(Diagnostic::error(
                        "C005",
                        "where 'between' requires an ordered numeric or time field",
                        arguments[0].span,
                    ));
                }
                Ok(ModelCondition::Between {
                    field,
                    lower,
                    upper,
                })
            }
            _ => {
                if field_type.base() != &Type::Text {
                    return Err(Diagnostic::error(
                        "C005",
                        format!("where '{name}' requires a Text field"),
                        arguments[0].span,
                    ));
                }
                let value = self.expression(&arguments[1], Some(&Type::Text))?;
                let operation = match name {
                    "contains" => TextCondition::Contains,
                    "starts_with" => TextCondition::StartsWith,
                    "ends_with" => TextCondition::EndsWith,
                    _ => unreachable!(),
                };
                Ok(ModelCondition::Text {
                    field,
                    operation,
                    value,
                })
            }
        }
    }

    fn order(&self, model: &ModelSchema, source: &Expression) -> Checked<Vec<ModelOrder>> {
        let items: &[Expression] = match &source.kind {
            Ast::List(items) => items,
            _ => std::slice::from_ref(source),
        };
        let fields = self.model_fields(model);
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for item in items {
            let Ast::Name(path) = &item.kind else {
                return Err(Diagnostic::error(
                    "C005",
                    "order entries use 'field.asc' or 'field.desc'",
                    item.span,
                ));
            };
            if path.len() != 2 || !matches!(path[1].text.as_str(), "asc" | "desc") {
                return Err(Diagnostic::error(
                    "C005",
                    "order entries use 'field.asc' or 'field.desc'",
                    item.span,
                ));
            }
            let Some(index) = fields.iter().position(|field| field.name == path[0].text) else {
                return Err(Diagnostic::error(
                    "C004",
                    format!("unknown Model field '{}'", path[0].text),
                    path[0].span,
                ));
            };
            if !seen.insert(index) {
                return Err(Diagnostic::error(
                    "C002",
                    "order cannot repeat a Model field",
                    item.span,
                ));
            }
            result.push(ModelOrder {
                field: index,
                descending: path[1].text == "desc",
            });
        }
        Ok(result)
    }

    fn model_fields(&self, model: &ModelSchema) -> &[crate::types::Field] {
        let Shape::Record(fields) = &self.context.types[model.record].shape else {
            unreachable!("Model record type")
        };
        fields
    }

    fn id_type(&self, model: &ModelSchema) -> Type {
        self.model_fields(model)[0].ty.clone()
    }

    fn sql_parameter_type(&self, parameter: &ModelSqlParameter) -> Type {
        let ty = match &parameter.ty {
            ModelFieldType::Bool => Type::Bool,
            ModelFieldType::Int => Type::Int,
            ModelFieldType::Float => Type::Float,
            ModelFieldType::Decimal { .. } => Type::Decimal,
            ModelFieldType::Text { .. } => Type::Text,
            ModelFieldType::Bytes { .. } => Type::Bytes,
            ModelFieldType::Uuid => Type::Uuid,
            ModelFieldType::DateTime => Type::DateTime,
            ModelFieldType::Date => Type::Date,
            ModelFieldType::Time => Type::Time,
            ModelFieldType::Duration => Type::Duration,
            ModelFieldType::Json => Type::Json,
            ModelFieldType::Choice(name) => Type::Named(
                self.context
                    .types
                    .iter()
                    .position(|definition| definition.name == *name)
                    .expect("validated SQL choice type"),
            ),
            ModelFieldType::ModelId(model) => {
                let id = format!("{model}.id");
                Type::Named(
                    self.context
                        .types
                        .iter()
                        .position(|definition| definition.name == id)
                        .expect("validated SQL Model ID type"),
                )
            }
        };
        if parameter.nullable {
            ty.nullable()
        } else {
            ty
        }
    }
}

fn arity(name: &str, arguments: &[Expression], expected: usize, span: Span) -> Checked<()> {
    if arguments.len() == expected {
        Ok(())
    } else {
        Err(Diagnostic::error(
            "C005",
            format!("'{name}' expects {expected} argument(s)"),
            span,
        ))
    }
}

fn map_entries<'a>(source: &'a Expression, operation: &str) -> Checked<&'a [MapEntry]> {
    match &source.kind {
        Ast::Map(entries) => Ok(entries),
        _ => Err(Diagnostic::error(
            "C005",
            format!("{operation} requires an anonymous block"),
            source.span,
        )),
    }
}

fn map_key(entry: &MapEntry) -> Checked<String> {
    match &entry.key.kind {
        Ast::Name(path) if path.len() == 1 => Ok(path_name(path)),
        _ => Err(Diagnostic::error(
            "C005",
            "anonymous block keys must be bare names",
            entry.key.span,
        )),
    }
}

fn validate_distinct_fields(
    left: &[(usize, Value)],
    right: &[(usize, Value)],
    span: Span,
) -> Checked<()> {
    let left = left
        .iter()
        .map(|(field, _)| *field)
        .collect::<BTreeSet<_>>();
    if right.iter().any(|(field, _)| left.contains(field)) {
        return Err(Diagnostic::error(
            "C002",
            "upsert key and create blocks cannot repeat a Model field",
            span,
        ));
    }
    Ok(())
}

fn invalid_condition(span: Span) -> Diagnostic {
    Diagnostic::error(
        "C005",
        "where requires Model comparisons, not/and/or, in, between or Text matching",
        span,
    )
}

fn field_name(source: &Expression) -> Option<&str> {
    match &source.kind {
        Ast::Name(path) if path.len() == 1 => Some(&path[0].text),
        _ => None,
    }
}

fn default_query(span: Span) -> QueryPlan {
    QueryPlan {
        condition: None,
        order: vec![ModelOrder {
            field: 0,
            descending: true,
        }],
        relations: Vec::new(),
        after: None,
        page: int(1, span),
        size: int(20, span),
    }
}

fn int(value: i64, span: Span) -> Value {
    Value {
        kind: Expr::Constant(crate::hir::Constant::Int(value)),
        ty: Type::Int,
        span,
    }
}
