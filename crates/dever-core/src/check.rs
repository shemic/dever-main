use std::collections::BTreeMap;

use crate::diagnostic::Diagnostic;
use crate::hir::{Function, Program, TestCase};
use crate::source::{SourceLayout, SourceMap, SourceRole};
use crate::syntax::{Declaration, FunctionClause, Package};
use crate::types::{Definition, DefinitionKind, Shape};

mod api;
mod auth;
mod body;
mod clauses;
mod collections;
mod components;
mod concurrency;
mod constants;
mod cycles;
mod expressions;
mod intrinsics;
mod jobs;
mod layout;
mod model;
mod names;
mod operators;
mod orm;
mod ports;
mod sql_read;
mod upload;
pub(crate) use ports::test_program;
mod symbols;
mod values;
pub(crate) use body::check_suspensions;
pub(crate) use cycles::{visit, visit_expression};

type Checked<T> = Result<T, Diagnostic>;

struct Symbols {
    functions: BTreeMap<(String, usize), usize>,
    types: BTreeMap<String, usize>,
    packages: Vec<PackageScope>,
    strict_layout: bool,
}

struct PackageScope {
    name: String,
    layout: SourceLayout,
    bundled: bool,
    implemented_ports: std::collections::BTreeSet<String>,
    contract_types: std::collections::BTreeSet<String>,
}

struct Context<'a> {
    symbols: &'a Symbols,
    types: &'a [Definition],
    functions: &'a [Function],
    models: &'a [crate::model::ModelSchema],
    owner: usize,
}

/// Checks every declaration, including bundled standard source and unused functions.
pub fn check(sources: &SourceMap) -> Result<Program, Vec<Diagnostic>> {
    cache::check(sources, check_uncached)
}

mod cache;

fn check_uncached(sources: &SourceMap) -> Result<Program, Vec<Diagnostic>> {
    check_configured(sources, None)
}

/// Deployment bindings participate in checking before ambient identity types resolve.
/// They intentionally bypass the source-only cache; secrets are never retained in HIR.
pub fn check_with_settings(
    sources: &SourceMap,
    settings: &dever_runtime::config::Settings,
) -> Result<Program, Vec<Diagnostic>> {
    check_with_bindings(sources, &settings.compilation_bindings())
}

pub fn check_with_bindings(
    sources: &SourceMap,
    bindings: &dever_runtime::config::CompilationBindings,
) -> Result<Program, Vec<Diagnostic>> {
    check_configured(sources, Some(bindings))
}

fn check_configured(
    sources: &SourceMap,
    settings: Option<&dever_runtime::config::CompilationBindings>,
) -> Result<Program, Vec<Diagnostic>> {
    let mut errors = layout::validate(sources);
    if !errors.is_empty() {
        return Err(sorted(errors));
    }
    let packages: Vec<_> = sources
        .all_files()
        .filter_map(|source| match crate::parse(source) {
            Ok(package) => Some(package),
            Err(diagnostics) => {
                errors.extend(diagnostics);
                None
            }
        })
        .collect();
    if !errors.is_empty() {
        return Err(sorted(errors));
    }

    let mut symbols = Symbols {
        functions: BTreeMap::new(),
        types: BTreeMap::new(),
        packages: Vec::new(),
        strict_layout: sources.strict_layout(),
    };
    let mut types = Vec::new();
    let mut type_sources = Vec::new();
    let mut functions: Vec<Function> = Vec::new();
    let mut groups: Vec<Vec<&FunctionClause>> = Vec::new();
    for (owner, package) in packages.iter().enumerate() {
        if let Err(error) = names::check(package) {
            errors.push(error);
        }
        check_package(package, sources.strict_layout(), &mut errors);
        let package_name = path_name(&package.name);
        if symbols
            .packages
            .iter()
            .any(|scope| scope.name == package_name)
        {
            errors.push(Diagnostic::error("C002", "duplicate package", package.span));
        }
        let bundled = SourceMap::is_standard(package.span.source);
        symbols.packages.push(PackageScope {
            name: package_name.clone(),
            layout: package.layout.clone(),
            bundled,
            implemented_ports: package
                .declarations
                .iter()
                .filter_map(|declaration| {
                    let Declaration::Function(function) = declaration else {
                        return None;
                    };
                    let (port, _) = function.name.text.rsplit_once('.')?;
                    Some(format!("{}.{}", package.layout.app_prefix()?, port))
                })
                .collect(),
            contract_types: std::collections::BTreeSet::new(),
        });
        let mut last_function = None;
        for declaration in &package.declarations {
            match declaration {
                Declaration::Type(ty) => {
                    let qualified = declaration_name(package, &package_name, &ty.name.text);
                    let public = declaration_public(package, bundled, ty.public);
                    last_function = None;
                    if symbols
                        .types
                        .insert(qualified.clone(), types.len())
                        .is_some()
                    {
                        errors.push(Diagnostic::error("C002", "duplicate type", ty.span));
                    }
                    types.push(Definition {
                        name: qualified,
                        owner,
                        public,
                        shape: Shape::Record(Vec::new()),
                        span: ty.span,
                        properties: Default::default(),
                        kind: DefinitionKind::Regular,
                    });
                    type_sources.push(Some(ty));
                }
                Declaration::Function(clause) => {
                    let qualified = declaration_name(package, &package_name, &clause.name.text);
                    let public = declaration_public(package, bundled, clause.public);
                    let key = (qualified.clone(), clause.inputs.len());
                    if let Some(id) = symbols.functions.get(&key).copied() {
                        if functions[id].owner != owner {
                            errors.push(Diagnostic::error(
                                "C002",
                                format!("duplicate App function '{qualified}'"),
                                clause.span,
                            ));
                            last_function = None;
                            continue;
                        }
                        if functions[id].public != public {
                            errors.push(Diagnostic::error(
                                "C006",
                                "all clauses of a function must use the same public visibility",
                                clause.span,
                            ));
                        }
                        if last_function != Some(id) {
                            errors.push(Diagnostic::error(
                                "C002",
                                "function clauses must be contiguous",
                                clause.span,
                            ));
                        }
                        groups[id].push(clause);
                        last_function = Some(id);
                    } else {
                        let id = functions.len();
                        symbols.functions.insert(key, id);
                        functions.push(Function {
                            name: qualified,
                            owner,
                            public,
                            parameters: Vec::new(),
                            outputs: Vec::new(),
                            clauses: Vec::new(),
                            span: clause.span,
                            suspends: false,
                            pure: clause.pure,
                            recovery: clause.recovery.clone(),
                            kind: clause.kind,
                            port: None,
                            implementation: clause.name.text.contains('.'),
                            setting: None,
                            external: None,
                        });
                        groups.push(vec![clause]);
                        last_function = Some(id);
                    }
                }
                Declaration::Database(_)
                | Declaration::Schedule(_)
                | Declaration::ModelIndex(_)
                | Declaration::Relation(_)
                | Declaration::Seed(_)
                | Declaration::Migration(_)
                | Declaration::ModelSql(_)
                | Declaration::Api(_)
                | Declaration::Rest(_)
                | Declaration::External(_) => last_function = None,
            }
        }
    }
    let model_registrations = model::register(
        &packages,
        sources,
        &mut symbols,
        &mut types,
        &mut type_sources,
        &mut errors,
    );
    if !errors.is_empty() {
        return Err(sorted(errors));
    }
    ports::expand_contract_types(&packages, &mut symbols, &types, &type_sources);
    for (id, source) in type_sources.iter().enumerate() {
        let Some(source) = source else {
            continue;
        };
        let context = Context {
            symbols: &symbols,
            types: &types,
            functions: &functions,
            models: &[],
            owner: types[id].owner,
        };
        match context.shape(&source.shape) {
            Ok(shape) => types[id].shape = shape,
            Err(error) => errors.push(error),
        }
    }
    model::synthesize_fields(&model_registrations, &mut types, &mut errors);
    if !errors.is_empty() {
        return Err(sorted(errors));
    }
    let models = model::schemas(
        &model_registrations,
        &packages,
        &symbols,
        &types,
        &mut errors,
    );
    let model_rest_sources = model::rest_sources(&models, &packages, &types);
    if !errors.is_empty() {
        return Err(sorted(errors));
    }
    model::synthesize_relation_fields(&models, &mut types, &mut errors);
    symbols::validate_types(&types, &mut errors);
    if !errors.is_empty() {
        return Err(sorted(errors));
    }
    for (id, group) in groups.iter().enumerate() {
        let context = Context {
            symbols: &symbols,
            types: &types,
            functions: &functions,
            models: &models,
            owner: functions[id].owner,
        };
        match clauses::signature(group, &context) {
            Ok((parameters, outputs)) => {
                functions[id].parameters = parameters;
                functions[id].outputs = outputs;
            }
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        return Err(sorted(errors));
    }
    let mut adapters = ports::register(
        &packages,
        &groups,
        &symbols,
        &types,
        &mut functions,
        &mut errors,
    );
    ports::settings(
        &packages,
        &types,
        &mut functions,
        &mut adapters,
        &mut errors,
    );
    let tests = test_cases(&packages, &functions, &mut errors);
    let api::Entries {
        routes: mut api_routes,
        commands: api_commands,
        rest: mut api_rest,
    } = api::register(
        &packages,
        &symbols,
        &functions,
        &types,
        &models,
        &model_rest_sources,
        &mut errors,
    );
    upload::validate_signatures(&functions, &api_routes, &mut errors);
    let jobs = jobs::register(&packages, &functions, &types, &mut errors);
    let auth::Registration {
        hooks: auth,
        permissions,
    } = auth::register(
        settings,
        &packages,
        &functions,
        &types,
        &mut api_routes,
        &mut api_rest,
        &mut errors,
    );
    validate_public_boundaries(&functions, &types, &mut errors);
    validate_sensitive_boundaries(&packages, &functions, &types, &mut errors);
    // Markdown documents the unified function interface, after clause patterns resolve to types.
    for (owner, package) in packages.iter().enumerate() {
        let source = sources.get(package.span.source);
        if source.is_markdown() {
            errors.extend(crate::markdown::validate_contract(
                source, package, owner, &types, &functions,
            ));
        }
    }
    if !errors.is_empty() {
        return Err(sorted(errors));
    }
    let mut warnings = Vec::new();
    for (id, group) in groups.iter().enumerate() {
        if functions[id].port.is_some() {
            continue;
        }
        let context = Context {
            symbols: &symbols,
            types: &types,
            functions: &functions,
            models: &models,
            owner: functions[id].owner,
        };
        let result = (|| {
            let patterns = clauses::check(group, &functions[id].parameters, &context)?;
            group
                .iter()
                .zip(patterns)
                .map(|(source, domains)| {
                    body::check(source, domains, &functions[id], &context, &mut warnings)
                })
                .collect::<Checked<Vec<_>>>()
        })();
        match result {
            Ok(clauses) => functions[id].clauses = clauses,
            Err(error) => errors.push(error),
        }
    }
    if errors.is_empty() {
        cycles::check(&functions, &types, &symbols, &mut errors);
    }
    if errors.is_empty() {
        let mut program = Program {
            types,
            functions,
            warnings: sorted(warnings),
            effects: Vec::new(),
            recoveries: Vec::new(),
            suspension_handlers: Vec::new(),
            failures: Vec::new(),
            failure_handlers: Vec::new(),
            database_effects: Vec::new(),
            models,
            jobs,
            api_routes,
            api_commands,
            api_rest,
            permissions,
            tenant_components: Vec::new(),
            tests,
            adapters,
            auth,
            packages: packages
                .iter()
                .map(|package| crate::hir::PackageContract {
                    bundled: SourceMap::is_standard(package.span.source),
                    domain: package
                        .layout
                        .domain()
                        .map(|(component, domain)| (component.to_owned(), domain.to_owned())),
                    role: package.layout.role(),
                })
                .collect(),
        };
        // Test bodies are checked under their own fake bindings, never production effects.
        let test_owners: std::collections::BTreeSet<_> = program
            .tests
            .iter()
            .map(|test| program.functions[test.function].owner)
            .collect();
        let test_bodies: Vec<_> = program
            .functions
            .iter_mut()
            .enumerate()
            .filter(|(_, function)| test_owners.contains(&function.owner))
            .map(|(id, function)| (id, std::mem::take(&mut function.clauses)))
            .collect();
        crate::contracts::check(&mut program, &mut errors).map_err(|error| vec![error])?;
        components::register(&mut program, &mut errors);
        api::validate_rest_bindings(&program, &mut errors);
        api::validate_get_effects(&program, &mut errors);
        api::validate_write_transactions(&program, &mut errors);
        api::validate_context_roots(&program, &mut errors);
        api::validate_context_detachment(&program, &mut errors);
        auth::validate(&program, settings.is_some(), &mut errors);
        ports::validate_failures(&program, &mut errors);
        validate_exported_failures(&program, &mut errors);
        for (id, clauses) in test_bodies {
            program.functions[id].clauses = clauses;
        }
        finalize_tests(&mut program, &mut errors);
        program.warnings = sorted(program.warnings);
        if errors.is_empty() {
            Ok(program)
        } else {
            Err(sorted(errors))
        }
    } else {
        Err(sorted(errors))
    }
}

fn declaration_name(package: &Package, physical: &str, name: &str) -> String {
    match package.layout.role() {
        Some(SourceRole::App) => format!("{}.{}", package.layout.app_prefix().unwrap(), name),
        _ => format!("{physical}.{name}"),
    }
}

fn declaration_public(package: &Package, bundled: bool, explicit: bool) -> bool {
    if bundled || matches!(&package.layout, SourceLayout::Main | SourceLayout::Loose) {
        explicit
    } else {
        package.layout.role() == Some(SourceRole::App)
    }
}

fn test_cases(
    packages: &[Package],
    functions: &[Function],
    errors: &mut Vec<Diagnostic>,
) -> Vec<TestCase> {
    let mut tests = Vec::new();
    for (owner, package) in packages.iter().enumerate() {
        let SourceLayout::Test {
            component,
            domain,
            topic,
        } = &package.layout
        else {
            continue;
        };
        let entry_name = format!("{}.{}", path_name(&package.name), topic);
        let matching = functions
            .iter()
            .enumerate()
            .filter(|(_, function)| function.owner == owner && function.name == entry_name)
            .collect::<Vec<_>>();
        let [(function, entry)] = matching.as_slice() else {
            errors.push(Diagnostic::error(
                "C005",
                if matching.is_empty() {
                    format!("test file must declare the ordinary '{topic}() ()' entry")
                } else {
                    format!("test entry '{topic}' cannot be overloaded")
                },
                package.span,
            ));
            continue;
        };
        if entry.kind != crate::syntax::FunctionKind::Ordinary
            || !entry.parameters.is_empty()
            || !entry.outputs.is_empty()
        {
            errors.push(Diagnostic::error(
                "C005",
                format!(
                    "test entry '{topic}' must be an ordinary zero-input, zero-output function"
                ),
                entry.span,
            ));
            continue;
        }
        tests.push(TestCase {
            name: format!("{component}/{domain}/{topic}"),
            function: *function,
            span: entry.span,
            database_connections: Vec::new(),
            port_bindings: BTreeMap::new(),
        });
    }
    tests.sort_by(|left, right| left.name.cmp(&right.name));
    tests
}

fn finalize_tests(program: &mut Program, errors: &mut Vec<Diagnostic>) {
    ports::bind_tests(program);
    if !errors.is_empty() {
        return;
    }
    for index in 0..program.tests.len() {
        let case = match ports::test_program(program, index) {
            Ok(case) => case,
            Err(case_errors) => {
                errors.extend(case_errors);
                continue;
            }
        };
        let function = program.tests[index].function;
        let span = program.tests[index].span;
        let root = crate::specialize::Specialization {
            function,
            handlers: Vec::new(),
        };
        let instances = match crate::specialize::reachable(&case.functions, [root.clone()]) {
            Ok(instances) => instances,
            Err(cycle) => {
                errors.push(Diagnostic::error(
                    "C007",
                    "recursive specialized test calls are not allowed",
                    cycle,
                ));
                continue;
            }
        };
        if instances.iter().any(|instance| {
            matches!(
                program.functions[instance.function].name.as_str(),
                "dever.api.serve" | "dever.job.serve"
            )
        }) {
            errors.push(Diagnostic::error(
                "C006",
                "tests cannot start the application HTTP API or Job worker",
                span,
            ));
        }
        if crate::specialize::has_effect(&case, &root, "identity") {
            errors.push(Diagnostic::error(
                "C006",
                "HTTP identity and site capabilities are not available from tests",
                span,
            ));
        }
        let models = crate::specialize::database_effects(&case, &root);
        if models.is_empty() {
            continue;
        }
        let mut connections = std::collections::BTreeSet::from(["default".to_owned()]);
        for model in &program.models {
            if let crate::model::ConnectionSelector::Explicit(name) = &model.connection {
                connections.insert(name.clone());
            }
        }
        for job in &program.jobs {
            if let crate::model::ConnectionSelector::Explicit(name) = &job.connection {
                connections.insert(name.clone());
            }
        }
        program.tests[index].database_connections = connections.into_iter().collect();
    }
}

fn check_package(package: &Package, strict_layout: bool, errors: &mut Vec<Diagnostic>) {
    let name = path_name(&package.name);
    if !SourceMap::is_standard(package.span.source)
        && (package.name[0].text == "dever"
            || matches!(name.as_str(), "text" | "math" | "decimal" | "float" | "int"))
    {
        errors.push(Diagnostic::error(
            "C003",
            "reserved standard package namespace",
            package.name[0].span,
        ));
    }
    match package.layout.role() {
        None if strict_layout && matches!(&package.layout, SourceLayout::Main) => {
            for declaration in &package.declarations {
                if !matches!(declaration, Declaration::Function(function)
                    if function.name.text == "main"
                        && function.kind == crate::syntax::FunctionKind::Ordinary)
                {
                    errors.push(Diagnostic::error(
                        "C005",
                        "main.dever may contain only the ordinary main() boot function",
                        crate::markdown::declaration_span(declaration),
                    ));
                }
            }
        }
        Some(SourceRole::Model) => {
            for declaration in &package.declarations {
                if let Declaration::Function(function) = declaration {
                    errors.push(Diagnostic::error(
                        "C005",
                        "model source declares storage only; move business behavior to app or domain",
                        function.span,
                    ));
                }
            }
        }
        Some(SourceRole::Port) => {
            for declaration in &package.declarations {
                if !matches!(declaration, Declaration::Type(_) | Declaration::Function(_)) {
                    errors.push(Diagnostic::error(
                        "C005",
                        "Port declares only types and bodyless contracts",
                        crate::markdown::declaration_span(declaration),
                    ));
                }
            }
        }
        Some(SourceRole::Adapter) => {
            let externals = package
                .declarations
                .iter()
                .filter(|declaration| matches!(declaration, Declaration::External(_)))
                .count();
            if externals > 1 {
                errors.push(Diagnostic::error(
                    "C002",
                    "Adapter source may declare only one external implementation",
                    package.span,
                ));
            }
            if externals == 1 {
                for declaration in &package.declarations {
                    let allowed = matches!(declaration, Declaration::External(_))
                        || matches!(declaration, Declaration::Type(ty) if ty.setting);
                    if !allowed {
                        errors.push(Diagnostic::error(
                            "C005",
                            "external Adapter may contain only its external declaration and optional setting",
                            crate::markdown::declaration_span(declaration),
                        ));
                    }
                }
            }
        }
        None if matches!(package.layout, SourceLayout::Test { .. }) => {
            for declaration in &package.declarations {
                if let Declaration::Function(function) = declaration
                    && matches!(
                        function.name.text.as_str(),
                        "assert" | "assert_eq" | "secret"
                    )
                {
                    errors.push(Diagnostic::error(
                        "C002",
                        "assert, assert_eq and secret are reserved test primitives",
                        function.name.span,
                    ));
                }
            }
        }
        _ => {}
    }
}

fn validate_public_boundaries(
    functions: &[Function],
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) {
    let mut check_type = |ty: &crate::types::Type, name: &str, span| {
        symbols::referenced_types(ty, &mut |id| {
            if !types[id].public {
                errors.push(Diagnostic::error(
                    "C006",
                    format!(
                        "exported declaration '{name}' cannot expose private type '{}'",
                        types[id].name,
                    ),
                    span,
                ));
            }
        });
    };
    for definition in types.iter().filter(|definition| definition.public) {
        for field in symbols::shape_fields(&definition.shape)
            .into_iter()
            .filter(|field| !field.private)
        {
            check_type(&field.ty, &definition.name, definition.span);
        }
    }
    for function in functions.iter().filter(|function| function.public) {
        let mut check_type = |ty| check_type(ty, &function.name, function.span);
        for parameter in &function.parameters {
            match parameter {
                crate::types::Parameter::Value(ty) => check_type(ty),
                crate::types::Parameter::Handler(signature) => {
                    for ty in &signature.parameters {
                        check_type(ty);
                    }
                    for field in &signature.outputs {
                        check_type(&field.ty);
                    }
                }
            }
        }
        for output in &function.outputs {
            check_type(&output.ty);
        }
    }
}

fn validate_exported_failures(program: &Program, errors: &mut Vec<Diagnostic>) {
    for (id, function) in program.functions.iter().enumerate() {
        if program.packages[function.owner].role != Some(SourceRole::App) {
            continue;
        }
        let private_types = program.failures[id]
            .iter()
            .map(|failure| failure.ty)
            .filter(|ty| !program.types[*ty].public)
            .collect::<std::collections::BTreeSet<_>>();
        for ty in private_types {
            errors.push(Diagnostic::error(
                "C006",
                format!(
                    "exported function '{}' cannot expose private error type '{}'",
                    function.name, program.types[ty].name,
                ),
                function.span,
            ));
        }
    }
}

fn validate_sensitive_boundaries(
    packages: &[crate::syntax::Package],
    functions: &[Function],
    types: &[Definition],
    errors: &mut Vec<Diagnostic>,
) {
    for definition in types {
        if let crate::types::Shape::Choice(variants) = &definition.shape {
            for variant in variants.iter().filter(|variant| variant.error) {
                if variant
                    .fields
                    .iter()
                    .any(|field| !field.ty.observable(types))
                {
                    errors.push(Diagnostic::error(
                        "C006",
                        "error payload cannot contain Secret or private Model data",
                        definition.span,
                    ));
                }
            }
        }
    }
    for function in functions {
        let app = packages[function.owner].layout.role() == Some(SourceRole::App);
        let entry = function.name == "main.main"
            || (function.public && !crate::source::SourceMap::is_standard(function.span.source));
        if !app && !entry {
            continue;
        }
        let sensitive_input = function.parameters.iter().any(|parameter| match parameter {
            crate::types::Parameter::Value(ty) => ty.contains_private_model(types),
            crate::types::Parameter::Handler(signature) => {
                signature
                    .parameters
                    .iter()
                    .any(|ty| ty.contains_private_model(types))
                    || signature
                        .outputs
                        .iter()
                        .any(|field| !field.ty.observable(types))
            }
        });
        if sensitive_input
            || function
                .outputs
                .iter()
                .any(|field| !field.ty.observable(types))
        {
            errors.push(Diagnostic::error(
                "C006",
                "App and entry boundaries cannot expose private Model data or output Secret; return an explicit View",
                function.span,
            ));
        }
    }
}

fn path_name(path: &[crate::syntax::Name]) -> String {
    path.iter()
        .map(|name| name.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn sorted(mut errors: Vec<Diagnostic>) -> Vec<Diagnostic> {
    errors.sort_by_key(|error| {
        (
            error.primary.source,
            error.primary.start,
            error.primary.end,
            error.code,
        )
    });
    errors
}
