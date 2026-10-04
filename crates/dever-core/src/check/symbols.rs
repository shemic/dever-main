use std::collections::BTreeSet;

use crate::diagnostic::Diagnostic;
use crate::source::{SourceLayout, SourceRole, Span};
use crate::syntax::{self, TypeRef};
use crate::types::{Definition, DefinitionKind, Field, Shape, Type, Variant};

use super::{Checked, Context, cycles, path_name};

impl Context<'_> {
    pub(super) fn owns_private_fields(&self, id: usize) -> bool {
        let definition = &self.types[id];
        if definition.kind != crate::types::DefinitionKind::Model {
            return definition.owner == self.owner;
        }
        let caller = &self.symbols.packages[self.owner].layout;
        let owner = &self.symbols.packages[definition.owner].layout;
        caller.role() == Some(crate::source::SourceRole::App) && caller.domain() == owner.domain()
    }
    pub(super) fn is_test(&self) -> bool {
        matches!(
            &self.symbols.packages[self.owner].layout,
            SourceLayout::Test { .. }
        )
    }

    fn fragment_main(&self) -> bool {
        !self.symbols.strict_layout
            && matches!(
                &self.symbols.packages[self.owner].layout,
                SourceLayout::Main
            )
    }

    pub(super) fn candidates(&self, name: &str) -> Vec<String> {
        let scope = &self.symbols.packages[self.owner];
        if !name.contains('.') {
            return vec![match scope.layout.role() {
                Some(SourceRole::App) => {
                    format!("{}.{name}", scope.layout.app_prefix().unwrap())
                }
                _ => format!("{}.{name}", scope.name),
            }];
        }

        let mut candidates = Vec::new();
        if let SourceLayout::Role {
            component, domain, ..
        }
        | SourceLayout::Test {
            component, domain, ..
        } = &scope.layout
        {
            if let Some(rest) = name.strip_prefix("app.") {
                candidates.push(format!("{component}.{domain}.{rest}"));
            } else if matches!(
                name.split('.').next(),
                Some("domain" | "model" | "port" | "adapter" | "api" | "job")
            ) {
                candidates.push(format!("{component}.{domain}.{name}"));
            }
            candidates.push(name.to_owned());
            candidates.push(format!("{component}.{name}"));
        } else {
            candidates.push(name.to_owned());
        }
        candidates.dedup();
        candidates
    }

    pub(super) fn has_function(&self, name: &str, arity: usize) -> bool {
        self.candidates(name).iter().any(|candidate| {
            self.symbols
                .functions
                .contains_key(&(candidate.clone(), arity))
        })
    }

    pub(super) fn named_type(&self, name: &str, span: Span) -> Checked<usize> {
        let (name, id) = self
            .candidates(name)
            .into_iter()
            .find_map(|candidate| {
                self.symbols
                    .types
                    .get(&candidate)
                    .copied()
                    .map(|id| (candidate, id))
            })
            .ok_or_else(|| Diagnostic::error("C004", format!("unknown type '{name}'"), span))?;
        if !self.can_access_type(id) {
            let target = &self.symbols.packages[self.types[id].owner];
            let legacy = matches!(
                &self.symbols.packages[self.owner].layout,
                SourceLayout::Loose
            ) || matches!(&target.layout, SourceLayout::Loose)
                || self.fragment_main();
            return Err(Diagnostic::error(
                "C006",
                if legacy {
                    format!("type '{name}' is private")
                } else {
                    format!("type '{name}' is outside the current domain boundary")
                },
                span,
            ));
        }
        Ok(id)
    }

    pub(super) fn variant(&self, name: &str, span: Span) -> Checked<Option<(usize, usize)>> {
        let Some((type_name, variant_name)) = name.rsplit_once('.') else {
            return Ok(None);
        };
        let Some(id) = self
            .candidates(type_name)
            .into_iter()
            .find_map(|name| self.symbols.types.get(&name).copied())
        else {
            return Ok(None);
        };
        self.named_type(type_name, span)?;
        let Shape::Choice(variants) = &self.types[id].shape else {
            return Ok(None);
        };
        let variant = variants
            .iter()
            .position(|variant| variant.name == variant_name)
            .ok_or_else(|| Diagnostic::error("C004", format!("unknown variant '{name}'"), span))?;
        Ok(Some((id, variant)))
    }

    pub(super) fn related_variant(
        &self,
        name: &str,
        arguments: &[TypeRef],
        span: Span,
    ) -> Checked<Option<(Type, usize, Vec<Field>)>> {
        let variant = match name {
            "Related.Unloaded" => 0,
            "Related.Loaded" => 1,
            _ => return Ok(None),
        };
        if arguments.len() != 1 {
            return Err(Diagnostic::error(
                "C005",
                "Related variant patterns require one Model type argument",
                span,
            ));
        }
        let payload = self.resolve_type(&arguments[0])?;
        if !related_payload(&payload, self.types) {
            return Err(Diagnostic::error(
                "C005",
                "Related payload must be a Model, nullable Model or List<Model>",
                arguments[0].span,
            ));
        }
        let fields = if variant == 0 {
            Vec::new()
        } else {
            vec![Field {
                name: "value".into(),
                ty: payload.clone(),
                private: false,
                bounds: Vec::new(),
            }]
        };
        Ok(Some((Type::Related(Box::new(payload)), variant, fields)))
    }

    pub(super) fn function(&self, name: &str, arity: usize, span: Span) -> Checked<usize> {
        let (qualified, id) = self
            .candidates(name)
            .into_iter()
            .find_map(|candidate| {
                self.symbols
                    .functions
                    .get(&(candidate.clone(), arity))
                    .copied()
                    .map(|id| (candidate, id))
            })
            .ok_or_else(|| {
                Diagnostic::error(
                    "C004",
                    format!("unknown function '{name}' with {arity} argument(s)"),
                    span,
                )
            })?;
        if !self.can_call(id) {
            let target = &self.symbols.packages[self.functions[id].owner];
            let legacy = matches!(
                &self.symbols.packages[self.owner].layout,
                SourceLayout::Loose
            ) || matches!(&target.layout, SourceLayout::Loose)
                || self.fragment_main();
            return Err(Diagnostic::error(
                "C006",
                if legacy {
                    format!("function '{qualified}' is private")
                } else {
                    format!("function '{qualified}' is not callable from this source role")
                },
                span,
            ));
        }
        Ok(id)
    }

    fn can_call(&self, function: usize) -> bool {
        if matches!(
            self.functions[function].kind,
            crate::syntax::FunctionKind::Job { .. }
        ) {
            return false;
        }
        if self.functions[function].implementation {
            return false;
        }
        let target = &self.symbols.packages[self.functions[function].owner];
        let caller = &self.symbols.packages[self.owner];
        if caller.layout.role() == Some(SourceRole::Job) && target.bundled {
            return false;
        }
        if matches!(&target.layout, SourceLayout::Test { .. }) {
            return matches!(&caller.layout, SourceLayout::Test { .. })
                && self.functions[function].owner == self.owner;
        }
        if target.bundled {
            return self.functions[function].owner == self.owner || self.functions[function].public;
        }
        if self.fragment_main() {
            return self.functions[function].owner == self.owner || self.functions[function].public;
        }
        if target.layout.role() == Some(SourceRole::Api) {
            return false;
        }
        if matches!(&caller.layout, SourceLayout::Loose)
            || matches!(&target.layout, SourceLayout::Loose)
        {
            return self.functions[function].owner == self.owner || self.functions[function].public;
        }
        if matches!(&caller.layout, SourceLayout::Test { .. }) {
            return match target.layout.role() {
                Some(SourceRole::App) => true,
                Some(SourceRole::Domain) => caller.layout.domain() == target.layout.domain(),
                Some(
                    SourceRole::Model
                    | SourceRole::Port
                    | SourceRole::Adapter
                    | SourceRole::Api
                    | SourceRole::Job,
                )
                | None => false,
            };
        }
        match caller.layout.role() {
            None => target.layout.role() == Some(SourceRole::App),
            Some(SourceRole::Api | SourceRole::Job) => {
                target.layout.role() == Some(SourceRole::App)
                    && caller.layout.domain() == target.layout.domain()
            }
            Some(SourceRole::App) => {
                target.layout.role() == Some(SourceRole::App)
                    || (matches!(
                        target.layout.role(),
                        Some(SourceRole::Domain | SourceRole::Port)
                    ) && caller.layout.domain() == target.layout.domain())
            }
            Some(SourceRole::Domain) => {
                target.layout.role() == Some(SourceRole::Domain)
                    && caller.layout.domain() == target.layout.domain()
            }
            Some(SourceRole::Adapter) => self.functions[function].owner == self.owner,
            Some(SourceRole::Model | SourceRole::Port) => false,
        }
    }

    fn can_access_type(&self, definition: usize) -> bool {
        let target = &self.symbols.packages[self.types[definition].owner];
        let caller = &self.symbols.packages[self.owner];
        if matches!(&target.layout, SourceLayout::Test { .. }) {
            return matches!(&caller.layout, SourceLayout::Test { .. })
                && self.types[definition].owner == self.owner;
        }
        if target.bundled {
            return self.types[definition].owner == self.owner || self.types[definition].public;
        }
        if self.fragment_main() {
            return self.types[definition].owner == self.owner || self.types[definition].public;
        }
        if matches!(&caller.layout, SourceLayout::Loose)
            || matches!(&target.layout, SourceLayout::Loose)
        {
            return self.types[definition].owner == self.owner || self.types[definition].public;
        }
        if target.layout.role() == Some(SourceRole::Model) {
            return self.types[definition].public
                && !matches!(
                    caller.layout.role(),
                    Some(SourceRole::Domain | SourceRole::Port | SourceRole::Adapter)
                );
        }
        if self.types[definition].owner == self.owner {
            return true;
        }
        if matches!(&caller.layout, SourceLayout::Test { .. }) {
            return match target.layout.role() {
                Some(SourceRole::App) => self.types[definition].public,
                Some(SourceRole::Domain | SourceRole::Port) => {
                    caller.layout.domain() == target.layout.domain()
                }
                Some(SourceRole::Model) => self.types[definition].public,
                Some(SourceRole::Adapter | SourceRole::Api | SourceRole::Job) | None => false,
            };
        }
        if matches!(
            caller.layout.role(),
            Some(SourceRole::Port | SourceRole::Adapter)
        ) {
            if target.layout.role() == Some(SourceRole::App)
                && self.types[definition].public
                && caller.layout.domain() == target.layout.domain()
            {
                let name = &self.types[definition].name;
                return if caller.layout.role() == Some(SourceRole::Port) {
                    caller.contract_types.contains(name)
                } else {
                    self.symbols.packages.iter().any(|port| {
                        caller.implemented_ports.contains(&port.name)
                            && port.contract_types.contains(name)
                    })
                };
            }
            return target.layout.role() == Some(SourceRole::Port)
                && caller.implemented_ports.contains(&target.name);
        }
        match target.layout.role() {
            Some(SourceRole::App) => self.types[definition].public,
            Some(SourceRole::Domain | SourceRole::Port) => {
                caller.layout.domain() == target.layout.domain()
                    && (caller.layout.role() == Some(SourceRole::App)
                        || caller.layout.role() == target.layout.role())
            }
            Some(SourceRole::Adapter | SourceRole::Api | SourceRole::Model | SourceRole::Job)
            | None => false,
        }
    }

    pub(super) fn resolve_type(&self, reference: &TypeRef) -> Checked<Type> {
        let arguments = reference
            .arguments
            .iter()
            .map(|ty| self.resolve_type(ty))
            .collect::<Checked<Vec<_>>>()?;
        if arguments.iter().any(|ty| ty == &Type::Upload) {
            return Err(Diagnostic::error(
                "C005",
                "Upload cannot be stored in a container",
                reference.span,
            ));
        }
        let name = path_name(&reference.name);
        let ty = match (name.as_str(), arguments.as_slice()) {
            ("Bool", []) => Type::Bool,
            ("Int", []) => Type::Int,
            ("Decimal", []) => Type::Decimal,
            ("Float", []) => Type::Float,
            ("Text", []) => Type::Text,
            ("Id", []) => Type::Id,
            ("Bytes", []) => Type::Bytes,
            ("Secret", []) => Type::Secret,
            ("Upload", []) => Type::Upload,
            ("Uuid", []) => Type::Uuid,
            ("DateTime", []) => Type::DateTime,
            ("Date", []) => Type::Date,
            ("Time", []) => Type::Time,
            ("Duration", []) => Type::Duration,
            ("Json", []) => Type::Json,
            ("dever.system.File", []) => Type::File,
            ("dever.system.Socket", []) => Type::Socket,
            ("dever.system.Listener", []) => Type::Listener,
            ("dever.system.HttpReply", []) => Type::HttpReply,
            ("dever.system.WebSocket", []) => Type::WebSocket,
            ("dever.system.ClientTls", []) => Type::ClientTls,
            ("dever.system.ServerTls", []) => Type::ServerTls,
            ("dever.system.HttpClient", []) => Type::HttpClient,
            ("List", [element]) => Type::List(Box::new(element.clone())),
            ("Map", [key, value]) => Type::Map(Box::new(key.clone()), Box::new(value.clone())),
            ("MapEntry", [key, value]) => {
                Type::MapEntry(Box::new(key.clone()), Box::new(value.clone()))
            }
            ("Stream", [element]) => Type::Stream(Box::new(element.clone())),
            ("AsyncStream", [element]) => Type::AsyncStream(Box::new(element.clone())),
            ("Channel", [element]) => Type::Channel(Box::new(element.clone())),
            ("Related", [element]) if related_payload(element, self.types) => {
                Type::Related(Box::new(element.clone()))
            }
            ("Related", [_]) => {
                return Err(Diagnostic::error(
                    "C005",
                    "Related payload must be a Model, nullable Model or List<Model>",
                    reference.span,
                ));
            }
            ("List" | "Map" | "MapEntry" | "Stream" | "AsyncStream" | "Channel" | "Related", _) => {
                return Err(Diagnostic::error(
                    "C005",
                    "incorrect number of type arguments",
                    reference.span,
                ));
            }
            (_, []) => Type::Named(self.named_type(&name, reference.span)?),
            _ => {
                return Err(Diagnostic::error(
                    "C005",
                    "user-defined generic types are not supported",
                    reference.span,
                ));
            }
        };
        if reference.nullable && ty == Type::Upload {
            return Err(Diagnostic::error(
                "C005",
                "Upload cannot be nullable",
                reference.span,
            ));
        }
        Ok(if reference.nullable {
            ty.nullable()
        } else {
            ty
        })
    }

    pub(super) fn fields(&self, fields: &[syntax::Field]) -> Checked<Vec<Field>> {
        let mut names = BTreeSet::new();
        fields
            .iter()
            .map(|field| {
                if !names.insert(&field.name.text) {
                    return Err(Diagnostic::error(
                        "C002",
                        "duplicate field or output name",
                        field.span,
                    ));
                }
                let ty = self.resolve_type(&field.ty)?;
                Ok(Field {
                    name: field.name.text.clone(),
                    bounds: super::clauses::field_bounds(field, &ty, self)?,
                    ty,
                    private: field.private,
                })
            })
            .collect()
    }

    pub(super) fn shape(&self, shape: &syntax::TypeShape) -> Checked<Shape> {
        match shape {
            syntax::TypeShape::Record(fields) => Ok(Shape::Record(self.fields(fields)?)),
            syntax::TypeShape::Choice(variants) => {
                let mut names = BTreeSet::new();
                Ok(Shape::Choice(
                    variants
                        .iter()
                        .map(|variant| {
                            if !names.insert(&variant.name.text) {
                                return Err(Diagnostic::error(
                                    "C002",
                                    "duplicate choice variant",
                                    variant.span,
                                ));
                            }
                            Ok(Variant {
                                name: variant.name.text.clone(),
                                fields: self.fields(&variant.payload)?,
                                error: variant.error,
                                label: variant.label.clone(),
                            })
                        })
                        .collect::<Checked<_>>()?,
                ))
            }
        }
    }

    pub(super) fn map_key(&self, ty: &Type) -> bool {
        map_key(ty, self.types)
    }

    pub(super) fn value_fields(&self, ty: &Type) -> Option<Vec<Field>> {
        match ty {
            Type::Named(id) => match &self.types[*id].shape {
                Shape::Record(fields) => Some(fields.clone()),
                _ => None,
            },
            Type::MapEntry(key, value) => Some(vec![
                Field {
                    name: "key".into(),
                    ty: *key.clone(),
                    private: false,
                    bounds: Vec::new(),
                },
                Field {
                    name: "value".into(),
                    ty: *value.clone(),
                    private: false,
                    bounds: Vec::new(),
                },
            ]),
            Type::Outputs(fields) => Some(fields.clone()),
            _ => None,
        }
    }

    pub(super) fn model_choice(&self, name: &str, span: Span) -> Checked<Option<usize>> {
        let Some(id) = self
            .candidates(name)
            .into_iter()
            .find_map(|name| self.symbols.types.get(&name).copied())
        else {
            return Ok(None);
        };
        if self.types[id].kind != DefinitionKind::ModelChoice {
            return Ok(None);
        }
        self.named_type(name, span).map(Some)
    }
}

fn map_key(ty: &Type, definitions: &[Definition]) -> bool {
    match ty {
        Type::Bool | Type::Int | Type::Text | Type::Id => true,
        Type::Named(id) => {
            definitions[*id].kind == DefinitionKind::ModelId
                || matches!(&definitions[*id].shape, Shape::Choice(variants) if variants.iter().all(|variant| variant.fields.is_empty()))
        }
        _ => false,
    }
}

pub(super) fn referenced_types(ty: &Type, visit: &mut impl FnMut(usize)) {
    match ty {
        Type::Named(id) => visit(*id),
        Type::List(element)
        | Type::Nullable(element)
        | Type::Stream(element)
        | Type::AsyncStream(element)
        | Type::RowStream(element)
        | Type::Related(element)
        | Type::Channel(element) => {
            if !matches!(ty, Type::Related(_)) {
                referenced_types(element, visit)
            }
        }
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            referenced_types(key, visit);
            referenced_types(value, visit);
        }
        Type::Outputs(fields) | Type::Task(fields) => {
            for field in fields {
                referenced_types(&field.ty, visit);
            }
        }
        _ => {}
    }
}

pub(super) fn shape_fields(shape: &Shape) -> Vec<&Field> {
    match shape {
        Shape::Record(fields) => fields.iter().collect(),
        Shape::Choice(variants) => variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .collect(),
    }
}

pub(super) fn validate_maps(ty: &Type, types: &[Definition], span: Span) -> Checked<()> {
    match ty {
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            if !map_key(key, types) {
                return Err(Diagnostic::error("C005", "invalid Map key type", span));
            }
            validate_maps(value, types, span)
        }
        Type::List(element)
        | Type::Nullable(element)
        | Type::Stream(element)
        | Type::AsyncStream(element)
        | Type::RowStream(element)
        | Type::Related(element)
        | Type::Channel(element) => validate_maps(element, types, span),
        _ => Ok(()),
    }
}

pub(super) fn validate_type_constraints(
    ty: &Type,
    types: &[Definition],
    span: Span,
) -> Checked<()> {
    validate_maps(ty, types, span)?;
    validate_transfer_containers(ty, types, span)
}

fn validate_transfer_containers(ty: &Type, types: &[Definition], span: Span) -> Checked<()> {
    match ty {
        Type::AsyncStream(element) | Type::RowStream(element) | Type::Channel(element) => {
            if !element.transferable(types) {
                let container = if matches!(ty, Type::AsyncStream(_) | Type::RowStream(_)) {
                    "AsyncStream"
                } else {
                    "Channel"
                };
                return Err(Diagnostic::error(
                    "C005",
                    format!("{container} item type must be transferable between threads"),
                    span,
                ));
            }
            validate_transfer_containers(element, types, span)
        }
        Type::List(element)
        | Type::Nullable(element)
        | Type::Related(element)
        | Type::Stream(element) => validate_transfer_containers(element, types, span),
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            validate_transfer_containers(key, types, span)?;
            validate_transfer_containers(value, types, span)
        }
        _ => Ok(()),
    }
}

fn related_payload(ty: &Type, definitions: &[Definition]) -> bool {
    match ty {
        Type::Named(id) => definitions[*id].kind == DefinitionKind::Model,
        Type::Nullable(inner) => {
            matches!(
                inner.as_ref(),
                Type::Named(id) if definitions[*id].kind == DefinitionKind::Model
            )
        }
        Type::List(inner) => {
            matches!(
                inner.as_ref(),
                Type::Named(id) if definitions[*id].kind == DefinitionKind::Model
            )
        }
        _ => false,
    }
}

pub(super) fn validate_types(types: &[Definition], errors: &mut Vec<Diagnostic>) {
    let mut graph = vec![Vec::new(); types.len()];
    for (id, definition) in types.iter().enumerate() {
        for field in shape_fields(&definition.shape) {
            let mut upload = false;
            crate::types::visit_type(&field.ty, &mut |ty| upload |= matches!(ty, Type::Upload));
            if upload {
                errors.push(Diagnostic::error(
                    "C005",
                    "Upload cannot be stored in a record or choice",
                    definition.span,
                ));
            }
            referenced_types(&field.ty, &mut |next| {
                graph[id].push((next, definition.span))
            });
            if let Err(error) = validate_maps(&field.ty, types, definition.span) {
                errors.push(error);
            }
        }
    }
    if let Some(span) = cycles::cycle(&graph) {
        errors.push(Diagnostic::error(
            "C007",
            "recursive type definitions are not allowed",
            span,
        ));
    } else if errors.is_empty() {
        crate::types::cache_properties(types, &graph);
        for definition in types {
            for field in shape_fields(&definition.shape) {
                if let Err(error) = validate_transfer_containers(&field.ty, types, definition.span)
                {
                    errors.push(error);
                }
            }
        }
    }
}
