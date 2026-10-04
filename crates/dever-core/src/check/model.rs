use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Diagnostic;
use crate::model::{
    self, ConnectionSelector, ModelChoice, ModelField, ModelFieldType, ModelIndex, ModelMigration,
    ModelMigrationSql, ModelRelation, ModelRelationKind, ModelSchema, ModelScope, ModelSql,
    ModelSqlCardinality, ModelSqlParameter, ModelSqlResult, ModelValue, RestOwnerSource, SeedRow,
};
use crate::source::{SourceLayout, SourceMap, SourceRole};
use crate::syntax::{
    Declaration, Expression, ExpressionKind, Field, MigrationOperation, Package, TypeDeclaration,
    TypeRef, TypeShape, UnaryOperator,
};
use crate::types::{Definition, DefinitionKind, Field as CheckedField, Shape, Type};

use super::{Symbols, path_name};

#[derive(Clone, Copy, Debug)]
pub(super) struct Registration {
    owner: usize,
    record: usize,
    id: usize,
    page: usize,
    cursor: usize,
}

#[derive(Clone)]
pub(super) struct RestFieldSource {
    pub field: usize,
    pub private: bool,
    pub span: crate::source::Span,
    pub create: Option<Expression>,
    pub replace: Option<Expression>,
    pub search: Option<Expression>,
}

pub(super) fn rest_sources(
    schemas: &[ModelSchema],
    packages: &[Package],
    types: &[Definition],
) -> Vec<Vec<RestFieldSource>> {
    schemas
        .iter()
        .map(|schema| {
            let package = &packages[types[schema.record].owner];
            let record = package
                .declarations
                .iter()
                .find_map(|declaration| match declaration {
                    Declaration::Type(record) if record.name.text == schema.name => Some(record),
                    _ => None,
                })
                .expect("Model source record");
            let TypeShape::Record(fields) = &record.shape else {
                unreachable!()
            };
            fields
                .iter()
                .enumerate()
                .filter_map(|(index, field)| {
                    let storage = &field.storage;
                    (storage.create.is_some()
                        || storage.replace.is_some()
                        || storage.search.is_some())
                    .then(|| RestFieldSource {
                        field: index + 2,
                        private: field.private,
                        span: field.span,
                        create: storage.create.clone(),
                        replace: storage.replace.clone(),
                        search: storage.search.clone(),
                    })
                })
                .collect()
        })
        .collect()
}

pub(super) fn register<'a>(
    packages: &'a [Package],
    sources: &SourceMap,
    symbols: &mut Symbols,
    types: &mut Vec<Definition>,
    type_sources: &mut Vec<Option<&'a TypeDeclaration>>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Registration> {
    let mut registrations = Vec::new();
    for (owner, package) in packages.iter().enumerate() {
        let source = sources.get(package.span.source);
        if !source.is_model() {
            validate_regular_package(package, errors);
            continue;
        }
        let Some(stem) = source.path().file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let stem = if source.is_markdown() {
            stem.strip_suffix(".dever").expect("Markdown source suffix")
        } else {
            stem
        };
        let role_model = matches!(package.layout.role(), Some(SourceRole::Model));
        let model_name = match &package.layout {
            SourceLayout::Role { domain, topics, .. } if topics.is_empty() => domain.as_str(),
            _ => stem,
        };
        let expected_name = model_type_name(model_name);
        let records = package
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Type(declaration)
                    if matches!(declaration.shape, TypeShape::Record(_)) =>
                {
                    Some(declaration)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let Some(record_source) = records
            .iter()
            .copied()
            .find(|record| record.name.text == expected_name)
        else {
            if let [record] = records.as_slice() {
                errors.push(Diagnostic::error(
                    "C014",
                    format!(
                        "Model record must be named '{expected_name}' to match '{}.dever'",
                        model_name
                    ),
                    record.name.span,
                ));
            } else {
                errors.push(Diagnostic::error(
                    "C014",
                    format!("a Model file must declare record '{expected_name}'"),
                    package.span,
                ));
            }
            continue;
        };
        if record_source.global && !role_model {
            errors.push(Diagnostic::error(
                "C014",
                "'global type' requires a Model source role",
                record_source.span,
            ));
        }
        if !role_model && !record_source.public {
            errors.push(Diagnostic::error(
                "C014",
                "the Model record must be declared public",
                record_source.name.span,
            ));
        }
        validate_model_declarations(package, record_source, errors);
        let package_name = path_name(&package.name);
        let qualified_record = format!("{package_name}.{expected_name}");
        let Some(record) = symbols.types.get(&qualified_record).copied() else {
            continue;
        };
        types[record].kind = DefinitionKind::Model;
        if role_model {
            types[record].public = true;
        }
        for declaration in &package.declarations {
            let Declaration::Type(choice) = declaration else {
                continue;
            };
            if !matches!(choice.shape, TypeShape::Choice(_)) {
                continue;
            }
            let qualified = format!("{package_name}.{}", choice.name.text);
            if let Some(id) = symbols.types.get(&qualified) {
                types[*id].kind = DefinitionKind::ModelChoice;
                if role_model {
                    types[*id].public = true;
                }
            }
        }
        let qualified_id = format!("{package_name}.id");
        if symbols.types.contains_key(&qualified_id) {
            errors.push(Diagnostic::error(
                "C002",
                "Model ID type conflicts with an existing type",
                record_source.span,
            ));
            continue;
        }
        let id = types.len();
        symbols.types.insert(qualified_id.clone(), id);
        types.push(Definition {
            name: qualified_id,
            owner,
            public: true,
            shape: Shape::Record(Vec::new()),
            span: record_source.span,
            properties: Default::default(),
            kind: DefinitionKind::ModelId,
        });
        type_sources.push(None);
        let Some(page) = register_result_type(
            &package_name,
            "Page",
            DefinitionKind::ModelPage,
            owner,
            record_source,
            symbols,
            types,
            type_sources,
            errors,
        ) else {
            continue;
        };
        let Some(cursor) = register_result_type(
            &package_name,
            "Cursor",
            DefinitionKind::ModelCursor,
            owner,
            record_source,
            symbols,
            types,
            type_sources,
            errors,
        ) else {
            continue;
        };
        registrations.push(Registration {
            owner,
            record,
            id,
            page,
            cursor,
        });
    }
    registrations
}

#[allow(clippy::too_many_arguments)]
fn register_result_type<'a>(
    package: &str,
    name: &str,
    kind: DefinitionKind,
    owner: usize,
    source: &'a TypeDeclaration,
    symbols: &mut Symbols,
    types: &mut Vec<Definition>,
    type_sources: &mut Vec<Option<&'a TypeDeclaration>>,
    errors: &mut Vec<Diagnostic>,
) -> Option<usize> {
    let qualified = format!("{package}.{name}");
    if symbols.types.contains_key(&qualified) {
        errors.push(Diagnostic::error(
            "C002",
            format!("Model {name} type conflicts with an existing type"),
            source.span,
        ));
        return None;
    }
    let id = types.len();
    symbols.types.insert(qualified.clone(), id);
    types.push(Definition {
        name: qualified,
        owner,
        public: true,
        shape: Shape::Record(Vec::new()),
        span: source.span,
        properties: Default::default(),
        kind,
    });
    type_sources.push(None);
    Some(id)
}

pub(super) fn synthesize_fields(
    registrations: &[Registration],
    types: &mut [Definition],
    errors: &mut Vec<Diagnostic>,
) {
    for registration in registrations {
        let Shape::Record(fields) = &mut types[registration.record].shape else {
            unreachable!("registered Model is a record")
        };
        if fields
            .iter()
            .any(|field| matches!(field.name.as_str(), "id" | "created_at"))
        {
            errors.push(Diagnostic::error(
                "C014",
                "'id' and 'created_at' are implicit Model fields",
                types[registration.record].span,
            ));
            continue;
        }
        fields.insert(
            0,
            CheckedField {
                name: "created_at".into(),
                ty: Type::DateTime,
                private: false,
                bounds: Vec::new(),
            },
        );
        fields.insert(
            0,
            CheckedField {
                name: "id".into(),
                ty: Type::Named(registration.id),
                private: false,
                bounds: Vec::new(),
            },
        );
        let page_fields = vec![
            CheckedField {
                name: "items".into(),
                ty: Type::List(Box::new(Type::Named(registration.record))),
                private: false,
                bounds: Vec::new(),
            },
            CheckedField {
                name: "page".into(),
                ty: Type::Int,
                private: false,
                bounds: Vec::new(),
            },
            CheckedField {
                name: "size".into(),
                ty: Type::Int,
                private: false,
                bounds: Vec::new(),
            },
            CheckedField {
                name: "total".into(),
                ty: Type::Int,
                private: false,
                bounds: Vec::new(),
            },
            CheckedField {
                name: "pages".into(),
                ty: Type::Int,
                private: false,
                bounds: Vec::new(),
            },
        ];
        types[registration.page].shape = Shape::Record(page_fields);
        types[registration.cursor].shape = Shape::Record(vec![
            CheckedField {
                name: "items".into(),
                ty: Type::List(Box::new(Type::Named(registration.record))),
                private: false,
                bounds: Vec::new(),
            },
            CheckedField {
                name: "next".into(),
                ty: Type::Named(registration.record).nullable(),
                private: false,
                bounds: Vec::new(),
            },
            CheckedField {
                name: "has_more".into(),
                ty: Type::Bool,
                private: false,
                bounds: Vec::new(),
            },
        ]);
    }
}

pub(super) fn synthesize_relation_fields(
    schemas: &[ModelSchema],
    types: &mut [Definition],
    errors: &mut Vec<Diagnostic>,
) {
    let records = schemas
        .iter()
        .map(|schema| (schema.package.as_str(), schema.record))
        .collect::<BTreeMap<_, _>>();
    for schema in schemas {
        let Shape::Record(fields) = &mut types[schema.record].shape else {
            unreachable!("Model is a record")
        };
        for relation in &schema.relations {
            if fields.iter().any(|field| field.name == relation.name) {
                errors.push(Diagnostic::error(
                    "C002",
                    format!("relation '{}' conflicts with a Model field", relation.name),
                    types[schema.record].span,
                ));
                continue;
            }
            let Some(target) = records.get(relation.model.as_str()).copied() else {
                continue;
            };
            let payload = match relation.kind {
                ModelRelationKind::ToOne => {
                    let nullable = schema
                        .fields
                        .iter()
                        .find(|field| field.name == relation.field)
                        .is_some_and(|field| field.nullable);
                    let target = Type::Named(target);
                    if nullable { target.nullable() } else { target }
                }
                ModelRelationKind::ToMany => Type::List(Box::new(Type::Named(target))),
            };
            fields.push(CheckedField {
                name: relation.name.clone(),
                ty: Type::Related(Box::new(payload)),
                private: false,
                bounds: Vec::new(),
            });
        }
    }
}

pub(super) fn schemas(
    registrations: &[Registration],
    packages: &[Package],
    symbols: &Symbols,
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) -> Vec<ModelSchema> {
    let mut schemas = Vec::new();
    for registration in registrations {
        let package = &packages[registration.owner];
        if let Some(schema) = schema(package, *registration, symbols, types, errors) {
            schemas.push(schema);
        }
    }
    validate_model_references(&mut schemas, packages, registrations, errors);
    validate_relations(&mut schemas, packages, registrations, errors);
    for schema in &mut schemas {
        schema.revision = model::checksum(&schema_identity(schema));
    }
    validate_table_names(&schemas, packages, registrations, errors);
    schemas
}

fn validate_regular_package(package: &Package, errors: &mut Vec<Diagnostic>) {
    for declaration in &package.declarations {
        match declaration {
            Declaration::Type(declaration) => {
                if declaration.global {
                    errors.push(Diagnostic::error(
                        "C014",
                        "'global type' is allowed only for the primary record in a Model source role",
                        declaration.span,
                    ));
                }
                match &declaration.shape {
                    TypeShape::Record(fields) => validate_regular_fields(fields, errors),
                    TypeShape::Choice(variants) => {
                        for variant in variants {
                            if variant.label.is_some() {
                                errors.push(Diagnostic::error(
                                    "C014",
                                    "choice display labels are only allowed in Model files",
                                    variant.span,
                                ));
                            }
                            validate_regular_fields(&variant.payload, errors);
                        }
                    }
                }
            }
            Declaration::Function(function) => {
                for input in &function.inputs {
                    match &input.kind {
                        crate::syntax::InputKind::Value(crate::syntax::Pattern::Typed {
                            ty,
                            ..
                        }) if !ty.parameters.is_empty() => errors.push(Diagnostic::error(
                            "C014",
                            "bounded storage types are only allowed on Model fields",
                            ty.span,
                        )),
                        crate::syntax::InputKind::Handler(handler) => {
                            validate_regular_fields(&handler.inputs, errors);
                            validate_regular_fields(&handler.outputs, errors);
                        }
                        _ => {}
                    }
                }
                validate_regular_fields(&function.outputs, errors);
            }
            Declaration::Database(_) | Declaration::Schedule(_)
                if package.layout.role() == Some(crate::source::SourceRole::Job) => {}
            Declaration::Api(_) | Declaration::Rest(_) | Declaration::External(_) => {}
            Declaration::Schedule(_)
            | Declaration::Database(_)
            | Declaration::ModelIndex(_)
            | Declaration::Relation(_)
            | Declaration::Seed(_)
            | Declaration::Migration(_)
            | Declaration::ModelSql(_) => errors.push(Diagnostic::error(
                "C014",
                "Model metadata is only allowed in a model source role",
                declaration_span(declaration),
            )),
        }
    }
}

fn validate_regular_fields(fields: &[Field], errors: &mut Vec<Diagnostic>) {
    for field in fields {
        if !field.ty.parameters.is_empty() || field.storage != Default::default() {
            errors.push(Diagnostic::error(
                "C014",
                "storage parameters and modifiers are only allowed on Model fields",
                field.span,
            ));
        }
    }
}

fn validate_model_declarations(
    package: &Package,
    record: &TypeDeclaration,
    errors: &mut Vec<Diagnostic>,
) {
    let mut database = None;
    let mut seed = None;
    let mut migrations = BTreeSet::new();
    let mut sql = BTreeSet::new();
    for declaration in &package.declarations {
        match declaration {
            Declaration::Database(binding) => {
                if database.replace(binding.span).is_some() {
                    errors.push(Diagnostic::error(
                        "C002",
                        "a Model may declare only one database binding",
                        binding.span,
                    ));
                }
            }
            Declaration::Seed(value) => {
                if seed.replace(value.span).is_some() {
                    errors.push(Diagnostic::error(
                        "C002",
                        "a Model may declare only one seed block",
                        value.span,
                    ));
                }
            }
            Declaration::Migration(migration) => {
                if !migrations.insert(&migration.name.text) {
                    errors.push(Diagnostic::error(
                        "C002",
                        "duplicate Model migration name",
                        migration.name.span,
                    ));
                }
                if migration.operations.is_empty() {
                    errors.push(Diagnostic::error(
                        "C014",
                        "a Model migration must contain at least one operation",
                        migration.span,
                    ));
                }
            }
            Declaration::ModelSql(operation) => {
                if !sql.insert(&operation.name.text) {
                    errors.push(Diagnostic::error(
                        "C002",
                        "duplicate Model SQL operation name",
                        operation.name.span,
                    ));
                }
                if matches!(
                    operation.name.text.as_str(),
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
                ) || package.declarations.iter().any(|declaration| {
                    matches!(declaration, Declaration::Function(function) if function.name.text == operation.name.text)
                }) {
                    errors.push(Diagnostic::error(
                        "C002",
                        "Model SQL operation name conflicts with an existing operation",
                        operation.name.span,
                    ));
                }
            }
            Declaration::Type(declaration) if !std::ptr::eq(declaration, record) => {
                if declaration.global {
                    errors.push(Diagnostic::error(
                        "C014",
                        "only the primary Model record may be declared global",
                        declaration.span,
                    ));
                }
                if let TypeShape::Record(fields) = &declaration.shape {
                    if declaration.public {
                        errors.push(Diagnostic::error(
                            "C014",
                            "a Model SQL result record must be private",
                            declaration.span,
                        ));
                    }
                    validate_regular_fields(fields, errors);
                }
            }
            Declaration::Api(_) | Declaration::Rest(_) => errors.push(Diagnostic::error(
                "C014",
                "API declarations are only allowed in an API source role",
                declaration_span(declaration),
            )),
            _ => {}
        }
    }
    let TypeShape::Record(fields) = &record.shape else {
        unreachable!()
    };
    for field in fields {
        if !field.bounds.is_empty() {
            errors.push(Diagnostic::error(
                "C014",
                "Model constraints use storage type parameters, not function pattern bounds",
                field.span,
            ));
        }
    }
}

fn schema(
    package: &Package,
    registration: Registration,
    symbols: &Symbols,
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) -> Option<ModelSchema> {
    let package_name = path_name(&package.name);
    let record_source = package
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Type(declaration)
                if declaration.name.text
                    == types[registration.record].name.rsplit('.').next().unwrap() =>
            {
                Some(declaration)
            }
            _ => None,
        })?;
    let TypeShape::Record(source_fields) = &record_source.shape else {
        unreachable!()
    };
    let choices = normalize_choices(package, errors);
    let mut fields = vec![
        ModelField {
            name: "id".into(),
            ty: ModelFieldType::ModelId(package_name.clone()),
            nullable: false,
            generated: true,
            default: None,
            rename_from: None,
            logical_reference: false,
            owner: None,
        },
        ModelField {
            name: "created_at".into(),
            ty: ModelFieldType::DateTime,
            nullable: false,
            generated: true,
            default: None,
            rename_from: None,
            logical_reference: false,
            owner: None,
        },
    ];
    let mut source_by_name = BTreeMap::new();
    let mut rename_sources = BTreeSet::new();
    for field in source_fields {
        source_by_name.insert(field.name.text.clone(), field);
        let Some(ty) = normalize_field_type(&field.ty, &package_name, symbols, types, errors)
        else {
            continue;
        };
        let nullable = field.ty.nullable;
        let storage = &field.storage;
        if storage.generated && (!matches!(ty, ModelFieldType::Uuid) || nullable) {
            errors.push(Diagnostic::error(
                "C014",
                "'generated' requires a non-null Uuid field",
                field.span,
            ));
        }
        if storage.generated && storage.default.is_some() {
            errors.push(Diagnostic::error(
                "C014",
                "a generated field cannot also declare a default",
                field.span,
            ));
        }
        if storage.index && storage.unique {
            errors.push(Diagnostic::error(
                "C014",
                "'unique' already creates an index",
                field.span,
            ));
        }
        if let Some(from) = &storage.from
            && (from.text == field.name.text || !rename_sources.insert(&from.text))
        {
            errors.push(Diagnostic::error(
                "C014",
                "a rename source must be distinct and used by one field",
                from.span,
            ));
        }
        let default = storage
            .default
            .as_ref()
            .and_then(|value| normalize_value(value, &ty, nullable, &choices, errors));
        let owner = storage
            .owner
            .as_ref()
            .and_then(|source| normalize_owner_source(source, &ty, nullable, storage, errors));
        fields.push(ModelField {
            name: field.name.text.clone(),
            ty,
            nullable,
            generated: storage.generated,
            default,
            rename_from: storage.from.as_ref().map(|name| name.text.clone()),
            logical_reference: false,
            owner,
        });
    }

    if fields.iter().filter(|field| field.owner.is_some()).count() > 1 {
        errors.push(Diagnostic::error(
            "C014",
            "a Model may declare only one REST owner field",
            record_source.span,
        ));
    }

    let indexes = normalize_indexes(package, &fields, &source_by_name, errors);
    let seed = normalize_seed(package, &fields, &indexes, &choices, errors);
    let migrations = normalize_migrations(package, &fields, errors);
    let mut relations = fields
        .iter()
        .filter_map(|field| {
            let ModelFieldType::ModelId(target) = &field.ty else {
                return None;
            };
            let name = field.name.strip_suffix("_id")?;
            if name.is_empty() {
                return None;
            }
            Some(ModelRelation {
                name: name.into(),
                model: target.clone(),
                field: field.name.clone(),
                kind: ModelRelationKind::ToOne,
                logical: false,
            })
        })
        .collect::<Vec<_>>();
    relations.extend(
        package
            .declarations
            .iter()
            .filter_map(|declaration| match declaration {
                Declaration::Relation(relation) => {
                    let mut path = relation
                        .field
                        .iter()
                        .map(|name| name.text.clone())
                        .collect::<Vec<_>>();
                    let field = path.pop()?;
                    Some(ModelRelation {
                        name: relation.name.text.clone(),
                        model: path.join("."),
                        field,
                        kind: ModelRelationKind::ToMany,
                        logical: false,
                    })
                }
                _ => None,
            }),
    );
    let connection = package
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Database(binding) => {
                Some(ConnectionSelector::Explicit(binding.name.text.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| ConnectionSelector::PackageRoot(package.name[0].text.clone()));
    let sql = normalize_sql(package, registration, symbols, types, errors);
    let table = match &package.layout {
        SourceLayout::Role {
            domain,
            role: SourceRole::Model,
            topics,
            ..
        } => topics.last().unwrap_or(domain).clone(),
        _ => package.name.last()?.text.clone(),
    };
    let mut schema = ModelSchema {
        record: registration.record,
        page: registration.page,
        cursor: registration.cursor,
        package: package_name,
        name: record_source.name.text.clone(),
        scope: if record_source.global {
            ModelScope::Global
        } else {
            ModelScope::Tenant
        },
        table,
        connection,
        fields,
        choices,
        indexes,
        relations,
        sql,
        seed,
        migrations,
        revision: String::new(),
        seed_revision: String::new(),
    };
    schema.seed_revision = model::checksum(&seed_identity(&schema.seed));
    let _ = registration;
    Some(schema)
}

fn normalize_owner_source(
    source: &Expression,
    ty: &ModelFieldType,
    nullable: bool,
    storage: &crate::syntax::FieldStorage,
    errors: &mut Vec<Diagnostic>,
) -> Option<RestOwnerSource> {
    let mut valid = true;
    if nullable {
        errors.push(Diagnostic::error(
            "C014",
            "a REST owner field must be non-null",
            source.span,
        ));
        valid = false;
    }
    if !matches!(ty, ModelFieldType::Int | ModelFieldType::ModelId(_)) {
        errors.push(Diagnostic::error(
            "C014",
            "a REST owner field must use Int or a Model id type",
            source.span,
        ));
        valid = false;
    }
    if storage.generated || storage.default.is_some() {
        errors.push(Diagnostic::error(
            "C014",
            "a REST owner field cannot also be generated or declare a default",
            source.span,
        ));
        valid = false;
    }

    let source = super::expressions::ungroup(source);
    let owner = match &source.kind {
        ExpressionKind::Call {
            function,
            arguments,
        } if arguments.is_empty() => {
            let function = super::expressions::ungroup(function);
            let ExpressionKind::Name(path) = &function.kind else {
                return invalid_owner_source(source, errors);
            };
            match path_name(path).as_str() {
                "dever.auth.user_id" => Some(RestOwnerSource::User),
                "dever.auth.tenant_id" => Some(RestOwnerSource::Tenant),
                _ => None,
            }
        }
        _ => None,
    };
    let Some(owner) = owner else {
        return invalid_owner_source(source, errors);
    };
    valid.then_some(owner)
}

fn invalid_owner_source(
    source: &Expression,
    errors: &mut Vec<Diagnostic>,
) -> Option<RestOwnerSource> {
    errors.push(Diagnostic::error(
        "C014",
        "a REST owner source must be dever.auth.user_id() or dever.auth.tenant_id()",
        source.span,
    ));
    None
}

fn normalize_choices(package: &Package, errors: &mut Vec<Diagnostic>) -> Vec<ModelChoice> {
    package
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Type(declaration) => match &declaration.shape {
                TypeShape::Choice(variants) => {
                    let labelled = variants
                        .iter()
                        .filter(|variant| variant.label.is_some())
                        .count();
                    if labelled != 0 && labelled != variants.len() {
                        errors.push(Diagnostic::error(
                            "C014",
                            "choice variants must either all have display labels or all omit them",
                            declaration.span,
                        ));
                    }
                    if variants
                        .iter()
                        .any(|variant| variant.error || !variant.payload.is_empty())
                    {
                        errors.push(Diagnostic::error(
                            "C014",
                            "Model choices cannot contain error or payload variants",
                            declaration.span,
                        ));
                    }
                    Some(ModelChoice {
                        name: format!("{}.{}", path_name(&package.name), declaration.name.text),
                        options: variants
                            .iter()
                            .map(|variant| (variant.name.text.clone(), variant.label.clone()))
                            .collect(),
                    })
                }
                TypeShape::Record(_) => None,
            },
            _ => None,
        })
        .collect()
}

fn normalize_sql(
    package: &Package,
    registration: Registration,
    symbols: &Symbols,
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) -> Vec<ModelSql> {
    let package_name = path_name(&package.name);
    let mut used_result_records = BTreeSet::new();
    let operations = package
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::ModelSql(source) = declaration else {
                return None;
            };
            let mut names = BTreeSet::new();
            let mut parameters = Vec::new();
            for input in &source.inputs {
                if input.private || !input.bounds.is_empty() || input.storage != Default::default()
                {
                    errors.push(Diagnostic::error(
                        "C014",
                        "SQL parameters cannot use field modifiers or bounds",
                        input.span,
                    ));
                    continue;
                }
                if !names.insert(&input.name.text) {
                    errors.push(Diagnostic::error(
                        "C002",
                        "duplicate SQL parameter name",
                        input.name.span,
                    ));
                    continue;
                }
                let Some(ty) =
                    normalize_field_type(&input.ty, &package_name, symbols, types, errors)
                else {
                    continue;
                };
                parameters.push(ModelSqlParameter {
                    name: input.name.text.clone(),
                    ty,
                    nullable: input.ty.nullable,
                });
            }
            if parameters.len() != source.inputs.len() {
                return None;
            }
            if source.outputs.len() != 1 {
                errors.push(Diagnostic::error(
                    "C014",
                    "a SQL operation must declare exactly one result",
                    source.span,
                ));
                return None;
            }
            let output = &source.outputs[0];
            if output.private || !output.bounds.is_empty() || output.storage != Default::default() {
                errors.push(Diagnostic::error(
                    "C014",
                    "a SQL result cannot use field modifiers or bounds",
                    output.span,
                ));
                return None;
            }
            let result = normalize_sql_result(
                &output.ty,
                registration,
                &package_name,
                symbols,
                types,
                errors,
            )?;
            if types[result.record].kind == DefinitionKind::Regular {
                used_result_records.insert(result.record);
            }
            validate_sql_placeholders(
                &source.sqlite.value,
                '?',
                '$',
                parameters.len(),
                source.sqlite.span,
                errors,
            );
            validate_sql_placeholders(
                &source.postgres.value,
                '$',
                '?',
                parameters.len(),
                source.postgres.span,
                errors,
            );
            if source.sqlite.value.trim().is_empty() || source.postgres.value.trim().is_empty() {
                errors.push(Diagnostic::error(
                    "C014",
                    "SQL dialect text cannot be empty",
                    source.span,
                ));
            }
            Some(ModelSql {
                name: source.name.text.clone(),
                parameters,
                result,
                sqlite: source.sqlite.value.clone(),
                postgres: source.postgres.value.clone(),
            })
        })
        .collect::<Vec<_>>();

    for declaration in &package.declarations {
        let Declaration::Type(declaration) = declaration else {
            continue;
        };
        if declaration.name.text == types[registration.record].name.rsplit('.').next().unwrap()
            || !matches!(declaration.shape, TypeShape::Record(_))
        {
            continue;
        }
        let qualified = format!("{package_name}.{}", declaration.name.text);
        let Some(record) = symbols.types.get(&qualified) else {
            continue;
        };
        if !used_result_records.contains(record) {
            errors.push(Diagnostic::error(
                "C014",
                "an auxiliary record in a Model file must be used by a SQL result",
                declaration.span,
            ));
        }
    }
    operations
}

fn normalize_sql_result(
    source: &TypeRef,
    registration: Registration,
    package: &str,
    symbols: &Symbols,
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) -> Option<ModelSqlResult> {
    let (row, cardinality) = if path_name(&source.name) == "List" {
        if source.arguments.len() != 1
            || source.nullable
            || !source.parameters.is_empty()
            || source.arguments[0].nullable
        {
            errors.push(Diagnostic::error(
                "C014",
                "SQL results use Record, Record? or List<Record>",
                source.span,
            ));
            return None;
        }
        (&source.arguments[0], ModelSqlCardinality::Many)
    } else {
        (
            source,
            if source.nullable {
                ModelSqlCardinality::Optional
            } else {
                ModelSqlCardinality::One
            },
        )
    };
    if !row.arguments.is_empty() || !row.parameters.is_empty() {
        errors.push(Diagnostic::error(
            "C014",
            "SQL results use a concrete Model or record type",
            row.span,
        ));
        return None;
    }
    let name = path_name(&row.name);
    let context = super::Context {
        symbols,
        types,
        functions: &[],
        models: &[],
        owner: registration.owner,
    };
    let Some(record) = context
        .candidates(&name)
        .iter()
        .find_map(|qualified| symbols.types.get(qualified).copied())
    else {
        errors.push(Diagnostic::error(
            "C004",
            format!("unknown SQL result type '{name}' in '{package}'"),
            row.span,
        ));
        return None;
    };
    let Shape::Record(fields) = &types[record].shape else {
        errors.push(Diagnostic::error(
            "C014",
            "SQL results must use a concrete Model or record type",
            row.span,
        ));
        return None;
    };
    let result_owner = &symbols.packages[types[record].owner].layout;
    let model_owner = &symbols.packages[registration.owner].layout;
    let app_view = types[record].public
        && result_owner.role() == Some(SourceRole::App)
        && result_owner.domain() == model_owner.domain();
    match types[record].kind {
        DefinitionKind::Model => {}
        DefinitionKind::Regular
            if (types[record].owner == registration.owner && !types[record].public) || app_view =>
        {
            for field in fields {
                if (app_view && field.private)
                    || !field.bounds.is_empty()
                    || !sql_result_field(&field.ty, types)
                {
                    errors.push(Diagnostic::error(
                        "C014",
                        format!(
                            "SQL result field '{}' must be visible, without bounds, and have a supported storage type, found {}",
                            field.name,
                            field.ty.label(types)
                        ),
                        types[record].span,
                    ));
                }
            }
        }
        _ => {
            errors.push(Diagnostic::error(
                "C014",
                "a custom SQL result record must be private to its Model file or an owning-domain App View",
                row.span,
            ));
            return None;
        }
    }
    Some(ModelSqlResult {
        record,
        cardinality,
    })
}

fn sql_result_field(ty: &Type, types: &[Definition]) -> bool {
    match ty {
        Type::Bool
        | Type::Int
        | Type::Decimal
        | Type::Float
        | Type::Text
        | Type::Bytes
        | Type::Uuid
        | Type::DateTime
        | Type::Date
        | Type::Time
        | Type::Duration
        | Type::Json => true,
        Type::Nullable(inner) => sql_result_field(inner, types),
        Type::Named(id) => {
            types[*id].kind == DefinitionKind::ModelId
                || matches!(&types[*id].shape, Shape::Choice(variants) if variants.iter().all(|variant| !variant.error && variant.fields.is_empty()))
        }
        _ => false,
    }
}

fn validate_sql_placeholders(
    sql: &str,
    marker: char,
    other_marker: char,
    parameters: usize,
    span: crate::source::Span,
    errors: &mut Vec<Diagnostic>,
) {
    validate_sql(sql, marker, other_marker, parameters, span, false, errors);
}

fn validate_sql(
    sql: &str,
    marker: char,
    other_marker: char,
    parameters: usize,
    span: crate::source::Span,
    migration: bool,
    errors: &mut Vec<Diagnostic>,
) {
    let bytes = sql.as_bytes();
    let marker = marker as u8;
    let other_marker = other_marker as u8;
    let mut used = BTreeSet::new();
    let mut index = 0;
    let mut started = false;
    let mut terminated = false;
    while index < bytes.len() {
        let current = bytes[index];
        if current.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if current == b'-' && bytes.get(index + 1) == Some(&b'-') {
            index = skip_sql_line_comment(bytes, index + 2);
            continue;
        }
        if current == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index = skip_sql_block_comment(bytes, index + 2, marker == b'$');
            continue;
        }
        if migration {
            if terminated {
                errors.push(Diagnostic::error(
                    "C014",
                    "migration SQL must contain exactly one DML statement",
                    span,
                ));
                return;
            }
            if !started {
                let end = bytes[index..]
                    .iter()
                    .position(|byte| !byte.is_ascii_alphabetic())
                    .map_or(bytes.len(), |end| index + end);
                if !matches!(
                    sql[index..end].to_ascii_uppercase().as_str(),
                    "INSERT" | "UPDATE" | "DELETE"
                ) {
                    errors.push(Diagnostic::error(
                        "C014",
                        "migration SQL must start with INSERT, UPDATE or DELETE",
                        span,
                    ));
                    return;
                }
                started = true;
            }
            if current == b';' {
                terminated = true;
                index += 1;
                continue;
            }
            if current.is_ascii_alphabetic() || current == b'_' || current >= 128 {
                let end = bytes[index..]
                    .iter()
                    .position(|byte| {
                        !byte.is_ascii_alphanumeric()
                            && *byte != b'_'
                            && *byte < 128
                            && !(marker == b'$' && *byte == b'$')
                    })
                    .map_or(bytes.len(), |end| index + end);
                if reserved_migration_identifier(&sql[index..end]) {
                    errors.push(Diagnostic::error(
                        "C014",
                        "migration SQL cannot reference reserved database objects",
                        span,
                    ));
                    return;
                }
                if marker == b'$'
                    && sql[index..end].eq_ignore_ascii_case("u")
                    && bytes[end..].starts_with(b"&\"")
                {
                    errors.push(Diagnostic::error(
                        "C014",
                        "migration SQL does not accept Unicode-escaped identifiers",
                        span,
                    ));
                    return;
                }
                index = end;
                continue;
            }
        }
        if matches!(current, b'\'' | b'"' | b'`') {
            let backslash_escape = marker == b'$'
                && current == b'\''
                && index > 0
                && matches!(bytes[index - 1], b'e' | b'E')
                && (index == 1
                    || !matches!(
                        bytes[index - 2],
                        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'
                    ));
            let Some(end) = skip_sql_quoted(bytes, index, current, backslash_escape) else {
                errors.push(Diagnostic::error(
                    "C014",
                    "SQL has an unclosed quoted value or identifier",
                    span,
                ));
                return;
            };
            // SQLite also permits single-quoted table names. Reserved names belong
            // in bind values when they are data, never in migration SQL text.
            if migration && reserved_migration_identifier(&sql[index + 1..end - 1]) {
                errors.push(Diagnostic::error(
                    "C014",
                    "migration SQL cannot reference reserved database objects",
                    span,
                ));
                return;
            }
            index = end;
            continue;
        }
        if current == b'[' && marker == b'?' {
            let Some(end) = skip_sql_bracket_identifier(bytes, index) else {
                errors.push(Diagnostic::error(
                    "C014",
                    "SQL has an unclosed quoted identifier",
                    span,
                ));
                return;
            };
            if migration && reserved_migration_identifier(&sql[index + 1..end - 1]) {
                errors.push(Diagnostic::error(
                    "C014",
                    "migration SQL cannot reference reserved database objects",
                    span,
                ));
                return;
            }
            index = end;
            continue;
        }
        if marker == b'$'
            && current == b'$'
            && (index == 0 || !postgres_identifier_byte(bytes[index - 1]))
        {
            if let Some(delimiter) = postgres_dollar_quote(bytes, index) {
                let Some(end) = skip_postgres_dollar_quote(bytes, index, delimiter) else {
                    errors.push(Diagnostic::error(
                        "C014",
                        "SQL has an unclosed dollar-quoted value",
                        span,
                    ));
                    return;
                };
                index = end;
                continue;
            }
            if bytes
                .get(index + 1)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_' || *byte >= 128)
            {
                errors.push(Diagnostic::error(
                    "C014",
                    "SQL has an unclosed dollar-quote delimiter",
                    span,
                ));
                return;
            }
        }
        if current == other_marker && bytes.get(index + 1).is_some_and(u8::is_ascii_digit) {
            errors.push(Diagnostic::error(
                "C014",
                "SQL uses the other dialect's parameter marker",
                span,
            ));
            return;
        }
        if current != marker || !bytes.get(index + 1).is_some_and(u8::is_ascii_digit) {
            index += 1;
            continue;
        }
        let start = index + 1;
        index = start;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        let parameter = sql[start..index].parse::<usize>().unwrap_or(usize::MAX);
        if parameter == 0 || parameter > parameters {
            errors.push(Diagnostic::error(
                "C014",
                format!("SQL parameter {parameter} is outside the declared parameter list"),
                span,
            ));
            return;
        }
        used.insert(parameter);
    }
    if migration && !started {
        errors.push(Diagnostic::error(
            "C014",
            "migration SQL cannot be empty",
            span,
        ));
    }
    if let Some(parameter) = (1..=parameters).find(|parameter| !used.contains(parameter)) {
        errors.push(Diagnostic::error(
            "C014",
            format!("SQL parameter {parameter} is declared but not bound in this dialect"),
            span,
        ));
    }
}

fn reserved_migration_identifier(identifier: &str) -> bool {
    let identifier = identifier.to_ascii_lowercase();
    identifier.starts_with("_dever_")
        || identifier.starts_with("sqlite_")
        || identifier.starts_with("pg_")
        || identifier == "information_schema"
}

fn skip_sql_quoted(
    bytes: &[u8],
    mut index: usize,
    quote: u8,
    backslash_escape: bool,
) -> Option<usize> {
    index += 1;
    while index < bytes.len() {
        if backslash_escape && bytes[index] == b'\\' {
            index = (index + 2).min(bytes.len());
        } else if bytes[index] == quote {
            if bytes.get(index + 1) == Some(&quote) {
                index += 2;
            } else {
                return Some(index + 1);
            }
        } else {
            index += 1;
        }
    }
    None
}

fn skip_sql_bracket_identifier(bytes: &[u8], index: usize) -> Option<usize> {
    bytes[index + 1..]
        .iter()
        .position(|byte| *byte == b']')
        .map(|end| index + end + 2)
}

fn skip_sql_line_comment(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
        index += 1;
    }
    index
}

fn skip_sql_block_comment(bytes: &[u8], mut index: usize, nested: bool) -> usize {
    let mut depth = 1usize;
    while index < bytes.len() && depth > 0 {
        if nested && bytes[index..].starts_with(b"/*") {
            depth += 1;
            index += 2;
        } else if bytes[index..].starts_with(b"*/") {
            depth -= 1;
            index += 2;
        } else {
            index += 1;
        }
    }
    index
}

fn postgres_dollar_quote(bytes: &[u8], index: usize) -> Option<&[u8]> {
    let mut end = index + 1;
    if bytes.get(end) == Some(&b'$') {
        return Some(&bytes[index..=end]);
    }
    if !bytes
        .get(end)
        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_' || *byte >= 128)
    {
        return None;
    }
    end += 1;
    while bytes
        .get(end)
        .is_some_and(|byte| postgres_identifier_byte(*byte) && *byte != b'$')
    {
        end += 1;
    }
    (bytes.get(end) == Some(&b'$')).then(|| &bytes[index..=end])
}

fn postgres_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || byte >= 128
}

fn skip_postgres_dollar_quote(bytes: &[u8], index: usize, delimiter: &[u8]) -> Option<usize> {
    let content = index + delimiter.len();
    bytes[content..]
        .windows(delimiter.len())
        .position(|candidate| candidate == delimiter)
        .map(|end| content + end + delimiter.len())
}

fn normalize_field_type(
    reference: &TypeRef,
    package: &str,
    symbols: &Symbols,
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) -> Option<ModelFieldType> {
    if !reference.arguments.is_empty() {
        errors.push(Diagnostic::error(
            "C014",
            "List, Map and stream types cannot be stored as Model fields; use Json explicitly",
            reference.span,
        ));
        return None;
    }
    let name = path_name(&reference.name);
    let parameters = reference
        .parameters
        .iter()
        .map(|parameter| parameter.value.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>();
    let Some(parameters) = parameters else {
        errors.push(Diagnostic::error(
            "C014",
            "storage type parameters must be unsigned integers",
            reference.span,
        ));
        return None;
    };
    let simple = match (name.as_str(), parameters.as_slice()) {
        ("Bool", []) => Some(ModelFieldType::Bool),
        ("Int", []) => Some(ModelFieldType::Int),
        ("Float", []) => Some(ModelFieldType::Float),
        ("Uuid", []) => Some(ModelFieldType::Uuid),
        ("DateTime", []) => Some(ModelFieldType::DateTime),
        ("Date", []) => Some(ModelFieldType::Date),
        ("Time", []) => Some(ModelFieldType::Time),
        ("Duration", []) => Some(ModelFieldType::Duration),
        ("Json", []) => Some(ModelFieldType::Json),
        ("Text", []) => Some(ModelFieldType::Text {
            minimum: 0,
            maximum: None,
        }),
        ("Text", [maximum]) if *maximum > 0 => Some(ModelFieldType::Text {
            minimum: 0,
            maximum: Some(*maximum),
        }),
        ("Text", [minimum, maximum]) if minimum <= maximum && *maximum > 0 => {
            Some(ModelFieldType::Text {
                minimum: *minimum,
                maximum: Some(*maximum),
            })
        }
        ("Bytes", []) => Some(ModelFieldType::Bytes {
            minimum: 0,
            maximum: None,
        }),
        ("Bytes", [maximum]) if *maximum > 0 => Some(ModelFieldType::Bytes {
            minimum: 0,
            maximum: Some(*maximum),
        }),
        ("Bytes", [minimum, maximum]) if minimum <= maximum && *maximum > 0 => {
            Some(ModelFieldType::Bytes {
                minimum: *minimum,
                maximum: Some(*maximum),
            })
        }
        ("Decimal", [precision, scale])
            if *precision > 0 && *precision <= 34 && scale <= precision =>
        {
            Some(ModelFieldType::Decimal {
                precision: *precision as u8,
                scale: *scale as u8,
            })
        }
        ("Decimal", _) | ("Text", _) | ("Bytes", _) => {
            errors.push(Diagnostic::error(
                "C014",
                "invalid storage type parameters",
                reference.span,
            ));
            return None;
        }
        _ => None,
    };
    if simple.is_some() {
        return simple;
    }
    if !parameters.is_empty() {
        errors.push(Diagnostic::error(
            "C014",
            "this Model field type does not accept storage parameters",
            reference.span,
        ));
        return None;
    }
    let qualified = if name.contains('.') {
        name.clone()
    } else {
        format!("{package}.{name}")
    };
    let Some(id) = symbols.types.get(&qualified).copied() else {
        errors.push(Diagnostic::error(
            "C004",
            format!("unknown Model field type '{qualified}'"),
            reference.span,
        ));
        return None;
    };
    match types[id].kind {
        DefinitionKind::ModelId => Some(ModelFieldType::ModelId(
            qualified.trim_end_matches(".id").into(),
        )),
        DefinitionKind::Regular | DefinitionKind::ModelChoice => match &types[id].shape {
            Shape::Choice(variants)
                if variants
                    .iter()
                    .all(|variant| !variant.error && variant.fields.is_empty()) =>
            {
                Some(ModelFieldType::Choice(qualified))
            }
            _ => {
                errors.push(Diagnostic::error(
                    "C014",
                    "only no-payload choices can be stored as Model fields",
                    reference.span,
                ));
                None
            }
        },
        DefinitionKind::Model | DefinitionKind::ModelPage | DefinitionKind::ModelCursor => {
            errors.push(Diagnostic::error(
                "C014",
                "Model relations must reference the target '.id' type",
                reference.span,
            ));
            None
        }
    }
}

fn normalize_value(
    expression: &Expression,
    ty: &ModelFieldType,
    nullable: bool,
    choices: &[ModelChoice],
    errors: &mut Vec<Diagnostic>,
) -> Option<ModelValue> {
    let value = match &expression.kind {
        ExpressionKind::Literal(crate::syntax::Literal::Null) if nullable => ModelValue::Null,
        ExpressionKind::Literal(crate::syntax::Literal::Bool(value))
            if matches!(ty, ModelFieldType::Bool) =>
        {
            ModelValue::Bool(*value)
        }
        ExpressionKind::Literal(crate::syntax::Literal::Number(value))
            if matches!(
                ty,
                ModelFieldType::Int
                    | ModelFieldType::Float
                    | ModelFieldType::Decimal { .. }
                    | ModelFieldType::DateTime
                    | ModelFieldType::Date
                    | ModelFieldType::Time
                    | ModelFieldType::Duration
            ) =>
        {
            ModelValue::Number(value.clone())
        }
        ExpressionKind::Unary {
            operator: UnaryOperator::Negate,
            value,
        } if matches!(
            ty,
            ModelFieldType::Int
                | ModelFieldType::Float
                | ModelFieldType::Decimal { .. }
                | ModelFieldType::Duration
        ) =>
        {
            match &value.kind {
                ExpressionKind::Literal(crate::syntax::Literal::Number(value)) => {
                    ModelValue::Number(format!("-{value}"))
                }
                _ => return invalid_value(expression, errors),
            }
        }
        ExpressionKind::Literal(crate::syntax::Literal::Text(value))
            if matches!(ty, ModelFieldType::Text { .. } | ModelFieldType::Uuid) =>
        {
            if matches!(ty, ModelFieldType::Uuid)
                && let Err(error) = dever_runtime::orm::Uuid::parse(value)
            {
                errors.push(Diagnostic::error(
                    "C014",
                    error.to_string(),
                    expression.span,
                ));
                return None;
            }
            ModelValue::Text(value.clone())
        }
        ExpressionKind::Name(path) => {
            let name = path_name(path);
            let ModelFieldType::Choice(choice) = ty else {
                return invalid_value(expression, errors);
            };
            let valid = choices.iter().any(|candidate| {
                candidate.name == *choice
                    && candidate.options.iter().any(|(variant, _)| {
                        name == format!("{}.{}", candidate.name, variant)
                            || name
                                == format!(
                                    "{}.{}",
                                    candidate.name.rsplit('.').next().unwrap(),
                                    variant
                                )
                    })
            });
            if !valid {
                return invalid_value(expression, errors);
            }
            ModelValue::Choice(name)
        }
        _ => return invalid_value(expression, errors),
    };
    if let Err(message) = validate_constant(&value, ty) {
        errors.push(Diagnostic::error("C014", message, expression.span));
        return None;
    }
    Some(value)
}

fn validate_constant(value: &ModelValue, ty: &ModelFieldType) -> Result<(), String> {
    match (value, ty) {
        (ModelValue::Number(value), ModelFieldType::Float) => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|_| ())
            .ok_or_else(|| "Model Float constant must be finite".to_owned()),
        (ModelValue::Number(value), ModelFieldType::Decimal { precision, scale }) => {
            dever_runtime::orm::decimal_storage(value, *precision, *scale)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
        (ModelValue::Number(value), _) => value.parse::<i64>().map(|_| ()).map_err(|_| {
            "Model integer constant must be an integer in the 64-bit range".to_owned()
        }),
        (ModelValue::Text(value), ModelFieldType::Text { minimum, maximum }) => {
            let length = value.chars().count() as u64;
            if length < u64::from(*minimum)
                || maximum.is_some_and(|maximum| length > u64::from(maximum))
            {
                Err(
                    "Model Text constant is outside the declared codepoint length bounds"
                        .to_owned(),
                )
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

fn invalid_value(expression: &Expression, errors: &mut Vec<Diagnostic>) -> Option<ModelValue> {
    errors.push(Diagnostic::error(
        "C014",
        "Model defaults and Seed values must be constants matching the field type",
        expression.span,
    ));
    None
}

fn normalize_indexes(
    package: &Package,
    fields: &[ModelField],
    source_by_name: &BTreeMap<String, &Field>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<ModelIndex> {
    let known = fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut candidates = vec![ModelIndex {
        fields: vec!["id".into()],
        unique: true,
        implicit: true,
    }];
    for (name, field) in source_by_name {
        if field.storage.index || field.storage.unique {
            candidates.push(ModelIndex {
                fields: vec![name.clone()],
                unique: field.storage.unique,
                implicit: false,
            });
        }
        if fields.iter().any(|model_field| {
            model_field.name == *name
                && (matches!(model_field.ty, ModelFieldType::ModelId(_))
                    || model_field.owner.is_some())
        }) {
            candidates.push(ModelIndex {
                fields: vec![name.clone()],
                unique: false,
                implicit: true,
            });
        }
    }
    let search_fields = source_by_name
        .iter()
        .filter(|(_, field)| field.storage.search.is_some())
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    if let [search] = search_fields.as_slice() {
        let mut indexed = fields
            .iter()
            .find(|field| field.owner.is_some())
            .map(|field| vec![field.name.clone()])
            .unwrap_or_default();
        indexed.push(search.clone());
        candidates.push(ModelIndex {
            fields: indexed,
            unique: false,
            implicit: true,
        });
    }
    for declaration in &package.declarations {
        let Declaration::ModelIndex(index) = declaration else {
            continue;
        };
        let names = index
            .fields
            .iter()
            .map(|field| field.text.clone())
            .collect::<Vec<_>>();
        let mut unique_names = BTreeSet::new();
        for field in &index.fields {
            if !known.contains(field.text.as_str()) {
                errors.push(Diagnostic::error(
                    "C004",
                    format!("unknown Model index field '{}'", field.text),
                    field.span,
                ));
            }
            if !unique_names.insert(&field.text) {
                errors.push(Diagnostic::error(
                    "C002",
                    "an index cannot repeat a field",
                    field.span,
                ));
            }
        }
        candidates.push(ModelIndex {
            fields: names,
            unique: index.unique,
            implicit: false,
        });
    }
    let mut normalized: BTreeMap<Vec<String>, ModelIndex> = BTreeMap::new();
    for index in candidates {
        if let Some(existing) = normalized.get_mut(&index.fields) {
            if existing.unique == index.unique && !existing.implicit && !index.implicit {
                errors.push(Diagnostic::error(
                    "C002",
                    "duplicate Model index",
                    package.span,
                ));
            }
            existing.unique |= index.unique;
            existing.implicit &= index.implicit;
        } else {
            normalized.insert(index.fields.clone(), index);
        }
    }
    normalized.into_values().collect()
}

fn normalize_seed(
    package: &Package,
    fields: &[ModelField],
    indexes: &[ModelIndex],
    choices: &[ModelChoice],
    errors: &mut Vec<Diagnostic>,
) -> Vec<SeedRow> {
    let Some(seed) = package
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Seed(seed) => Some(seed),
            _ => None,
        })
    else {
        return Vec::new();
    };
    let writable = fields
        .iter()
        .filter(|field| !field.generated)
        .map(|field| (field.name.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let unique = indexes
        .iter()
        .filter(|index| {
            index.unique
                && index
                    .fields
                    .iter()
                    .all(|field| writable.contains_key(field.as_str()))
        })
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    for row in &seed.rows {
        let mut seen = BTreeSet::new();
        let mut normalized = Vec::new();
        for value in row {
            let Some(field) = writable.get(value.name.text.as_str()) else {
                errors.push(Diagnostic::error(
                    "C004",
                    format!("unknown or generated Seed field '{}'", value.name.text),
                    value.name.span,
                ));
                continue;
            };
            if !seen.insert(value.name.text.as_str()) {
                errors.push(Diagnostic::error(
                    "C002",
                    "duplicate Seed field",
                    value.name.span,
                ));
                continue;
            }
            if let Some(value) =
                normalize_value(&value.value, &field.ty, field.nullable, choices, errors)
            {
                normalized.push((field.name.clone(), value));
            }
        }
        for field in writable.values() {
            if !field.nullable && field.default.is_none() && !seen.contains(field.name.as_str()) {
                errors.push(Diagnostic::error(
                    "C014",
                    format!("Seed row is missing required field '{}'", field.name),
                    seed.span,
                ));
            }
        }
        let identity = unique.iter().find(|index| {
            index.fields.iter().all(|identity_field| {
                normalized.iter().any(|(field, value)| {
                    field == identity_field && !matches!(value, ModelValue::Null)
                })
            })
        });
        if identity.is_none() {
            errors.push(Diagnostic::error(
                "C014",
                "each Seed row must provide non-null values for every field of a unique key",
                seed.span,
            ));
        }
        normalized.sort_by(|left, right| left.0.cmp(&right.0));
        rows.push(SeedRow {
            fields: normalized,
            identity: identity
                .map(|index| index.fields.clone())
                .unwrap_or_default(),
        });
    }
    rows
}

fn normalize_migrations(
    package: &Package,
    fields: &[ModelField],
    errors: &mut Vec<Diagnostic>,
) -> Vec<ModelMigration> {
    let current = fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    package
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Migration(migration) => {
                let mut drops = Vec::new();
                let mut statements = Vec::new();
                let mut seen = BTreeSet::new();
                for operation in &migration.operations {
                    match operation {
                        MigrationOperation::Drop(field) => {
                            if current.contains(field.text.as_str()) {
                                errors.push(Diagnostic::error(
                                    "C014",
                                    "a migration cannot drop a field in the target schema",
                                    field.span,
                                ));
                            }
                            if !seen.insert(&field.text) {
                                errors.push(Diagnostic::error(
                                    "C002",
                                    "duplicate migration operation",
                                    field.span,
                                ));
                            }
                            drops.push(field.text.clone());
                        }
                        MigrationOperation::Sql(sql) => {
                            let parameters = sql
                                .parameters
                                .iter()
                                .filter_map(|value| migration_parameter(value, errors))
                                .collect::<Vec<_>>();
                            for (text, marker, other) in
                                [(&sql.sqlite, '?', '$'), (&sql.postgres, '$', '?')]
                            {
                                validate_sql(
                                    &text.value,
                                    marker,
                                    other,
                                    sql.parameters.len(),
                                    text.span,
                                    true,
                                    errors,
                                );
                            }
                            statements.push(ModelMigrationSql {
                                phase: sql.phase,
                                sqlite: sql.sqlite.value.clone(),
                                postgres: sql.postgres.value.clone(),
                                parameters,
                            });
                        }
                    }
                }
                Some(ModelMigration {
                    name: migration.name.text.clone(),
                    drops,
                    statements,
                })
            }
            _ => None,
        })
        .collect()
}

fn migration_parameter(
    expression: &Expression,
    errors: &mut Vec<Diagnostic>,
) -> Option<(ModelFieldType, ModelValue)> {
    use crate::syntax::Literal;
    let literal = match &expression.kind {
        ExpressionKind::Literal(literal) => literal,
        ExpressionKind::Unary {
            operator: UnaryOperator::Negate,
            value,
        } => match &value.kind {
            ExpressionKind::Literal(literal @ Literal::Number(_)) => literal,
            _ => {
                errors.push(Diagnostic::error(
                    "C014",
                    "migration parameters must be static literals",
                    expression.span,
                ));
                return None;
            }
        },
        _ => {
            errors.push(Diagnostic::error(
                "C014",
                "migration parameters must be static literals",
                expression.span,
            ));
            return None;
        }
    };
    let ty = match literal {
        Literal::Null | Literal::Text(_) => ModelFieldType::Text {
            minimum: 0,
            maximum: None,
        },
        Literal::Bool(_) => ModelFieldType::Bool,
        Literal::Number(value) if value.contains(['e', 'E']) => ModelFieldType::Float,
        Literal::Number(value) if value.contains('.') => {
            errors.push(Diagnostic::error("C014", "migration Decimal parameters require an explicit SQL conversion from bound Text; use exponent notation only for Float", expression.span));
            return None;
        }
        Literal::Number(_) => ModelFieldType::Int,
    };
    normalize_value(expression, &ty, true, &[], errors).map(|value| (ty, value))
}

fn validate_model_references(
    schemas: &mut [ModelSchema],
    packages: &[Package],
    registrations: &[Registration],
    errors: &mut Vec<Diagnostic>,
) {
    let by_package = schemas
        .iter()
        .enumerate()
        .map(|(index, schema)| (schema.package.clone(), index))
        .collect::<BTreeMap<_, _>>();
    for (index, registration) in registrations.iter().enumerate() {
        let source_scope = schemas[index].scope;
        for field_index in 0..schemas[index].fields.len() {
            let ModelFieldType::ModelId(target) = &schemas[index].fields[field_index].ty else {
                continue;
            };
            let Some(target_index) = by_package.get(target).copied() else {
                continue;
            };
            let target_scope = schemas[target_index].scope;
            if source_scope == ModelScope::Global && target_scope == ModelScope::Tenant {
                errors.push(Diagnostic::error(
                    "C014",
                    format!(
                        "global Model field '{}' cannot reference tenant Model '{}'",
                        schemas[index].fields[field_index].name, target
                    ),
                    packages[registration.owner].span,
                ));
            } else if source_scope == ModelScope::Tenant && target_scope == ModelScope::Global {
                schemas[index].fields[field_index].logical_reference = true;
            }
        }
    }
}

fn validate_relations(
    schemas: &mut [ModelSchema],
    packages: &[Package],
    registrations: &[Registration],
    errors: &mut Vec<Diagnostic>,
) {
    let by_package = schemas
        .iter()
        .enumerate()
        .map(|(index, schema)| (schema.package.clone(), index))
        .collect::<BTreeMap<_, _>>();
    for (index, registration) in registrations.iter().enumerate() {
        let package = &packages[registration.owner];
        let mut names = BTreeSet::new();
        let source_scope = schemas[index].scope;
        for relation_index in 0..schemas[index].relations.len() {
            let relation = schemas[index].relations[relation_index].clone();
            if !names.insert(relation.name.clone()) {
                errors.push(Diagnostic::error(
                    "C002",
                    "duplicate relation name",
                    package.span,
                ));
            }
            let Some(target_index) = by_package.get(&relation.model).copied() else {
                errors.push(Diagnostic::error(
                    "C004",
                    format!("unknown relation Model '{}'", relation.model),
                    package.span,
                ));
                continue;
            };
            let target_scope = schemas[target_index].scope;
            if source_scope == ModelScope::Global && target_scope == ModelScope::Tenant {
                if relation.kind == ModelRelationKind::ToMany {
                    errors.push(Diagnostic::error(
                        "C014",
                        "a global Model cannot declare a relation to a tenant Model",
                        package.span,
                    ));
                }
                continue;
            }
            let logical = source_scope == ModelScope::Tenant && target_scope == ModelScope::Global;
            let valid = match relation.kind {
                ModelRelationKind::ToOne => schemas[index].fields.iter().any(|field| {
                    field.name == relation.field
                        && matches!(&field.ty, ModelFieldType::ModelId(model) if model == &schemas[target_index].package)
                }),
                ModelRelationKind::ToMany => schemas[target_index].fields.iter().any(|field| {
                    field.name == relation.field
                        && matches!(&field.ty, ModelFieldType::ModelId(model) if model == &schemas[index].package)
                }),
            };
            if !valid {
                errors.push(Diagnostic::error(
                    "C014",
                    "a relation must name a foreign-key field that references this Model",
                    package.span,
                ));
            } else {
                schemas[index].relations[relation_index].logical = logical;
            }
        }
    }
}

fn validate_table_names(
    schemas: &[ModelSchema],
    packages: &[Package],
    registrations: &[Registration],
    errors: &mut Vec<Diagnostic>,
) {
    let mut names = BTreeMap::new();
    for (schema, registration) in schemas.iter().zip(registrations) {
        let selector = match &schema.connection {
            ConnectionSelector::Explicit(name) | ConnectionSelector::PackageRoot(name) => name,
        };
        if names
            .insert((selector, &schema.table), &schema.package)
            .is_some()
        {
            errors.push(Diagnostic::error(
                "C014",
                "Models bound to the same logical connection cannot share a table name",
                packages[registration.owner].span,
            ));
        }
    }
}

fn schema_identity(schema: &ModelSchema) -> String {
    let mut fields = schema.fields.iter().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    let mut indexes = schema.indexes.iter().collect::<Vec<_>>();
    indexes.sort_by(|left, right| left.fields.cmp(&right.fields));
    let mut migrations = schema.migrations.iter().collect::<Vec<_>>();
    migrations.sort_by(|left, right| left.name.cmp(&right.name));
    let scope = match schema.scope {
        // Keep existing tenant revisions stable when tenancy is not enabled.
        ModelScope::Tenant => "",
        ModelScope::Global => "global|",
    };
    format!(
        "{scope}{}|{}|{}|{}|{}|{}",
        schema.package,
        schema.table,
        fields
            .iter()
            .map(|field| format!(
                "{}:{}:{}:{}:{}:{}{}",
                field.name,
                model::field_type(&field.ty),
                field.nullable,
                field.generated,
                field.default.as_ref().map(model::value).unwrap_or_default(),
                field.rename_from.as_deref().unwrap_or_default(),
                if field.logical_reference {
                    ":logical"
                } else {
                    ""
                }
            ))
            .collect::<Vec<_>>()
            .join(";"),
        indexes
            .iter()
            .map(|index| format!("{}:{}", index.unique, index.fields.join(",")))
            .collect::<Vec<_>>()
            .join(";"),
        schema
            .choices
            .iter()
            .map(|choice| format!(
                "{}:{}",
                choice.name,
                choice
                    .options
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ))
            .collect::<Vec<_>>()
            .join(";"),
        migrations
            .iter()
            .map(|migration| {
                if migration.statements.is_empty() {
                    format!("{}:{}", migration.name, migration.drops.join(","))
                } else {
                    format!("{}:{}", migration.name, migration.revision())
                }
            })
            .collect::<Vec<_>>()
            .join(";")
    )
}

fn seed_identity(rows: &[SeedRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}:{}",
                row.identity.join(","),
                row.fields
                    .iter()
                    .map(|(name, value)| format!("{name}={}", model::value(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn model_type_name(stem: &str) -> String {
    stem.split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn declaration_span(declaration: &Declaration) -> crate::source::Span {
    match declaration {
        Declaration::Api(value) => value.span,
        Declaration::Rest(value) => value.span,
        Declaration::External(value) => value.span,
        Declaration::Type(value) => value.span,
        Declaration::Function(value) => value.span,
        Declaration::Database(value) => value.span,
        Declaration::Schedule(value) => value.span,
        Declaration::ModelIndex(value) => value.span,
        Declaration::Relation(value) => value.span,
        Declaration::Seed(value) => value.span,
        Declaration::Migration(value) => value.span,
        Declaration::ModelSql(value) => value.span,
    }
}
