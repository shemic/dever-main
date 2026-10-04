use crate::diagnostic::{Diagnostic, Label};
use crate::hir::Function;
use crate::source::Span;
use crate::syntax::{
    BinaryOperator, Declaration, Field, Input, InputKind, Package, TypeRef, TypeShape, Variant,
};
use crate::types::{Definition, Parameter, Type};

use super::model::{
    DocumentContract, DocumentValue, DocumentedList, FunctionCode, FunctionDocumentation,
    ModelCode, ModelDocumentation, SectionCode, SectionDocumentation, TypeCode, TypeDocumentation,
    TypeEntries,
};

pub(super) fn contract(
    contract: &DocumentContract,
    package: &Package,
    owner: usize,
    definitions: &[Definition],
    functions: &[Function],
) -> Vec<Diagnostic> {
    let package_name = path_name(&package.name);
    let mut errors = Vec::new();
    compare_value(
        "M006",
        "documented package name",
        &contract.package.name,
        &package_name,
        package.name[0].span,
        &mut errors,
    );

    let public_types = public_names(package, |declaration| match declaration {
        Declaration::Type(ty)
            if definitions.iter().any(|definition| {
                definition.owner == owner
                    && definition.public
                    && definition.name.rsplit('.').next() == Some(ty.name.text.as_str())
            }) =>
        {
            Some(&ty.name.text)
        }
        _ => None,
    });
    let public_functions = public_names(package, |declaration| match declaration {
        Declaration::Function(function)
            if functions.iter().any(|checked| {
                checked.owner == owner
                    && checked.public
                    && checked.name.rsplit('.').next() == Some(function.name.text.as_str())
            }) =>
        {
            Some(&function.name.text)
        }
        _ => None,
    });
    compare_list(
        "M006",
        "documented public types",
        &contract.package.public_types,
        &public_types,
        package.span,
        &mut errors,
    );
    compare_list(
        "M006",
        "documented public methods",
        &contract.package.public_functions,
        &public_functions,
        package.span,
        &mut errors,
    );

    let mut usages = Vec::new();
    for exposed in &public_functions {
        for section in &contract.sections {
            let SectionCode::Function(code) = &section.code else {
                continue;
            };
            if &code.name == exposed {
                let prefix = package
                    .layout
                    .app_prefix()
                    .unwrap_or_else(|| package_name.clone());
                usages.push(format!(
                    "{prefix}.{}({})",
                    code.name,
                    code.input_names
                        .iter()
                        .map(|input| input.text.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }
    compare_list(
        "M006",
        "documented public method usage",
        &contract.package.usages,
        &usages,
        package.span,
        &mut errors,
    );

    for section in &contract.sections {
        match (&section.documentation, &section.code) {
            (SectionDocumentation::Type(documentation), SectionCode::Type(code)) => {
                validate_type_section(documentation, code, package, &mut errors);
            }
            (SectionDocumentation::Function(documentation), SectionCode::Function(code)) => {
                validate_function_section(
                    documentation,
                    code,
                    package,
                    owner,
                    definitions,
                    functions,
                    &mut errors,
                );
            }
            (SectionDocumentation::Model(documentation), SectionCode::Model(code)) => {
                validate_model_section(documentation, code, package, &mut errors);
            }
            _ => unreachable!("section documentation follows its declaration kind"),
        }
    }
    errors
}

fn validate_model_section(
    documentation: &ModelDocumentation,
    code: &ModelCode,
    package: &Package,
    errors: &mut Vec<Diagnostic>,
) {
    compare_value(
        "M006",
        "documented Model declaration",
        &documentation.declaration,
        &code.declaration,
        code.span,
        errors,
    );
    let Some((inputs, outputs)) = &documentation.signature else {
        return;
    };
    let sql = package
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::ModelSql(sql) if sql.span == code.span => Some(sql),
            _ => None,
        })
        .expect("SQL section maps to its parsed declaration");
    compare_list(
        "M007",
        "documented SQL inputs",
        inputs,
        &sql.inputs.iter().map(field_label).collect::<Vec<_>>(),
        code.span,
        errors,
    );
    compare_list(
        "M008",
        "documented SQL outputs",
        outputs,
        &sql.outputs.iter().map(field_label).collect::<Vec<_>>(),
        code.span,
        errors,
    );
}

fn public_names(package: &Package, name: impl Fn(&Declaration) -> Option<&String>) -> Vec<String> {
    let mut names = Vec::new();
    for name in package.declarations.iter().filter_map(name) {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names
}

fn validate_type_section(
    documentation: &TypeDocumentation,
    code: &TypeCode,
    package: &Package,
    errors: &mut Vec<Diagnostic>,
) {
    compare_value(
        "M006",
        "documented type name",
        &documentation.name,
        &code.name,
        code.span,
        errors,
    );
    let declaration = package
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Type(ty) if ty.span.start == code.span.start => Some(ty),
            _ => None,
        })
        .expect("document type section maps to a parsed type");
    let expected = match &declaration.shape {
        TypeShape::Record(fields) => fields.iter().map(field_label).collect::<Vec<_>>(),
        TypeShape::Choice(variants) => variants.iter().map(variant_label).collect::<Vec<_>>(),
    };
    compare_list(
        "M008",
        match code.entries {
            TypeEntries::Fields => "documented type fields",
            TypeEntries::Variants => "documented choice variants",
        },
        &documentation.entries,
        &expected,
        code.span,
        errors,
    );
}

fn validate_function_section(
    documentation: &FunctionDocumentation,
    code: &FunctionCode,
    package: &Package,
    owner: usize,
    definitions: &[Definition],
    functions: &[Function],
    errors: &mut Vec<Diagnostic>,
) {
    compare_value(
        "M006",
        "documented function name",
        &documentation.name,
        &code.name,
        code.span,
        errors,
    );
    validate_clause_input_names(code, errors);
    if let (Some(documented), Some(expected)) = (&documentation.fails, &code.fails) {
        compare_value(
            "M008",
            "documented Port failure choice",
            documented,
            expected,
            code.span,
            errors,
        );
    }

    let function = functions
        .iter()
        .find(|function| {
            function.owner == owner
                && function.span.start == code.span.start
                && function.parameters.len() == code.arity
        })
        .expect("document function section maps to a checked function");
    let clause = package
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Function(function) if function.span.start == code.span.start => {
                Some(function)
            }
            _ => None,
        })
        .expect("document function section maps to a parsed clause");
    let inputs = clause
        .inputs
        .iter()
        .zip(&function.parameters)
        .map(|(source, parameter)| parameter_label(source, parameter, definitions, owner))
        .collect::<Vec<_>>();
    let outputs = function
        .outputs
        .iter()
        .map(|field| {
            format!(
                "{}: {}",
                field.name,
                type_label(&field.ty, definitions, owner)
            )
        })
        .collect::<Vec<_>>();
    compare_list(
        "M007",
        "documented function inputs",
        &documentation.inputs,
        &inputs,
        code.span,
        errors,
    );
    compare_list(
        "M008",
        "documented function outputs",
        &documentation.outputs,
        &outputs,
        code.span,
        errors,
    );
}

fn validate_clause_input_names(code: &FunctionCode, errors: &mut Vec<Diagnostic>) {
    for clause_inputs in &code.clause_inputs[1..] {
        for (first, candidate) in code.input_names.iter().zip(clause_inputs) {
            if first.text != candidate.text {
                errors.push(mismatch(
                    "M007",
                    format!(
                        "all clauses documented by one section must use input name '{}'; found '{}'",
                        first.text, candidate.text
                    ),
                    candidate.span,
                    first.span,
                ));
            }
        }
    }
}

fn parameter_label(
    source: &Input,
    parameter: &Parameter,
    definitions: &[Definition],
    owner: usize,
) -> String {
    match (parameter, &source.kind) {
        (Parameter::Value(ty), InputKind::Value(_)) => {
            format!(
                "{}: {}",
                source.name.text,
                type_label(ty, definitions, owner)
            )
        }
        (Parameter::Handler(signature), InputKind::Handler(source_signature)) => {
            let inputs = source_signature
                .inputs
                .iter()
                .zip(&signature.parameters)
                .map(|(field, ty)| {
                    format!(
                        "{}: {}",
                        field.name.text,
                        type_label(ty, definitions, owner)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let outputs = signature
                .outputs
                .iter()
                .map(|field| {
                    format!(
                        "{}: {}",
                        field.name,
                        type_label(&field.ty, definitions, owner)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}: handler({inputs}) ({outputs})", source.name.text)
        }
        _ => unreachable!("checked parameter preserves its source input kind"),
    }
}

fn type_label(ty: &Type, definitions: &[Definition], owner: usize) -> String {
    match ty {
        Type::Named(id) => {
            let definition = &definitions[*id];
            if definition.owner == owner {
                definition
                    .name
                    .rsplit_once('.')
                    .map_or_else(|| definition.name.clone(), |(_, name)| name.to_owned())
            } else {
                definition.name.clone()
            }
        }
        Type::Nullable(base) => format!("{}?", type_label(base, definitions, owner)),
        Type::List(element) => format!("List<{}>", type_label(element, definitions, owner)),
        Type::Map(key, value) => format!(
            "Map<{}, {}>",
            type_label(key, definitions, owner),
            type_label(value, definitions, owner)
        ),
        Type::MapEntry(key, value) => format!(
            "MapEntry<{}, {}>",
            type_label(key, definitions, owner),
            type_label(value, definitions, owner)
        ),
        Type::Stream(element) => format!("Stream<{}>", type_label(element, definitions, owner)),
        Type::AsyncStream(element) => {
            format!("AsyncStream<{}>", type_label(element, definitions, owner))
        }
        Type::RowStream(element) => {
            format!("AsyncStream<{}>", type_label(element, definitions, owner))
        }
        Type::Related(element) => {
            format!("Related<{}>", type_label(element, definitions, owner))
        }
        Type::Channel(element) => {
            format!("Channel<{}>", type_label(element, definitions, owner))
        }
        Type::Task(_) | Type::Group => unreachable!("affine types cannot enter Markdown APIs"),
        Type::File => "dever.system.File".into(),
        Type::Socket => "dever.system.Socket".into(),
        Type::Listener => "dever.system.Listener".into(),
        Type::HttpReply => "dever.system.HttpReply".into(),
        Type::WebSocket => "dever.system.WebSocket".into(),
        Type::ClientTls => "dever.system.ClientTls".into(),
        Type::ServerTls => "dever.system.ServerTls".into(),
        Type::HttpClient => "dever.system.HttpClient".into(),
        Type::Outputs(_) => "named outputs".into(),
        _ => format!("{ty:?}"),
    }
}

fn field_label(field: &Field) -> String {
    format!(
        "{}{}: {}{}",
        if field.private { "private " } else { "" },
        field.name.text,
        type_ref_label(&field.ty),
        bounds_label(&field.bounds)
    )
}

fn variant_label(variant: &Variant) -> String {
    let payload = if variant.payload.is_empty() {
        String::new()
    } else {
        format!(
            "({})",
            variant
                .payload
                .iter()
                .map(field_label)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "{}{}{}",
        if variant.error { "error " } else { "" },
        variant.name.text,
        payload
    )
}

fn type_ref_label(ty: &TypeRef) -> String {
    let mut label = path_name(&ty.name);
    if !ty.parameters.is_empty() {
        label.push('(');
        label.push_str(
            &ty.parameters
                .iter()
                .map(|parameter| parameter.value.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        label.push(')');
    }
    if !ty.arguments.is_empty() {
        label.push('<');
        label.push_str(
            &ty.arguments
                .iter()
                .map(type_ref_label)
                .collect::<Vec<_>>()
                .join(", "),
        );
        label.push('>');
    }
    if ty.nullable {
        label.push('?');
    }
    label
}

fn bounds_label(bounds: &[crate::syntax::Bound]) -> String {
    bounds
        .iter()
        .enumerate()
        .map(|(index, bound)| {
            format!(
                "{}{} {}",
                if index == 0 { " " } else { " and " },
                operator_label(bound.comparison),
                bound.number
            )
        })
        .collect()
}

fn operator_label(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "!=",
        BinaryOperator::Less => "<",
        BinaryOperator::LessEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterEqual => ">=",
        _ => unreachable!("parser permits only comparisons in bounds"),
    }
}

fn path_name(path: &crate::syntax::Path) -> String {
    path.iter()
        .map(|name| name.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn compare_value(
    code: &'static str,
    subject: &str,
    actual: &DocumentValue,
    expected: &str,
    related: Span,
    errors: &mut Vec<Diagnostic>,
) {
    if actual.text != expected {
        errors.push(mismatch(
            code,
            format!("{subject} must be `{expected}`; found `{}`", actual.text),
            actual.span,
            related,
        ));
    }
}

fn compare_list(
    code: &'static str,
    subject: &str,
    actual: &DocumentedList,
    expected: &[String],
    related: Span,
    errors: &mut Vec<Diagnostic>,
) {
    let actual_values = actual
        .values
        .iter()
        .map(|value| value.text.as_str())
        .collect::<Vec<_>>();
    let expected_values = expected.iter().map(String::as_str).collect::<Vec<_>>();
    if actual_values == expected_values {
        return;
    }
    let expected = if expected.is_empty() {
        "无".into()
    } else {
        expected
            .iter()
            .map(|value| format!("`{value}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let primary = actual
        .values
        .iter()
        .zip(expected_values.iter())
        .find_map(|(actual, expected)| (actual.text != *expected).then_some(actual.span))
        .or_else(|| {
            actual
                .values
                .get(expected_values.len())
                .map(|value| value.span)
        })
        .unwrap_or(actual.span);
    errors.push(mismatch(
        code,
        format!("{subject} must exactly match the code in order; expected {expected}"),
        primary,
        related,
    ));
}

fn mismatch(
    code: &'static str,
    message: impl Into<String>,
    primary: Span,
    related: Span,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(code, message, primary);
    diagnostic.related.push(Label {
        span: related,
        message: "corresponding Dever declaration".into(),
    });
    diagnostic
}
