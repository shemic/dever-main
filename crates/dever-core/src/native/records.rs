use std::fmt::Write;

use crate::types::{DefinitionKind, Shape, Type};

use super::{Emitter, rust_string, rust_type};

impl Emitter<'_> {
    pub(super) fn types(&self, output: &mut String) {
        for (id, definition) in self.program.types.iter().enumerate() {
            if definition.kind == DefinitionKind::ModelId {
                writeln!(
                    output,
                    "#[derive(Clone, Debug, Eq, Hash, PartialEq)]\nstruct T{id}(i64);\nimpl Render for T{id} {{ fn render_to(&self, output: &mut String) {{ self.0.render_to(output); }} }}"
                )
                .expect("string formatting");
                continue;
            }
            let equality = if Type::Named(id).comparable(&self.program.types) {
                ", PartialEq"
            } else {
                ""
            };
            let key = matches!(&definition.shape, Shape::Choice(variants) if variants.iter().all(|variant| variant.fields.is_empty()));
            writeln!(
                output,
                "#[derive(Clone, Debug{equality}{})]",
                if key { ", Eq, Hash" } else { "" }
            )
            .expect("string formatting");
            match &definition.shape {
                Shape::Record(fields) => {
                    writeln!(output, "struct T{id} {{").expect("string formatting");
                    for (index, field) in fields.iter().enumerate() {
                        writeln!(output, "f{index}: {},", rust_type(&field.ty))
                            .expect("string formatting");
                    }
                    output.push_str("}\n");
                    if !Type::Named(id).observable(&self.program.types) {
                        continue;
                    }
                    writeln!(
                        output,
                        "impl Render for T{id} {{ fn render_to(&self, output: &mut String) {{"
                    )
                    .expect("string formatting");
                    writeln!(
                        output,
                        "output.push_str({}); output.push_str(\" {{ \");",
                        rust_string(&definition.name)
                    )
                    .expect("string formatting");
                    for (index, field) in fields.iter().enumerate() {
                        if index != 0 {
                            output.push_str("output.push_str(\", \");\n");
                        }
                        writeln!(
                            output,
                            "output.push_str({}); output.push_str(\" = \"); self.f{index}.render_to(output);",
                            rust_string(&field.name)
                        )
                        .expect("string formatting");
                    }
                    output.push_str("output.push_str(\" }\");\n} }\n");
                }
                Shape::Choice(variants) => {
                    writeln!(output, "enum T{id} {{").expect("string formatting");
                    for (index, variant) in variants.iter().enumerate() {
                        let payload = if variant.fields.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "({})",
                                variant
                                    .fields
                                    .iter()
                                    .map(|field| rust_type(&field.ty))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        writeln!(output, "V{index}{payload},").expect("string formatting");
                    }
                    output.push_str("}\n");
                    if !Type::Named(id).observable(&self.program.types) {
                        continue;
                    }
                    writeln!(
                        output,
                        "impl Render for T{id} {{ fn render_to(&self, output: &mut String) {{ match self {{"
                    )
                    .expect("string formatting");
                    for (index, variant) in variants.iter().enumerate() {
                        let name = rust_string(&format!("{}.{}", definition.name, variant.name));
                        if variant.fields.is_empty() {
                            writeln!(output, "Self::V{index} => output.push_str({name}),")
                                .expect("string formatting");
                        } else {
                            let bindings = (0..variant.fields.len())
                                .map(|index| format!("field{index}"))
                                .collect::<Vec<_>>();
                            write!(
                                output,
                                "Self::V{index}({}) => {{ output.push_str({name}); output.push('(');",
                                bindings.join(", ")
                            )
                            .expect("string formatting");
                            for (field, binding) in bindings.iter().enumerate() {
                                if field != 0 {
                                    output.push_str("output.push_str(\", \");");
                                }
                                write!(output, "{binding}.render_to(output);")
                                    .expect("string formatting");
                            }
                            output.push_str("output.push(')'); },\n");
                        }
                    }
                    output.push_str("} } }\n");
                }
            }
        }
    }

    pub(super) fn errors(&self, output: &mut String) {
        let failures = self
            .program
            .failures
            .iter()
            .flat_map(|failures| failures.iter())
            .copied()
            .chain(crate::capture::database_errors(&self.program.types))
            .collect::<std::collections::BTreeSet<_>>();
        output.push_str("#[derive(Debug)]\nenum AppErrorKind {\n");
        for failure in &failures {
            let Shape::Choice(variants) = &self.program.types[failure.ty].shape else {
                unreachable!("failure type is a choice")
            };
            let variant = &variants[failure.variant];
            if variant.fields.is_empty() {
                writeln!(output, "E{}V{},", failure.ty, failure.variant)
                    .expect("string formatting");
            } else {
                writeln!(
                    output,
                    "E{}V{}({}),",
                    failure.ty,
                    failure.variant,
                    variant
                        .fields
                        .iter()
                        .map(|field| rust_type(&field.ty))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .expect("string formatting");
            }
        }
        output.push_str("Fault(String),\n}\n#[derive(Debug)]\nstruct AppError { kind: AppErrorKind, locations: Vec<&'static str>, causes: Vec<String> }\n");
        output.push_str("impl AppError {\nfn business(kind: AppErrorKind, location: &'static str) -> Self { Self { kind, locations: vec![location], causes: Vec::new() } }\nfn fault(location: &'static str, error: impl std::fmt::Display) -> Self { Self { kind: AppErrorKind::Fault(error.to_string()), locations: vec![location], causes: Vec::new() } }\nfn at(mut self, location: &'static str) -> Self { self.locations.push(location); self }\nfn cause(mut self, error: impl std::fmt::Display) -> Self { self.causes.push(error.to_string()); self }\n}\n");
        output.push_str("impl From<String> for AppError { fn from(message: String) -> Self { Self { kind: AppErrorKind::Fault(message), locations: Vec::new(), causes: Vec::new() } } }\n");
        output.push_str("impl AppError { fn database(location: &'static str, error: dever_runtime::orm::Error) -> Self { let kind = match error.kind() {\n");
        for failure in crate::capture::database_errors(&self.program.types) {
            let Shape::Choice(variants) = &self.program.types[failure.ty].shape else {
                unreachable!("database error choice")
            };
            writeln!(
                output,
                "dever_runtime::orm::ErrorKind::{} => AppErrorKind::E{}V{}(error.to_string()),",
                variants[failure.variant].name, failure.ty, failure.variant
            )
            .expect("string formatting");
        }
        output.push_str("}; Self::business(kind, location) } }\n");
        output.push_str("impl std::fmt::Display for AppError { fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { for location in self.locations.iter().rev() { write!(output, \"{location}: \")?; } match &self.kind {\n");
        for failure in &failures {
            let Shape::Choice(variants) = &self.program.types[failure.ty].shape else {
                unreachable!("failure type is a choice")
            };
            let variant = &variants[failure.variant];
            let name = rust_string(&format!(
                "{}.{}",
                self.program.types[failure.ty].name, variant.name
            ));
            if variant.fields.is_empty() {
                writeln!(
                    output,
                    "AppErrorKind::E{}V{} => output.write_str({name}),",
                    failure.ty, failure.variant
                )
                .expect("string formatting");
            } else {
                let bindings = (0..variant.fields.len())
                    .map(|index| format!("field{index}"))
                    .collect::<Vec<_>>();
                write!(
                    output,
                    "AppErrorKind::E{}V{}({}) => {{ let mut rendered = String::from({name}); rendered.push('(');",
                    failure.ty,
                    failure.variant,
                    bindings.join(", ")
                )
                .expect("string formatting");
                for (index, binding) in bindings.iter().enumerate() {
                    if index != 0 {
                        output.push_str("rendered.push_str(\", \");");
                    }
                    write!(output, "{binding}.render_to(&mut rendered);")
                        .expect("string formatting");
                }
                output.push_str("rendered.push(')'); output.write_str(&rendered) },\n");
            }
        }
        output.push_str("AppErrorKind::Fault(message) => output.write_str(message),\n}?; for cause in &self.causes { write!(output, \"; caused by: {cause}\")?; } Ok(()) } }\nimpl std::error::Error for AppError {}\n");
    }
}
