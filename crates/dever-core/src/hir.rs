use std::collections::BTreeMap;

use dever_runtime::number::DecimalValue;

use crate::diagnostic::Diagnostic;
use crate::intrinsic::Intrinsic;
use crate::source::Span;
use crate::syntax::{BinaryOperator, UnaryOperator};
use crate::types::{Definition, Field, Parameter, Type};

/// Only semantic checking can construct a program for native emission.
#[derive(Clone, Debug)]
pub struct Program {
    pub(crate) types: Vec<Definition>,
    pub(crate) functions: Vec<Function>,
    pub(crate) warnings: Vec<Diagnostic>,
    pub(crate) packages: Vec<PackageContract>,
    pub(crate) effects: Vec<std::collections::BTreeSet<&'static str>>,
    pub(crate) recoveries: Vec<std::collections::BTreeSet<String>>,
    pub(crate) suspension_handlers: Vec<std::collections::BTreeSet<usize>>,
    pub(crate) failures: Vec<std::collections::BTreeSet<Failure>>,
    pub(crate) failure_handlers: Vec<std::collections::BTreeSet<usize>>,
    pub(crate) database_effects: Vec<std::collections::BTreeSet<crate::model::DatabaseOwner>>,
    pub(crate) models: Vec<crate::model::ModelSchema>,
    pub(crate) jobs: Vec<Job>,
    pub(crate) api_routes: Vec<ApiRoute>,
    pub(crate) api_commands: Vec<ApiCommand>,
    pub(crate) api_rest: Vec<ApiRest>,
    pub(crate) permissions: Vec<Permission>,
    pub(crate) tenant_components: Vec<String>,
    pub(crate) tests: Vec<TestCase>,
    pub(crate) adapters: Vec<Adapter>,
    pub(crate) auth: Vec<AuthHook>,
}

#[derive(Clone, Debug)]
pub(crate) struct AuthHook {
    pub provider: String,
    pub function: usize,
    pub claims: usize,
    pub identity: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Job {
    pub function: usize,
    pub connection: crate::model::ConnectionSelector,
    pub payload: Option<crate::wire::Schema>,
    pub attempts: u32,
    pub timeout_ms: u32,
    pub schedule: Option<String>,
    pub components: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TestCase {
    pub(crate) name: String,
    pub(crate) function: usize,
    pub(crate) span: Span,
    pub(crate) database_connections: Vec<String>,
    pub(crate) port_bindings: std::collections::BTreeMap<usize, usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Adapter {
    pub owner: usize,
    pub name: String,
    pub port_owner: usize,
    pub identity: String,
    pub operations: std::collections::BTreeMap<usize, usize>,
    pub fake: bool,
    pub setting: Option<usize>,
    pub implementation: AdapterImplementation,
}

#[derive(Clone, Debug)]
pub(crate) enum AdapterImplementation {
    Dever,
    External(ExternalAdapter),
}

#[derive(Clone, Debug)]
pub(crate) struct ExternalAdapter {
    pub ecosystem: crate::syntax::ExternalEcosystem,
    pub entry: String,
    pub libs: Vec<String>,
    pub capabilities: std::collections::BTreeSet<&'static str>,
    pub schema: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalWorkerContract {
    pub port: String,
    pub adapter: String,
    pub ecosystem: String,
    pub entry: String,
    pub schema: String,
    pub capabilities: Vec<String>,
    pub operations: Vec<String>,
    pub libs: Vec<String>,
    pub setting: Option<serde_json::Value>,
    pub inputs: std::collections::BTreeMap<String, serde_json::Value>,
    pub outputs: std::collections::BTreeMap<String, serde_json::Value>,
    pub errors: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ExternalWorkerContract {
    pub fn sdk_manifest(&self) -> serde_json::Value {
        serde_json::json!({
            "port": self.port, "adapter": self.adapter, "ecosystem": self.ecosystem,
            "entry": self.entry, "schema": self.schema, "libs": self.libs,
            "capabilities": self.capabilities, "operations": self.operations,
            "setting": self.setting, "inputs": self.inputs, "outputs": self.outputs,
            "errors": self.errors,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ExternalOperation {
    pub adapter: usize,
    pub contract: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct PortOperation {
    pub identity: String,
    pub operation: String,
    pub input_names: Vec<String>,
    pub failures: std::collections::BTreeSet<Failure>,
    pub implementations: Vec<usize>,
}

impl TestCase {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn uses_database(&self) -> bool {
        !self.database_connections.is_empty()
    }

    pub fn database_connections(&self) -> &[String] {
        &self.database_connections
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ApiRoute {
    pub multipart: bool,
    pub method: &'static str,
    pub path: String,
    pub function: usize,
    pub inputs: Vec<String>,
    pub directory: Vec<String>,
    pub component: String,
    pub domain: String,
    pub action: String,
    pub anonymous: bool,
    pub auth: Option<usize>,
    pub permission: Option<usize>,
    pub components: Vec<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub(crate) struct ApiCommand {
    pub name: String,
    pub function: usize,
    pub inputs: Vec<String>,
    pub components: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ApiRest {
    pub path: String,
    pub model: usize,
    pub span: Span,
    pub directory: Vec<String>,
    pub component: String,
    pub domain: String,
    pub auth: Option<usize>,
    pub permissions: RestPermissions,
    pub components: Vec<String>,
    pub contract: crate::model::RestContract,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RestPermissions {
    pub read: Option<usize>,
    pub create: Option<usize>,
    pub replace: Option<usize>,
    pub delete: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Permission {
    pub key: String,
    pub component: String,
    pub domain: String,
    pub site: String,
    pub action: String,
    pub method: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Failure {
    pub ty: usize,
    pub variant: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct PackageContract {
    pub bundled: bool,
    pub domain: Option<(String, String)>,
    pub role: Option<crate::source::SourceRole>,
}

#[derive(Clone, Debug)]
pub(crate) struct Function {
    pub name: String,
    pub owner: usize,
    pub public: bool,
    pub parameters: Vec<Parameter>,
    pub outputs: Vec<Field>,
    pub clauses: Vec<Clause>,
    pub span: Span,
    pub suspends: bool,
    pub pure: bool,
    pub recovery: Option<String>,
    pub kind: crate::syntax::FunctionKind,
    pub port: Option<PortOperation>,
    pub implementation: bool,
    pub setting: Option<usize>,
    pub external: Option<ExternalOperation>,
}

#[derive(Clone, Debug)]
pub(crate) struct Clause {
    pub patterns: Vec<Domain>,
    pub bindings: Vec<Binding>,
    pub locals: Vec<Type>,
    pub outputs: Vec<usize>,
    pub body: Vec<Statement>,
    pub terminates: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Binding {
    pub slot: usize,
    pub input: usize,
    pub projection: Projection,
}

#[derive(Clone, Debug)]
pub(crate) enum Projection {
    Whole,
    NonNull,
    Payload { variant: usize, field: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Atom {
    Null,
    Any,
    Bool(bool),
    Variant(usize),
    Int(i64, i64),
    Decimal(DecimalValue, DecimalValue),
    Literal(Constant),
    Remainder(Vec<Constant>),
}

pub(crate) type Domain = Vec<Atom>;

#[derive(Clone, Debug)]
pub(crate) enum Statement {
    Assign {
        slot: usize,
        fields: Vec<usize>,
        value: Expression,
    },
    Call(Expression),
}

#[derive(Clone, Debug)]
pub(crate) struct Expression {
    pub kind: ExpressionKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub(crate) enum ExpressionKind {
    Constant(Constant),
    Local(usize),
    Field {
        value: Box<Expression>,
        index: usize,
    },
    ChoiceOptions {
        choice: usize,
    },
    ModelOperation {
        model: usize,
        operation: Box<ModelOperation>,
    },
    JobEnqueue {
        target: usize,
        arguments: Vec<Expression>,
        scheduled: bool,
    },
    Call {
        target: CallTarget,
        arguments: Vec<CallArgument>,
    },
    Fail(Box<Expression>),
    CaptureResult {
        target: CallTarget,
        arguments: Vec<CallArgument>,
        choice: usize,
        success: usize,
    },
    RunCall {
        group: Option<Box<Expression>>,
        target: CallTarget,
        arguments: Vec<CallArgument>,
    },
    AwaitTask(Box<Expression>),
    StopTask(Box<Expression>),
    Group(Box<Expression>),
    AwaitGroup(Box<Expression>),
    StopGroup(Box<Expression>),
    ParallelCall {
        target: CallTarget,
        arguments: Vec<CallArgument>,
    },
    BlockingCall {
        target: CallTarget,
        arguments: Vec<CallArgument>,
    },
    Channel {
        element: Type,
        capacity: Box<Expression>,
    },
    ChannelSend {
        channel: Box<Expression>,
        value: Box<Expression>,
    },
    ChannelReceive(Box<Expression>),
    ChannelClose(Box<Expression>),
    Intrinsic {
        operation: Intrinsic,
        handler: Option<HandlerTarget>,
        arguments: Vec<Expression>,
    },
    Collection {
        operation: CollectionOp,
        handler: Option<HandlerTarget>,
        sequence: Option<SequenceKind>,
        arguments: Vec<Expression>,
    },
    Record {
        fields: Vec<(usize, Expression)>,
    },
    Variant {
        variant: usize,
        arguments: Vec<Expression>,
    },
    List(Vec<Expression>),
    Map(Vec<(Expression, Expression)>),
    Unary {
        operator: UnaryOperator,
        value: Box<Expression>,
    },
    Binary {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>,
    },
    Promote(Box<Expression>),
    Some(Box<Expression>),
}

#[derive(Clone, Debug)]
pub(crate) enum ModelOperation {
    Create {
        values: Vec<(usize, Expression)>,
    },
    CreateMany {
        rows: Vec<Vec<(usize, Expression)>>,
    },
    Get {
        id: Box<Expression>,
    },
    First {
        query: QueryPlan,
    },
    List {
        query: QueryPlan,
    },
    Cursor {
        query: QueryPlan,
    },
    Count {
        query: QueryPlan,
    },
    Exists {
        query: QueryPlan,
    },
    Stream {
        query: QueryPlan,
    },
    Update {
        query: QueryPlan,
        values: Vec<(usize, Expression)>,
    },
    Delete {
        query: QueryPlan,
    },
    Upsert {
        key: Vec<(usize, Expression)>,
        create: Vec<(usize, Expression)>,
        update: Vec<(usize, Expression)>,
    },
    Sql {
        operation: usize,
        arguments: Vec<Expression>,
        read_only: bool,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct QueryPlan {
    pub condition: Option<ModelCondition>,
    pub order: Vec<ModelOrder>,
    /// Static indexes into ModelSchema::relations.
    pub relations: Vec<usize>,
    pub after: Option<Expression>,
    pub page: Expression,
    pub size: Expression,
}

#[derive(Clone, Debug)]
pub(crate) enum ModelCondition {
    Compare {
        field: usize,
        operator: BinaryOperator,
        value: Expression,
    },
    In {
        field: usize,
        values: Vec<Expression>,
    },
    Between {
        field: usize,
        lower: Expression,
        upper: Expression,
    },
    Text {
        field: usize,
        operation: TextCondition,
        value: Expression,
    },
    Not(Box<ModelCondition>),
    And(Box<ModelCondition>, Box<ModelCondition>),
    Or(Box<ModelCondition>, Box<ModelCondition>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextCondition {
    Contains,
    StartsWith,
    EndsWith,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModelOrder {
    pub field: usize,
    pub descending: bool,
}

impl ModelOperation {
    /// SQL outside the checker's proven projection subset remains a possible write.
    pub(crate) fn writes_database(&self) -> bool {
        matches!(
            self,
            Self::Create { .. }
                | Self::CreateMany { .. }
                | Self::Update { .. }
                | Self::Delete { .. }
                | Self::Upsert { .. }
                | Self::Sql {
                    read_only: false,
                    ..
                }
        )
    }

    pub(crate) fn expressions(&self) -> Vec<&Expression> {
        let mut values = Vec::new();
        self.visit(&mut |value| values.push(value));
        values
    }

    pub(crate) fn visit<'a>(&'a self, apply: &mut impl FnMut(&'a Expression)) {
        match self {
            Self::Create { values } => visit_write(values, apply),
            Self::CreateMany { rows } => {
                for values in rows {
                    visit_write(values, apply);
                }
            }
            Self::Get { id } => apply(id),
            Self::First { query }
            | Self::List { query }
            | Self::Cursor { query }
            | Self::Count { query }
            | Self::Exists { query }
            | Self::Stream { query }
            | Self::Delete { query } => query.visit(apply),
            Self::Update { query, values } => {
                query.visit(apply);
                visit_write(values, apply);
            }
            Self::Upsert {
                key,
                create,
                update,
            } => {
                visit_write(key, apply);
                visit_write(create, apply);
                visit_write(update, apply);
            }
            Self::Sql { arguments, .. } => arguments.iter().for_each(apply),
        }
    }

    pub(crate) fn visit_mut(&mut self, apply: &mut impl FnMut(&mut Expression)) {
        match self {
            Self::Create { values } => visit_write_mut(values, apply),
            Self::CreateMany { rows } => {
                for values in rows {
                    visit_write_mut(values, apply);
                }
            }
            Self::Get { id } => apply(id),
            Self::First { query }
            | Self::List { query }
            | Self::Cursor { query }
            | Self::Count { query }
            | Self::Exists { query }
            | Self::Stream { query }
            | Self::Delete { query } => query.visit_mut(apply),
            Self::Update { query, values } => {
                query.visit_mut(apply);
                visit_write_mut(values, apply);
            }
            Self::Upsert {
                key,
                create,
                update,
            } => {
                visit_write_mut(key, apply);
                visit_write_mut(create, apply);
                visit_write_mut(update, apply);
            }
            Self::Sql { arguments, .. } => arguments.iter_mut().for_each(apply),
        }
    }
}

impl QueryPlan {
    fn visit<'a>(&'a self, apply: &mut impl FnMut(&'a Expression)) {
        if let Some(condition) = &self.condition {
            condition.visit(apply);
        }
        if let Some(after) = &self.after {
            apply(after);
        }
        apply(&self.page);
        apply(&self.size);
    }

    fn visit_mut(&mut self, apply: &mut impl FnMut(&mut Expression)) {
        if let Some(condition) = &mut self.condition {
            condition.visit_mut(apply);
        }
        if let Some(after) = &mut self.after {
            apply(after);
        }
        apply(&mut self.page);
        apply(&mut self.size);
    }
}

impl ModelCondition {
    fn visit<'a>(&'a self, apply: &mut impl FnMut(&'a Expression)) {
        match self {
            Self::Compare { value, .. } => apply(value),
            Self::In { values, .. } => values.iter().for_each(apply),
            Self::Between { lower, upper, .. } => {
                apply(lower);
                apply(upper);
            }
            Self::Text { value, .. } => apply(value),
            Self::Not(value) => value.visit(apply),
            Self::And(left, right) | Self::Or(left, right) => {
                left.visit(apply);
                right.visit(apply);
            }
        }
    }

    fn visit_mut(&mut self, apply: &mut impl FnMut(&mut Expression)) {
        match self {
            Self::Compare { value, .. } => apply(value),
            Self::In { values, .. } => values.iter_mut().for_each(apply),
            Self::Between { lower, upper, .. } => {
                apply(lower);
                apply(upper);
            }
            Self::Text { value, .. } => apply(value),
            Self::Not(value) => value.visit_mut(apply),
            Self::And(left, right) | Self::Or(left, right) => {
                left.visit_mut(apply);
                right.visit_mut(apply);
            }
        }
    }
}

fn visit_write<'a>(values: &'a [(usize, Expression)], apply: &mut impl FnMut(&'a Expression)) {
    for (_, value) in values {
        apply(value);
    }
}

fn visit_write_mut(values: &mut [(usize, Expression)], apply: &mut impl FnMut(&mut Expression)) {
    for (_, value) in values {
        apply(value);
    }
}

impl ExpressionKind {
    pub(crate) fn callback(&self) -> Option<HandlerTarget> {
        match self {
            Self::Collection { handler, .. } | Self::Intrinsic { handler, .. } => *handler,
            _ => None,
        }
    }

    pub(crate) fn static_call(&self) -> Option<(CallTarget, &[CallArgument])> {
        match self {
            Self::Call { target, arguments }
            | Self::CaptureResult {
                target, arguments, ..
            }
            | Self::RunCall {
                target, arguments, ..
            }
            | Self::ParallelCall { target, arguments }
            | Self::BlockingCall { target, arguments } => Some((*target, arguments)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CallArgument {
    Value(Expression),
    Handler(HandlerTarget),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HandlerTarget {
    Function(usize),
    Parameter(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallTarget {
    Function(usize),
    Handler(HandlerTarget),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CollectionOp {
    Each,
    ParallelEach,
    Reduce,
    ReduceUntil,
    Filter,
    Find,
    Sum,
    Append,
    First,
    Get,
    Put,
    Remove,
    Entries,
    Length,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SequenceKind {
    List,
    Bytes,
    Stream,
    AsyncStream,
    RowStream,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Constant {
    Null,
    Bool(bool),
    Int(i64),
    Decimal(DecimalValue),
    Float(f64),
    Text(String),
}

impl Program {
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    pub fn tests(&self) -> &[TestCase] {
        &self.tests
    }

    /// Returns whether the checked program contains a production external
    /// Adapter.  The CLI uses this only for lock/preparation validation; the
    /// external Port contract remains owned by the compiler HIR.
    pub fn has_external_adapters(&self) -> bool {
        self.functions
            .iter()
            .any(|function| function.external.is_some())
    }

    pub fn external_worker_entries(&self) -> Vec<String> {
        let mut entries = self
            .adapters
            .iter()
            .filter_map(|adapter| match &adapter.implementation {
                AdapterImplementation::External(external)
                    if !adapter.fake
                        && external.ecosystem != crate::syntax::ExternalEcosystem::Command =>
                {
                    Some(external.entry.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        entries.sort();
        entries.dedup();
        entries
    }

    pub fn external_worker_contracts(&self) -> Vec<ExternalWorkerContract> {
        self.external_contracts(false)
    }

    pub fn external_command_contracts(&self) -> Vec<ExternalWorkerContract> {
        self.external_contracts(true)
    }

    fn external_contracts(&self, command: bool) -> Vec<ExternalWorkerContract> {
        let mut contracts = self
            .adapters
            .iter()
            .filter_map(|adapter| {
                let AdapterImplementation::External(external) = &adapter.implementation else {
                    return None;
                };
                if adapter.fake
                    || (external.ecosystem == crate::syntax::ExternalEcosystem::Command) != command
                {
                    return None;
                }
                Some(self.external_contract(adapter, external))
            })
            .collect::<Vec<_>>();
        contracts.sort_by(|left, right| {
            left.port
                .cmp(&right.port)
                .then(left.adapter.cmp(&right.adapter))
        });
        contracts
    }

    fn external_contract(
        &self,
        adapter: &Adapter,
        external: &ExternalAdapter,
    ) -> ExternalWorkerContract {
        let policy = crate::wire::Policy::external(external.ecosystem);
        let record = |fields: &[crate::types::Field]| {
            let fields = fields
                .iter()
                .map(|field| {
                    let schema = crate::wire::Schema::from_field(field, &self.types, policy)
                        .expect("checked external wire field");
                    (field.name.clone(), schema.worker_type())
                })
                .collect::<BTreeMap<_, _>>();
            serde_json::json!({"type": "record", "fields": fields})
        };
        let mut inputs = BTreeMap::new();
        let mut outputs = BTreeMap::new();
        let mut errors = BTreeMap::new();
        for contract in adapter.operations.keys() {
            let function = &self.functions[*contract];
            let port = function.port.as_ref().expect("checked Port operation");
            let fields = function
                .parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| crate::types::Field {
                    name: port.input_names[index].clone(),
                    ty: parameter
                        .value_type()
                        .expect("Port value parameter")
                        .clone(),
                    private: false,
                    bounds: Vec::new(),
                })
                .collect::<Vec<_>>();
            inputs.insert(port.operation.clone(), record(&fields));
            outputs.insert(port.operation.clone(), record(&function.outputs));
            for failure in &port.failures {
                let crate::types::Shape::Choice(variants) = &self.types[failure.ty].shape else {
                    unreachable!()
                };
                let variant = &variants[failure.variant];
                let identity = format!("{}.{}", self.types[failure.ty].name, variant.name);
                errors.insert(identity, record(&variant.fields));
            }
        }
        ExternalWorkerContract {
            port: adapter.identity.clone(),
            adapter: adapter.name.clone(),
            ecosystem: external.ecosystem.keyword().into(),
            entry: external.entry.clone(),
            schema: external.schema.clone(),
            capabilities: external
                .capabilities
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            operations: inputs.keys().cloned().collect(),
            libs: external.libs.clone(),
            setting: adapter.setting.map(|ty| {
                crate::wire::Schema::build(
                    &crate::types::Type::Named(ty),
                    &self.types,
                    crate::wire::Policy::SettingInput,
                )
                .expect("checked external setting")
                .worker_type()
            }),
            inputs,
            outputs,
            errors,
        }
    }

    /// Exact Lib requests declared by external Adapters. The compiler keeps
    /// them opaque; the CLI owns ecosystem parsing and resolution.
    pub fn external_lib_requests(&self) -> Vec<String> {
        let mut requests = self
            .adapters
            .iter()
            .filter_map(|adapter| match &adapter.implementation {
                AdapterImplementation::External(external) => Some(external.libs.clone()),
                AdapterImplementation::Dever => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        requests.sort();
        requests.dedup();
        requests
    }

    pub(crate) fn entry(&self, name: &str) -> Result<usize, String> {
        let (id, function) = self
            .functions
            .iter()
            .enumerate()
            .find(|(_, function)| function.name == name && function.parameters.is_empty())
            .ok_or_else(|| format!("entry '{name}' must name a zero-input function"))?;
        if !function.public && name != "main.main" {
            return Err(format!("entry '{name}' must be public"));
        }
        if function
            .outputs
            .iter()
            .any(|field| !field.ty.observable(&self.types))
        {
            return Err(format!(
                "entry '{name}' cannot render Secret or private Model data"
            ));
        }
        Ok(id)
    }
}
