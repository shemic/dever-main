use std::collections::BTreeMap;
use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use crate::diagnostic::{Diagnostic, Label};
use crate::source::{SourceFile, Span};
use crate::syntax::{Declaration, Package, TypeShape};

use super::metadata;
use super::model::{
    DeclarationSection, DocumentContract, DocumentValue, FunctionCode, ModelCode, SectionCode,
    SectionDocumentation, TypeCode, TypeEntries,
};
use crate::markdown::{ProgramBlock, declaration_span, normalize_lone_carriage_returns};

pub(super) fn contract(
    source: &SourceFile,
    package: &Package,
    blocks: &[ProgramBlock],
) -> Result<DocumentContract, Vec<Diagnostic>> {
    let headings = headings(source);
    let h1 = headings
        .iter()
        .enumerate()
        .filter(|(_, heading)| heading.level == HeadingLevel::H1)
        .collect::<Vec<_>>();
    if h1.len() != 1 {
        let primary = h1
            .get(1)
            .or_else(|| h1.first())
            .map_or_else(|| source.span(0, 0), |(_, heading)| heading.span);
        return Err(vec![Diagnostic::error(
            "M004",
            "a .dever.md source must contain exactly one top-level package heading ('#')",
            primary,
        )]);
    }
    if h1[0].0 != 0 {
        return Err(vec![Diagnostic::error(
            "M004",
            "the package heading ('#') must precede every declaration heading ('##')",
            headings[0].span,
        )]);
    }
    if h1[0].1.title.trim().is_empty() {
        return Err(vec![Diagnostic::error(
            "M004",
            "the package heading must have a natural-language name",
            h1[0].1.span,
        )]);
    }

    let section_blocks = match_blocks(source, &headings, blocks)?;
    let package_section_end = headings
        .get(1)
        .map_or(source.text().len(), |heading| heading.range.start);
    let package_documentation = metadata::package(
        source,
        headings[0].range.end..package_section_end,
        headings[0].span,
    )?;

    let mut sections = Vec::new();
    let mut documented_declarations = BTreeMap::new();
    for (heading_index, heading) in headings.iter().enumerate().skip(1) {
        if heading.title.trim().is_empty() {
            return Err(vec![Diagnostic::error(
                "M005",
                "each declaration heading must have a natural-language name",
                heading.span,
            )]);
        }
        let block = &blocks[section_blocks[heading_index - 1]];
        let declarations = declarations_in(package, block);
        let code = section_code(&declarations, heading.span)?;
        reject_duplicate_section(&code, heading.span, &mut documented_declarations)?;
        let documentation = match &code {
            SectionCode::Type(code) => SectionDocumentation::Type(metadata::type_declaration(
                source,
                heading.range.end..block.outer.start,
                heading.span,
                code.entries,
            )?),
            SectionCode::Function(code) => SectionDocumentation::Function(metadata::function(
                source,
                heading.range.end..block.outer.start,
                heading.span,
                code.fails.is_some(),
            )?),
            SectionCode::Model(code) => SectionDocumentation::Model(metadata::model_declaration(
                source,
                heading.range.end..block.outer.start,
                heading.span,
                code.sql,
            )?),
        };
        sections.push(DeclarationSection {
            documentation,
            code,
        });
    }

    Ok(DocumentContract {
        package: package_documentation,
        sections,
    })
}

fn match_blocks(
    source: &SourceFile,
    headings: &[Heading],
    blocks: &[ProgramBlock],
) -> Result<Vec<usize>, Vec<Diagnostic>> {
    let mut section_blocks = Vec::new();
    let mut assigned = vec![false; blocks.len()];
    for (index, heading) in headings.iter().enumerate() {
        let end = headings
            .get(index + 1)
            .map_or(source.text().len(), |next| next.range.start);
        let matched = blocks
            .iter()
            .enumerate()
            .filter_map(|(block_index, block)| {
                (heading.range.end <= block.outer.start && block.outer.start < end)
                    .then_some(block_index)
            })
            .collect::<Vec<_>>();
        let expected = usize::from(heading.level != HeadingLevel::H1);
        if matched.len() != expected {
            return Err(vec![Diagnostic::error(
                if heading.level == HeadingLevel::H1 {
                    "M004"
                } else {
                    "M005"
                },
                format!(
                    "each '{}' section must contain {expected} top-level Dever code blocks; found {}",
                    heading_marker(heading.level),
                    matched.len()
                ),
                heading.span,
            )]);
        }
        if let Some(&block) = matched.first() {
            assigned[block] = true;
            section_blocks.push(block);
        }
    }
    if let Some((index, _)) = assigned.iter().enumerate().find(|(_, used)| !**used) {
        return Err(vec![Diagnostic::error(
            "M005",
            "every top-level Dever code block must belong to one '##' declaration section",
            source.span(blocks[index].outer.start, blocks[index].outer.end),
        )]);
    }
    Ok(section_blocks)
}

fn reject_duplicate_section(
    code: &SectionCode,
    heading: Span,
    documented: &mut BTreeMap<String, Span>,
) -> Result<(), Vec<Diagnostic>> {
    let (key, code_span) = match code {
        SectionCode::Type(code) => (format!("type:{}", code.name), code.span),
        SectionCode::Function(code) => {
            (format!("function:{}/{}", code.name, code.arity), code.span)
        }
        SectionCode::Model(code) => (format!("model:{}", code.declaration), code.span),
    };
    let Some(first) = documented.insert(key, code_span) else {
        return Ok(());
    };
    let mut error = Diagnostic::error(
        "M005",
        "one declaration or all clauses of one logical function must share exactly one '##' section",
        heading,
    );
    error.related.extend([
        Label {
            span: first,
            message: "the declaration was already documented here".into(),
        },
        Label {
            span: code_span,
            message: "duplicate declaration section".into(),
        },
    ]);
    Err(vec![error])
}

fn section_code(
    declarations: &[&Declaration],
    heading: Span,
) -> Result<SectionCode, Vec<Diagnostic>> {
    match declarations {
        [Declaration::Type(declaration)] => Ok(SectionCode::Type(TypeCode {
            name: declaration.name.text.clone(),
            entries: match declaration.shape {
                TypeShape::Record(_) => TypeEntries::Fields,
                TypeShape::Choice(_) => TypeEntries::Variants,
            },
            span: declaration.span,
        })),
        [Declaration::Function(_), ..]
            if declarations
                .iter()
                .all(|declaration| matches!(declaration, Declaration::Function(_))) =>
        {
            function_code(declarations, heading)
        }
        [declaration] => Ok(SectionCode::Model(ModelCode {
            declaration: model_declaration_label(declaration),
            span: declaration_span(declaration),
            sql: matches!(declaration, Declaration::ModelSql(_)),
        })),
        _ => {
            let mut error = Diagnostic::error(
                "M005",
                "each '##' section must contain one type, one Model declaration or all clauses of one function",
                heading,
            );
            error
                .related
                .extend(declarations.iter().map(|declaration| Label {
                    span: declaration_span(declaration),
                    message: "declaration found in this section".into(),
                }));
            Err(vec![error])
        }
    }
}

fn model_declaration_label(declaration: &Declaration) -> String {
    match declaration {
        Declaration::Api(binding) => format!("{} {}", binding.kind.keyword(), binding.action.text),
        Declaration::Rest(rest) => match &rest.model {
            None => "rest".into(),
            Some(model) => format!(
                "rest {}",
                model
                    .iter()
                    .map(|part| part.text.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            ),
        },
        Declaration::External(external) => format!(
            "external {} {}",
            external.ecosystem.keyword(),
            external.entry,
        ),
        Declaration::Database(binding) => format!("database {}", binding.name.text),
        Declaration::Schedule(schedule) => format!("schedule {}", schedule.target.text),
        Declaration::ModelIndex(index) => format!(
            "{}({})",
            if index.unique { "unique" } else { "index" },
            index
                .fields
                .iter()
                .map(|field| field.text.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Declaration::Relation(relation) => format!(
            "relation {} = {}",
            relation.name.text,
            relation
                .field
                .iter()
                .map(|name| name.text.as_str())
                .collect::<Vec<_>>()
                .join(".")
        ),
        Declaration::Seed(_) => "seed".into(),
        Declaration::Migration(migration) => format!("migrate {}", migration.name.text),
        Declaration::ModelSql(sql) => format!("sql {}", sql.name.text),
        Declaration::Type(_) | Declaration::Function(_) => unreachable!("handled declaration kind"),
    }
}

fn function_code(
    declarations: &[&Declaration],
    heading: Span,
) -> Result<SectionCode, Vec<Diagnostic>> {
    let clauses = declarations
        .iter()
        .map(|declaration| match declaration {
            Declaration::Function(function) => function,
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    let first = clauses[0];
    if clauses.iter().any(|clause| {
        clause.name.text != first.name.text || clause.inputs.len() != first.inputs.len()
    }) {
        let mut error = Diagnostic::error(
            "M005",
            "one '##' section may contain clauses of only one function and arity",
            heading,
        );
        error.related.extend(clauses.iter().map(|clause| Label {
            span: clause.span,
            message: format!(
                "function '{}'/{} is in this section",
                clause.name.text,
                clause.inputs.len()
            ),
        }));
        return Err(vec![error]);
    }
    let last = clauses.last().expect("nonempty function section");
    Ok(SectionCode::Function(FunctionCode {
        name: first.name.text.clone(),
        arity: first.inputs.len(),
        span: Span {
            end: last.span.end,
            ..first.span
        },
        input_names: input_names(first),
        clause_inputs: clauses.iter().map(|clause| input_names(clause)).collect(),
        fails: first.fails.as_ref().map(|ty| {
            ty.name
                .iter()
                .map(|name| name.text.as_str())
                .collect::<Vec<_>>()
                .join(".")
        }),
    }))
}

fn input_names(function: &crate::syntax::FunctionClause) -> Vec<DocumentValue> {
    function
        .inputs
        .iter()
        .map(|input| DocumentValue {
            text: input.name.text.clone(),
            span: input.name.span,
        })
        .collect()
}

fn declarations_in<'a>(package: &'a Package, block: &ProgramBlock) -> Vec<&'a Declaration> {
    package
        .declarations
        .iter()
        .filter(|declaration| {
            let span = declaration_span(declaration);
            block.content.start <= span.start && span.end <= block.content.end
        })
        .collect()
}

fn headings(source: &SourceFile) -> Vec<Heading> {
    let markdown = normalize_lone_carriage_returns(source.text());
    let mut depth = 0usize;
    let mut current: Option<Heading> = None;
    let mut headings = Vec::new();
    for (event, range) in Parser::new(&markdown).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) if depth == 0 => {
                if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                    current = Some(Heading {
                        level,
                        title: String::new(),
                        span: source.span(range.start, range.end),
                        range,
                    });
                }
                depth += 1;
            }
            Event::Start(_) => depth += 1,
            Event::End(TagEnd::Heading(level)) => {
                depth = depth.saturating_sub(1);
                if current
                    .as_ref()
                    .is_some_and(|heading| heading.level == level)
                {
                    headings.push(current.take().expect("matched active heading"));
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Text(text) | Event::Code(text) if current.is_some() => {
                current
                    .as_mut()
                    .expect("checked active heading")
                    .title
                    .push_str(&text);
            }
            Event::SoftBreak | Event::HardBreak if current.is_some() => {
                current
                    .as_mut()
                    .expect("checked active heading")
                    .title
                    .push(' ');
            }
            _ => {}
        }
    }
    headings
}

fn heading_marker(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "#",
        HeadingLevel::H2 => "##",
        _ => unreachable!("only package and declaration headings are collected"),
    }
}

struct Heading {
    level: HeadingLevel,
    title: String,
    span: Span,
    range: Range<usize>,
}
