use crate::source::Span;

pub(super) struct DocumentContract {
    pub package: PackageDocumentation,
    pub sections: Vec<DeclarationSection>,
}

pub(super) struct PackageDocumentation {
    pub name: DocumentValue,
    pub public_types: DocumentedList,
    pub public_functions: DocumentedList,
    pub usages: DocumentedList,
}

pub(super) struct DeclarationSection {
    pub documentation: SectionDocumentation,
    pub code: SectionCode,
}

pub(super) enum SectionDocumentation {
    Type(TypeDocumentation),
    Function(FunctionDocumentation),
    Model(ModelDocumentation),
}

pub(super) struct ModelDocumentation {
    pub declaration: DocumentValue,
    pub signature: Option<(DocumentedList, DocumentedList)>,
}

pub(super) struct TypeDocumentation {
    pub name: DocumentValue,
    pub entries: DocumentedList,
}

pub(super) struct FunctionDocumentation {
    pub name: DocumentValue,
    pub inputs: DocumentedList,
    pub outputs: DocumentedList,
    pub fails: Option<DocumentValue>,
}

pub(super) enum SectionCode {
    Type(TypeCode),
    Function(FunctionCode),
    Model(ModelCode),
}

pub(super) struct ModelCode {
    pub declaration: String,
    pub span: Span,
    pub sql: bool,
}

pub(super) struct TypeCode {
    pub name: String,
    pub entries: TypeEntries,
    pub span: Span,
}

pub(super) struct FunctionCode {
    pub name: String,
    pub arity: usize,
    pub span: Span,
    pub input_names: Vec<DocumentValue>,
    pub clause_inputs: Vec<Vec<DocumentValue>>,
    pub fails: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) enum TypeEntries {
    Fields,
    Variants,
}

pub(super) struct DocumentValue {
    pub text: String,
    pub span: Span,
}

pub(super) struct DocumentedList {
    pub values: Vec<DocumentValue>,
    pub span: Span,
}
