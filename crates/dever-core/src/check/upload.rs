use super::{Checked, body::Body};
use crate::diagnostic::Diagnostic;
use crate::hir::{Expression, ExpressionKind};
use crate::intrinsic::Intrinsic;
use crate::source::Span;
use crate::syntax::{Expression as SourceExpression, ExpressionKind as Ast};
use crate::types::Type;

pub(super) fn validate_signatures(
    functions: &[crate::hir::Function],
    routes: &[crate::hir::ApiRoute],
    errors: &mut Vec<Diagnostic>,
) {
    for (id, function) in functions.iter().enumerate() {
        if crate::source::SourceMap::is_standard(function.span.source) {
            continue;
        }
        let allowed = routes
            .iter()
            .any(|route| route.function == id && route.method == "POST")
            || function.port.is_some()
            || function.implementation;
        for parameter in &function.parameters {
            let Some(ty) = parameter.value_type() else {
                if let crate::types::Parameter::Handler(handler) = parameter
                    && handler
                        .parameters
                        .iter()
                        .chain(handler.outputs.iter().map(|field| &field.ty))
                        .any(|ty| ty == &Type::Upload)
                {
                    errors.push(Diagnostic::error(
                        "C005",
                        "Upload cannot cross a handler boundary",
                        function.span,
                    ));
                }
                continue;
            };
            let mut upload = false;
            crate::types::visit_type(ty, &mut |ty| upload |= matches!(ty, Type::Upload));
            if upload && (!allowed || ty != &Type::Upload) {
                errors.push(Diagnostic::error(
                    "C005",
                    "Upload requires a direct POST App, Port or Adapter input",
                    function.span,
                ));
            }
        }
        for field in &function.outputs {
            let mut upload = false;
            crate::types::visit_type(&field.ty, &mut |ty| upload |= matches!(ty, Type::Upload));
            if upload {
                errors.push(Diagnostic::error(
                    "C005",
                    "Upload cannot be returned",
                    function.span,
                ));
            }
        }
    }
}

impl Body<'_, '_> {
    pub(super) fn upload_call(
        &mut self,
        name: &str,
        arguments: &[SourceExpression],
        span: Span,
    ) -> Option<Checked<Expression>> {
        let (operation, ty, consume) = match name {
            "dever.api.upload_filename" | "dever.system.upload_filename" => {
                (Intrinsic::UploadFilename, Type::Text, false)
            }
            "dever.api.upload_content_type" | "dever.system.upload_content_type" => {
                (Intrinsic::UploadContentType, Type::Text, false)
            }
            "dever.api.upload_size" | "dever.system.upload_size" => {
                (Intrinsic::UploadSize, Type::Int, false)
            }
            "dever.api.close_upload" | "dever.system.upload_close" => {
                (Intrinsic::UploadClose, Type::Unit, true)
            }
            "dever.system.upload_store" => {
                let result = self.context.named_type("dever.storage.PutResult", span);
                match result {
                    Ok(result) => (Intrinsic::UploadStore, Type::Named(result), true),
                    Err(error) => return Some(Err(error)),
                }
            }
            _ => return None,
        };
        Some((|| {
            if name.starts_with("dever.system.")
                && !self.context.symbols.packages[self.context.owner].bundled
            {
                return Err(Diagnostic::error(
                    "C006",
                    "system upload primitives are reserved for the official library",
                    span,
                ));
            }
            let [argument] = arguments else {
                return Err(Diagnostic::error(
                    "C005",
                    "upload operation requires one Upload local",
                    span,
                ));
            };
            let Ast::Name(path) = &argument.kind else {
                return Err(Diagnostic::error(
                    "C005",
                    "upload operation requires an Upload local",
                    span,
                ));
            };
            if path.len() != 1 {
                return Err(Diagnostic::error(
                    "C005",
                    "upload operation requires an Upload local",
                    span,
                ));
            }
            let (slot, input) = self.affine_local(&path[0].text, argument.span, consume)?;
            if input != Type::Upload {
                return Err(Diagnostic::error(
                    "C005",
                    "upload operation requires Upload",
                    span,
                ));
            }
            Ok(Expression {
                kind: ExpressionKind::Intrinsic {
                    operation,
                    handler: None,
                    arguments: vec![Expression {
                        kind: ExpressionKind::Local(slot),
                        ty: input,
                        span: argument.span,
                    }],
                },
                ty,
                span,
            })
        })())
    }
}
