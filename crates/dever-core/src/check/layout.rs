use std::collections::BTreeMap;

use crate::diagnostic::{Diagnostic, Label};
use crate::source::{SourceLayout, SourceMap, SourceRole, Span};

#[derive(Default)]
struct RoleFiles {
    root: Option<Span>,
    topics: Vec<Span>,
}

pub(super) fn validate(sources: &SourceMap) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    let mut roles: BTreeMap<(String, String, SourceRole), RoleFiles> = BTreeMap::new();

    for source in sources.files() {
        let layout = match source.layout() {
            Ok(layout) => layout,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        match layout {
            SourceLayout::Main => {}
            SourceLayout::Test {
                component,
                domain,
                topic,
            } => {
                check_bucket_names(
                    [&component, &domain, &topic],
                    source.span(0, 0),
                    &mut errors,
                );
            }
            SourceLayout::Loose if sources.strict_layout() => errors.push(Diagnostic::error(
                "C003",
                "application source must be main.dever or <component>/<domain>/<app|domain|model|port|adapter|api>.dever, with an optional role topic directory",
                source.span(0, 0),
            )),
            SourceLayout::Loose => {}
            SourceLayout::Role {
                component,
                domain,
                role,
                topics,
            } => {
                check_bucket_names(
                    std::iter::once(&component)
                        .chain(std::iter::once(&domain))
                        .chain(topics.iter()),
                    source.span(0, 0),
                    &mut errors,
                );
                if role != SourceRole::Api && topics.len() > 1 {
                    errors.push(Diagnostic::error(
                        "C003",
                        format!(
                            "{} topic directories are flat; nested directories are supported only below api/",
                            role.name()
                        ),
                        source.span(0, 0),
                    ));
                }
                let files = roles.entry((component, domain, role)).or_default();
                if topics.is_empty() {
                    files.root = Some(source.span(0, 0));
                } else {
                    files.topics.push(source.span(0, 0));
                }
            }
        }
    }

    for ((_, _, role), files) in roles {
        // Model topics name persisted entities; API topics name route paths.
        // Folding either into the root file would change its public identity.
        if matches!(role, SourceRole::Model | SourceRole::Api) {
            continue;
        }
        if let (Some(root), Some(topic)) = (files.root, files.topics.first().copied()) {
            let mut error = Diagnostic::error(
                "C003",
                format!(
                    "{}.dever and its {}/ directory are mutually exclusive",
                    role.name(),
                    role.name()
                ),
                topic,
            );
            error.related.push(Label {
                span: root,
                message: "role file declared here".into(),
            });
            errors.push(error);
        } else if files.root.is_none() && files.topics.len() == 1 {
            errors.push(Diagnostic::error(
                "C003",
                format!(
                    "a {}/ directory needs at least two cohesive topic files; merge a single topic into {}.dever",
                    role.name(),
                    role.name()
                ),
                files.topics[0],
            ));
        }
    }
    errors
}

fn check_bucket_names<'a>(
    names: impl IntoIterator<Item = &'a String>,
    span: Span,
    errors: &mut Vec<Diagnostic>,
) {
    for name in names {
        if matches!(
            name.as_str(),
            "common" | "shared" | "utils" | "helper" | "helpers" | "base" | "service" | "services"
        ) {
            errors.push(Diagnostic::error(
                "C003",
                format!(
                    "'{name}' is a generic source bucket; name the business domain or cohesive topic instead"
                ),
                span,
            ));
        }
    }
}
