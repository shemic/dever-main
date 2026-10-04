use std::collections::BTreeMap;

use crate::hir::Program;
use crate::types::{Definition, Shape};
use crate::wire::{Policy, Schema};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelSchema {
    pub record: usize,
    pub page: usize,
    pub cursor: usize,
    pub package: String,
    pub name: String,
    pub scope: ModelScope,
    pub table: String,
    pub connection: ConnectionSelector,
    pub fields: Vec<ModelField>,
    pub choices: Vec<ModelChoice>,
    pub indexes: Vec<ModelIndex>,
    pub relations: Vec<ModelRelation>,
    pub sql: Vec<ModelSql>,
    pub seed: Vec<SeedRow>,
    pub migrations: Vec<ModelMigration>,
    pub revision: String,
    pub seed_revision: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelScope {
    Global,
    Tenant,
}

impl ModelScope {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Tenant => "tenant",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RestContract {
    pub read: Vec<usize>,
    pub create: Vec<RestWriteField>,
    pub replace: Vec<RestWriteField>,
    pub owner: Option<(usize, RestOwnerSource)>,
    pub search: Option<RestSearch>,
}

#[derive(Clone, Debug)]
pub(crate) struct RestWriteField {
    pub field: usize,
    pub source: RestValueSource,
}

#[derive(Clone, Debug)]
pub(crate) enum RestValueSource {
    Input,
    Binding {
        expression: crate::hir::Expression,
        uses_input: bool,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct RestSearch {
    pub field: usize,
    pub expression: crate::hir::Expression,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RestOwnerSource {
    User,
    Tenant,
}

impl ModelSchema {
    pub(crate) fn rest_contract(&self, types: &[Definition]) -> Result<RestContract, String> {
        let Shape::Record(fields) = &types[self.record].shape else {
            unreachable!("checked Model record")
        };
        let mut contract = RestContract {
            read: Vec::new(),
            create: Vec::new(),
            replace: Vec::new(),
            owner: None,
            search: None,
        };
        for (index, (storage, field)) in self.fields.iter().zip(fields).enumerate() {
            if let Some(owner) = storage.owner {
                contract.owner = Some((index, owner));
            }
            if field.private {
                continue;
            }
            Schema::build(&field.ty, types, Policy::Output).map_err(|error| {
                format!(
                    "REST field '{}.{}' cannot be serialized: {error}",
                    self.package, storage.name
                )
            })?;
            contract.read.push(index);
            if storage.owner.is_some() {
                continue;
            }
            if storage.generated {
                continue;
            }
            Schema::build(&field.ty, types, Policy::ApiInput).map_err(|error| {
                format!(
                    "REST field '{}.{}' cannot be read from a request: {error}",
                    self.package, storage.name
                )
            })?;
            contract.replace.push(RestWriteField {
                field: index,
                source: RestValueSource::Input,
            });
            if storage.default.is_none() {
                contract.create.push(RestWriteField {
                    field: index,
                    source: RestValueSource::Input,
                });
            }
        }
        if contract.replace.is_empty() {
            return Err(format!(
                "REST Model '{}' has no writable public fields",
                self.package
            ));
        }
        Ok(contract)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionSelector {
    Explicit(String),
    PackageRoot(String),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum DatabaseOwner {
    Model(usize),
    Job(usize),
}

impl Program {
    pub(crate) fn rest_binding_roots(&self) -> std::collections::BTreeSet<usize> {
        let mut roots = std::collections::BTreeSet::new();
        for route in &self.api_rest {
            for expression in route
                .contract
                .create
                .iter()
                .chain(&route.contract.replace)
                .filter_map(|write| match &write.source {
                    RestValueSource::Binding { expression, .. } => Some(expression),
                    RestValueSource::Input => None,
                })
                .chain(
                    route
                        .contract
                        .search
                        .iter()
                        .map(|search| &search.expression),
                )
            {
                crate::check::visit_expression(expression, |value| {
                    if let Some((target, arguments)) = value.kind.static_call() {
                        if let crate::hir::CallTarget::Function(target) = target {
                            roots.insert(target);
                        }
                        for argument in arguments {
                            if let crate::hir::CallArgument::Handler(
                                crate::hir::HandlerTarget::Function(target),
                            ) = argument
                            {
                                roots.insert(*target);
                            }
                        }
                    }
                    if let crate::hir::ExpressionKind::Collection {
                        handler: Some(crate::hir::HandlerTarget::Function(target)),
                        ..
                    } = &value.kind
                    {
                        roots.insert(*target);
                    }
                });
            }
        }
        roots
    }

    pub(crate) fn database_binding(&self, owner: DatabaseOwner) -> (&ConnectionSelector, &str) {
        match owner {
            DatabaseOwner::Model(id) => (&self.models[id].connection, &self.models[id].package),
            DatabaseOwner::Job(function) => {
                let job = self
                    .jobs
                    .iter()
                    .find(|job| job.function == function)
                    .expect("checked Job target");
                (&job.connection, &self.functions[function].name)
            }
        }
    }

    pub(crate) fn database_selector(&self, owner: DatabaseOwner) -> (Option<&str>, &str) {
        let (connection, identity) = self.database_binding(owner);
        match connection {
            ConnectionSelector::Explicit(name) => (Some(name), identity.split('.').next().unwrap()),
            ConnectionSelector::PackageRoot(root) => (None, root),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelField {
    pub name: String,
    pub ty: ModelFieldType,
    pub nullable: bool,
    pub generated: bool,
    pub default: Option<ModelValue>,
    pub rename_from: Option<String>,
    pub logical_reference: bool,
    pub owner: Option<RestOwnerSource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ModelFieldType {
    Bool,
    Int,
    Float,
    Decimal { precision: u8, scale: u8 },
    Text { minimum: u32, maximum: Option<u32> },
    Bytes { minimum: u32, maximum: Option<u32> },
    Uuid,
    DateTime,
    Date,
    Time,
    Duration,
    Json,
    Choice(String),
    ModelId(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ModelValue {
    Null,
    Bool(bool),
    Number(String),
    Text(String),
    Choice(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelChoice {
    pub name: String,
    pub options: Vec<(String, Option<String>)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelIndex {
    pub fields: Vec<String>,
    pub unique: bool,
    pub implicit: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelRelation {
    pub name: String,
    pub model: String,
    pub field: String,
    pub kind: ModelRelationKind,
    pub logical: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelRelationKind {
    ToOne,
    ToMany,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelSql {
    pub name: String,
    pub parameters: Vec<ModelSqlParameter>,
    pub result: ModelSqlResult,
    pub sqlite: String,
    pub postgres: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelSqlParameter {
    pub name: String,
    pub ty: ModelFieldType,
    pub nullable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModelSqlResult {
    pub record: usize,
    pub cardinality: ModelSqlCardinality,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelSqlCardinality {
    One,
    Optional,
    Many,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SeedRow {
    pub fields: Vec<(String, ModelValue)>,
    pub identity: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelMigration {
    pub name: String,
    pub drops: Vec<String>,
    pub statements: Vec<ModelMigrationSql>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelMigrationSql {
    pub phase: crate::syntax::MigrationPhase,
    pub sqlite: String,
    pub postgres: String,
    pub parameters: Vec<(ModelFieldType, ModelValue)>,
}

impl ModelMigration {
    pub fn revision(&self) -> String {
        let identity = if self.statements.is_empty() {
            format!("{}:{}", self.name, self.drops.join(","))
        } else {
            format!("{:?}:{:?}:{:?}", self.name, self.drops, self.statements)
        };
        checksum(&identity)
    }
}

impl Program {
    /// Stable logical schema output. SQL dialect details and source locations are excluded.
    pub fn model_snapshot(&self) -> String {
        let mut models = self.models.iter().collect::<Vec<_>>();
        models.sort_by(|left, right| left.package.cmp(&right.package));
        let mut output = String::from("dever-model 1\n");
        for model in models {
            output.push_str(&format!(
                "model {} name={} table={} connection={} scope={} revision={} seed={}\n",
                model.package,
                model.name,
                model.table,
                connection(&model.connection),
                model.scope.name(),
                model.revision,
                model.seed_revision,
            ));
            for choice in &model.choices {
                let options = choice
                    .options
                    .iter()
                    .map(|(name, label)| match label {
                        Some(label) => format!("{name}={label:?}"),
                        None => name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                output.push_str(&format!("choice {} [{options}]\n", choice.name));
            }
            for field in &model.fields {
                let reference = if field.logical_reference {
                    " reference=logical"
                } else {
                    ""
                };
                let owner = match field.owner {
                    Some(RestOwnerSource::User) => " owner=auth.user_id",
                    Some(RestOwnerSource::Tenant) => " owner=auth.tenant_id",
                    None => "",
                };
                output.push_str(&format!(
                    "field {} {} nullable={} generated={} default={} from={}{reference}{owner}\n",
                    field.name,
                    field_type(&field.ty),
                    field.nullable,
                    field.generated,
                    field.default.as_ref().map(value).unwrap_or("-".into()),
                    field.rename_from.as_deref().unwrap_or("-"),
                ));
            }
            for index in &model.indexes {
                output.push_str(&format!(
                    "index {} ({}) implicit={}\n",
                    if index.unique { "unique" } else { "plain" },
                    index.fields.join(","),
                    index.implicit,
                ));
            }
            for relation in &model.relations {
                let kind = match relation.kind {
                    ModelRelationKind::ToOne => "to-one",
                    ModelRelationKind::ToMany => "to-many",
                };
                output.push_str(&format!(
                    "relation {}{} {} = {}.{}\n",
                    if relation.logical { "logical " } else { "" },
                    kind,
                    relation.name,
                    relation.model,
                    relation.field
                ));
            }
            for sql in &model.sql {
                output.push_str(&format!(
                    "sql {} ({}) -> {:?}:{}\n",
                    sql.name,
                    sql.parameters
                        .iter()
                        .map(|parameter| format!(
                            "{}:{}{}",
                            parameter.name,
                            field_type(&parameter.ty),
                            if parameter.nullable { "?" } else { "" }
                        ))
                        .collect::<Vec<_>>()
                        .join(","),
                    sql.result.cardinality,
                    self.types[sql.result.record].name,
                ));
            }
            for row in &model.seed {
                output.push_str(&format!(
                    "seed key({}) {{{}}}\n",
                    row.identity.join(","),
                    row.fields
                        .iter()
                        .map(|(name, item)| format!("{name}={}", value(item)))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            for migration in &model.migrations {
                output.push_str(&format!(
                    "migrate {} drop({})\n",
                    migration.name,
                    migration.drops.join(",")
                ));
                if !migration.statements.is_empty() {
                    output.push_str(&format!(
                        "migration-data {} revision={}\n",
                        migration.name,
                        migration.revision()
                    ));
                }
            }
        }
        output
    }

    pub fn validate_database_settings(
        &self,
        settings: &dever_runtime::config::Settings,
    ) -> Result<dever_runtime::config::RuntimeProfile, String> {
        self.validate_database_bindings(&settings.compilation_bindings())
    }

    pub fn validate_database_bindings(
        &self,
        settings: &dever_runtime::config::CompilationBindings,
    ) -> Result<dever_runtime::config::RuntimeProfile, String> {
        for job in &self.jobs {
            let (explicit, root) = self.database_selector(DatabaseOwner::Job(job.function));
            settings.resolve_database(explicit, root)?;
        }
        let connections = self
            .models
            .iter()
            .map(|model| resolve_connection(settings, model))
            .collect::<Result<Vec<_>, _>>()?;
        let model_ids = self
            .models
            .iter()
            .enumerate()
            .map(|(index, model)| (model.package.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        let mut tables = BTreeMap::new();
        for (index, model) in self.models.iter().enumerate() {
            if matches!(
                connections[index].1,
                dever_runtime::config::DatabaseDriver::Postgres
            ) {
                validate_postgres_identifiers(model)?;
            }
            if let Some(existing) = tables.insert(
                (connections[index].0, model.table.as_str()),
                model.package.as_str(),
            ) {
                return Err(format!(
                    "Models '{}' and '{}' resolve to database '{}' and share table '{}'",
                    existing, model.package, connections[index].0, model.table
                ));
            }
            for field in &model.fields {
                let ModelFieldType::ModelId(target) = &field.ty else {
                    continue;
                };
                let target = *model_ids
                    .get(target.as_str())
                    .expect("validated Model foreign key");
                if connections[index].0 != connections[target].0 {
                    return Err(format!(
                        "foreign key '{}.{}' crosses database connections '{}' and '{}'",
                        model.package, field.name, connections[index].0, connections[target].0
                    ));
                }
            }
        }
        let instances = crate::specialize::concrete(self)
            .map_err(|_| "recursive specialized handler calls are not allowed".to_owned())?;
        for instance in instances {
            let function = &self.functions[instance.function];
            if function.kind != crate::syntax::FunctionKind::Transaction {
                continue;
            }
            let mut connection = None;
            for owner in crate::specialize::database_effects(self, &instance) {
                let (explicit, root) = self.database_selector(owner);
                let name = settings.resolve_database(explicit, root)?.0;
                if connection
                    .replace(name)
                    .is_some_and(|current| current != name)
                {
                    return Err(format!(
                        "transaction '{}' spans multiple database connections",
                        function.name
                    ));
                }
            }
        }
        Ok(settings.profile())
    }
}

fn resolve_connection<'a>(
    settings: &'a dever_runtime::config::CompilationBindings,
    model: &ModelSchema,
) -> Result<(&'a str, &'a dever_runtime::config::DatabaseDriver), String> {
    match &model.connection {
        ConnectionSelector::Explicit(name) => settings.resolve_database(Some(name), &model.package),
        ConnectionSelector::PackageRoot(root) => settings.resolve_database(None, root),
    }
}

fn validate_postgres_identifiers(model: &ModelSchema) -> Result<(), String> {
    let identifiers = std::iter::once(("table", model.table.as_str()))
        .chain(model.fields.iter().flat_map(|field| {
            std::iter::once(("field", field.name.as_str())).chain(
                field
                    .rename_from
                    .iter()
                    .map(|name| ("renamed field", name.as_str())),
            )
        }))
        .chain(model.migrations.iter().flat_map(|migration| {
            migration
                .drops
                .iter()
                .map(|name| ("dropped field", name.as_str()))
        }));
    for (kind, identifier) in identifiers {
        if identifier.len() > 63 {
            return Err(format!(
                "PostgreSQL {kind} identifier '{}.{}' exceeds 63 bytes",
                model.package, identifier
            ));
        }
    }
    Ok(())
}

fn connection(selector: &ConnectionSelector) -> String {
    match selector {
        ConnectionSelector::Explicit(name) => format!("explicit:{name}"),
        ConnectionSelector::PackageRoot(name) => format!("package:{name}"),
    }
}

pub(crate) fn field_type(ty: &ModelFieldType) -> String {
    match ty {
        ModelFieldType::Bool => "Bool".into(),
        ModelFieldType::Int => "Int".into(),
        ModelFieldType::Float => "Float".into(),
        ModelFieldType::Decimal { precision, scale } => format!("Decimal({precision},{scale})"),
        ModelFieldType::Text { minimum, maximum } => bounded("Text", *minimum, *maximum),
        ModelFieldType::Bytes { minimum, maximum } => bounded("Bytes", *minimum, *maximum),
        ModelFieldType::Uuid => "Uuid".into(),
        ModelFieldType::DateTime => "DateTime".into(),
        ModelFieldType::Date => "Date".into(),
        ModelFieldType::Time => "Time".into(),
        ModelFieldType::Duration => "Duration".into(),
        ModelFieldType::Json => "Json".into(),
        ModelFieldType::Choice(name) => format!("choice:{name}"),
        ModelFieldType::ModelId(model) => format!("id:{model}"),
    }
}

fn bounded(name: &str, minimum: u32, maximum: Option<u32>) -> String {
    match (minimum, maximum) {
        (0, None) => name.into(),
        (0, Some(maximum)) => format!("{name}({maximum})"),
        (minimum, Some(maximum)) => format!("{name}({minimum},{maximum})"),
        (_, None) => unreachable!("minimum requires a maximum"),
    }
}

pub(crate) fn value(value: &ModelValue) -> String {
    match value {
        ModelValue::Null => "null".into(),
        ModelValue::Bool(value) => value.to_string(),
        ModelValue::Number(value) => value.clone(),
        ModelValue::Text(value) => format!("{value:?}"),
        ModelValue::Choice(value) => value.clone(),
    }
}

pub(crate) fn checksum(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
