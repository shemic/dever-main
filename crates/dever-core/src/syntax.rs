use crate::source::Span;

pub(crate) const MAX_SYNTAX_DEPTH: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

pub type Path = Vec<Name>;

#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub name: Path,
    pub(crate) layout: crate::source::SourceLayout,
    pub declarations: Vec<Declaration>,
    pub comments: Vec<Comment>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Declaration {
    Api(ApiBinding),
    Rest(RestDeclaration),
    External(ExternalAdapter),
    Type(TypeDeclaration),
    Function(FunctionClause),
    Database(DatabaseBinding),
    Schedule(Schedule),
    ModelIndex(ModelIndex),
    Relation(Relation),
    Seed(Seed),
    Migration(Migration),
    ModelSql(ModelSql),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalEcosystem {
    Exec,
    Command,
    Pip,
    Npm,
    Go,
}

impl ExternalEcosystem {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Exec => "exec",
            Self::Command => "command",
            Self::Pip => "pip",
            Self::Npm => "npm",
            Self::Go => "go",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExternalAdapter {
    pub ecosystem: ExternalEcosystem,
    pub entry: String,
    pub entry_span: Span,
    pub libs: Vec<Name>,
    pub capabilities: Vec<Name>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ApiKind {
    Get,
    Post,
    Put,
    Delete,
    Command,
}

impl ApiKind {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Delete => "delete",
            Self::Command => "cmd",
        }
    }

    pub(crate) fn method(self) -> Option<&'static str> {
        match self {
            Self::Get => Some("GET"),
            Self::Post => Some("POST"),
            Self::Put => Some("PUT"),
            Self::Delete => Some("DELETE"),
            Self::Command => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ApiBinding {
    pub anonymous: bool,
    pub kind: ApiKind,
    pub action: Name,
    pub target: Path,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestDeclaration {
    pub model: Option<Path>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DatabaseBinding {
    pub name: Name,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub target: Name,
    pub cron: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelIndex {
    pub unique: bool,
    pub fields: Vec<Name>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    pub name: Name,
    pub field: Path,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seed {
    pub rows: Vec<Vec<RecordField>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Migration {
    pub name: Name,
    pub operations: Vec<MigrationOperation>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MigrationOperation {
    Drop(Name),
    Sql(MigrationSql),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationPhase {
    Before,
    After,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MigrationSql {
    pub phase: MigrationPhase,
    pub sqlite: SqlText,
    pub postgres: SqlText,
    pub parameters: Vec<Expression>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelSql {
    pub name: Name,
    pub inputs: Vec<Field>,
    pub outputs: Vec<Field>,
    pub sqlite: SqlText,
    pub postgres: SqlText,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SqlText {
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDeclaration {
    pub public: bool,
    pub setting: bool,
    pub global: bool,
    pub name: Name,
    pub shape: TypeShape,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeShape {
    Record(Vec<Field>),
    Choice(Vec<Variant>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: Name,
    pub ty: TypeRef,
    pub private: bool,
    pub bounds: Vec<Bound>,
    pub storage: FieldStorage,
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldStorage {
    pub generated: bool,
    pub default: Option<Expression>,
    pub index: bool,
    pub unique: bool,
    pub from: Option<Name>,
    pub owner: Option<Expression>,
    pub create: Option<Expression>,
    pub replace: Option<Expression>,
    pub search: Option<Expression>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: Name,
    pub error: bool,
    pub payload: Vec<Field>,
    pub label: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeRef {
    pub name: Path,
    pub arguments: Vec<TypeRef>,
    pub parameters: Vec<NumberParameter>,
    pub nullable: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NumberParameter {
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionClause {
    pub public: bool,
    pub name: Name,
    pub kind: FunctionKind,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Field>,
    pub pure: bool,
    pub recovery: Option<String>,
    pub fails: Option<TypeRef>,
    pub bodyless: bool,
    pub body: Vec<Statement>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FunctionKind {
    Ordinary,
    Transaction,
    Job { attempts: u32, timeout_ms: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub name: Name,
    pub kind: InputKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InputKind {
    Value(Pattern),
    Handler(HandlerSignature),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandlerSignature {
    pub inputs: Vec<Field>,
    pub outputs: Vec<Field>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    Typed {
        ty: TypeRef,
        bounds: Vec<Bound>,
    },
    Variant {
        name: Path,
        arguments: Vec<TypeRef>,
        bindings: Vec<Name>,
        span: Span,
    },
    Literal {
        value: Literal,
        span: Span,
    },
    Other(Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub comparison: BinaryOperator,
    pub number: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatementKind {
    Assign { target: Path, value: Expression },
    Call(Expression),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
    pub(crate) depth: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    Literal(Literal),
    Name(Path),
    Group(Box<Expression>),
    Field {
        value: Box<Expression>,
        name: Name,
    },
    Call {
        function: Box<Expression>,
        arguments: Vec<Expression>,
    },
    Fail(Box<Expression>),
    CaptureResult(Box<Expression>),
    Contextual(ContextualExpression),
    Record {
        name: Path,
        fields: Vec<RecordField>,
    },
    List(Vec<Expression>),
    Map(Vec<MapEntry>),
    Unary {
        operator: UnaryOperator,
        value: Box<Expression>,
    },
    Binary {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContextualExpression {
    Channel {
        element: TypeRef,
        capacity: Box<Expression>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordField {
    pub name: Name,
    pub value: Expression,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapEntry {
    pub key: Expression,
    pub value: Expression,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Bool(bool),
    Null,
    Number(String),
    Text(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnaryOperator {
    Negate,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    IntegerDivide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}

impl BinaryOperator {
    pub fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Equal
            | Self::NotEqual
            | Self::Less
            | Self::LessEqual
            | Self::Greater
            | Self::GreaterEqual => 3,
            Self::Add | Self::Subtract => 4,
            Self::Multiply | Self::Divide | Self::IntegerDivide | Self::Remainder => 5,
        }
    }
}
