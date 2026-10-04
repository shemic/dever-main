use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Diagnostic;
use crate::hir::{
    Adapter, AdapterImplementation, ExternalAdapter, ExternalOperation, Failure, Function,
    PortOperation, Program,
};
use crate::source::{SourceLayout, SourceRole};
use crate::syntax::{FunctionClause, FunctionKind, InputKind, Package, Pattern};
use crate::types::{Definition, Shape, Type};

use super::{Context, Symbols};

pub(super) fn register(
    packages: &[Package],
    groups: &[Vec<&FunctionClause>],
    symbols: &Symbols,
    types: &[Definition],
    functions: &mut Vec<Function>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Adapter> {
    register_operations(packages, groups, symbols, types, functions, errors);
    let mut adapters = register_adapters(packages, groups, symbols, functions, errors);
    register_external_adapters(packages, groups, types, functions, &mut adapters, errors);
    validate_adapter_completeness(packages, functions, &adapters, errors);
    adapters
}

fn register_operations(
    packages: &[Package],
    groups: &[Vec<&FunctionClause>],
    symbols: &Symbols,
    types: &[Definition],
    functions: &mut [Function],
    errors: &mut Vec<Diagnostic>,
) {
    for id in 0..functions.len() {
        let source = groups[id][0];
        let owner = functions[id].owner;
        let package = &packages[owner];
        if package.layout.role() == Some(SourceRole::Adapter)
            && (source
                .inputs
                .iter()
                .any(|input| input.name.text == "setting")
                || source
                    .outputs
                    .iter()
                    .any(|field| field.name.text == "setting"))
        {
            errors.push(Diagnostic::error(
                "C006",
                "Adapter setting cannot be shadowed by a parameter or output",
                source.span,
            ));
        }
        if package.layout.role() != Some(SourceRole::Port) {
            if source.bodyless || source.fails.is_some() {
                errors.push(Diagnostic::error(
                    "C005",
                    "fails and bodyless contracts belong only to Port sources",
                    source.span,
                ));
            }
            if functions[id].implementation
                && package.layout.role() != Some(SourceRole::Adapter)
                && !matches!(package.layout, SourceLayout::Test { .. })
            {
                errors.push(Diagnostic::error(
                    "C005",
                    "qualified implementations belong only to Adapter or Test sources",
                    source.span,
                ));
            }
            continue;
        }
        if groups[id].len() != 1
            || !source.bodyless
            || source.fails.is_none()
            || source.kind != FunctionKind::Ordinary
            || source.pure
            || source.recovery.is_some()
            || functions[id].implementation
        {
            errors.push(Diagnostic::error(
                "C005",
                "Port operations require one ordinary bodyless signature with fails Choice",
                source.span,
            ));
            continue;
        }
        if source.inputs.iter().any(|input| !matches!(&input.kind, InputKind::Value(Pattern::Typed { bounds, .. }) if bounds.is_empty()))
            || source.outputs.iter().any(|field| !field.bounds.is_empty() || field.private)
        {
            errors.push(Diagnostic::error("C005", "Port signatures require typed value inputs and unbounded visible outputs", source.span));
            continue;
        }
        let context = Context {
            symbols,
            types,
            functions,
            models: &[],
            owner,
        };
        let failure_type = match context.resolve_type(source.fails.as_ref().unwrap()) {
            Ok(Type::Named(id)) if matches!(types[id].shape, Shape::Choice(_)) => id,
            Ok(_) => {
                errors.push(Diagnostic::error(
                    "C005",
                    "fails requires an error choice",
                    source.span,
                ));
                continue;
            }
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let Shape::Choice(variants) = &types[failure_type].shape else {
            unreachable!()
        };
        let failure_owner = &symbols.packages[types[failure_type].owner];
        if !types[failure_type].public
            || (!failure_owner.bundled
                && (failure_owner.layout.role() != Some(SourceRole::App)
                    || failure_owner.layout.domain() != package.layout.domain()))
        {
            errors.push(Diagnostic::error("C006", "Port fails must name a public App error choice in its domain or a public standard error choice", source.span));
            continue;
        }
        let failures: BTreeSet<_> = variants
            .iter()
            .enumerate()
            .filter(|(_, variant)| variant.error)
            .map(|(variant, _)| Failure {
                ty: failure_type,
                variant,
            })
            .collect();
        if failures.is_empty() {
            errors.push(Diagnostic::error(
                "C005",
                "Port failure choice must contain an error variant",
                source.span,
            ));
            continue;
        }
        functions[id].port = Some(PortOperation {
            identity: identity(package),
            operation: source.name.text.clone(),
            input_names: source
                .inputs
                .iter()
                .map(|input| input.name.text.clone())
                .collect(),
            failures,
            implementations: Vec::new(),
        });
    }
}

fn register_adapters(
    packages: &[Package],
    groups: &[Vec<&FunctionClause>],
    symbols: &Symbols,
    functions: &mut [Function],
    errors: &mut Vec<Diagnostic>,
) -> Vec<Adapter> {
    let mut adapters: Vec<Adapter> = Vec::new();
    for (id, source_group) in groups.iter().enumerate() {
        if !functions[id].implementation {
            continue;
        }
        let source = source_group[0];
        let owner = functions[id].owner;
        let package = &packages[owner];
        let Some(prefix) = package.layout.app_prefix() else {
            continue;
        };
        let target_name = format!("{prefix}.{}", source.name.text);
        let target = symbols
            .functions
            .get(&(target_name, source.inputs.len()))
            .copied();
        let Some(target) = target.filter(|target| functions[*target].port.is_some()) else {
            errors.push(Diagnostic::error(
                "C005",
                "qualified implementation must target an operation of this domain's Port",
                source.span,
            ));
            continue;
        };
        let port_owner = functions[target].owner;
        if !source.name.text.starts_with("port.") || source.kind != FunctionKind::Ordinary {
            errors.push(Diagnostic::error(
                "C005",
                "Adapter implementation must use port.operation or port.topic.operation",
                source.span,
            ));
            continue;
        }
        let expected = &functions[target];
        if functions[id].parameters != expected.parameters
            || functions[id].outputs != expected.outputs
            || source_group.iter().any(|clause| {
                clause
                    .inputs
                    .iter()
                    .map(|input| &input.name.text)
                    .ne(groups[target][0]
                        .inputs
                        .iter()
                        .map(|input| &input.name.text))
            })
        {
            errors.push(Diagnostic::error("C005", "implementation must preserve Port input names/types and ordered output names/types", source.span));
            continue;
        }
        let fake = matches!(package.layout, SourceLayout::Test { .. });
        let adapter_index = adapters.iter().position(|adapter| {
            adapter.owner == owner && (!fake || adapter.port_owner == port_owner)
        });
        let adapter_index = match adapter_index {
            Some(index) if adapters[index].port_owner != port_owner => {
                errors.push(Diagnostic::error(
                    "C005",
                    "one Adapter implementation owns exactly one Port identity",
                    source.span,
                ));
                continue;
            }
            Some(index) => index,
            None => {
                let name = match &package.layout {
                    SourceLayout::Role { topics, .. } if !topics.is_empty() => topics.join("."),
                    SourceLayout::Test { topic, .. } => topic.clone(),
                    _ => "default".into(),
                };
                adapters.push(Adapter {
                    owner,
                    name,
                    port_owner,
                    identity: identity(&packages[port_owner]),
                    operations: BTreeMap::new(),
                    fake: matches!(package.layout, SourceLayout::Test { .. }),
                    setting: None,
                    implementation: AdapterImplementation::Dever,
                });
                adapters.len() - 1
            }
        };
        adapters[adapter_index].operations.insert(target, id);
        if !adapters[adapter_index].fake {
            functions[target]
                .port
                .as_mut()
                .unwrap()
                .implementations
                .push(id);
        }
    }
    adapters
}

fn register_external_adapters(
    packages: &[Package],
    groups: &[Vec<&FunctionClause>],
    types: &[Definition],
    functions: &mut Vec<Function>,
    adapters: &mut Vec<Adapter>,
    errors: &mut Vec<Diagnostic>,
) {
    for (owner, package) in packages.iter().enumerate() {
        let declarations = package
            .declarations
            .iter()
            .filter_map(|declaration| {
                let crate::syntax::Declaration::External(external) = declaration else {
                    return None;
                };
                Some(external)
            })
            .collect::<Vec<_>>();
        let [source] = declarations.as_slice() else {
            continue;
        };
        if source.ecosystem == crate::syntax::ExternalEcosystem::Command
            && (!source.libs.is_empty()
                || package.declarations.iter().any(|declaration| {
                    matches!(declaration, crate::syntax::Declaration::Type(ty) if ty.setting)
                }))
        {
            errors.push(Diagnostic::error(
                "C005",
                "external command does not accept setting or lib declarations",
                source.span,
            ));
            continue;
        }
        if !valid_entry(&source.entry) {
            errors.push(Diagnostic::error(
                "C005",
                "external entry must be a portable relative path without NUL, '.', '..', '\\', or ':' segments",
                source.entry_span,
            ));
            continue;
        }
        let Some(domain) = package.layout.domain() else {
            errors.push(Diagnostic::error(
                "C005",
                "external implementation belongs only to an Adapter source",
                source.span,
            ));
            continue;
        };
        let port_owners = functions
            .iter()
            .filter(|function| {
                function.port.is_some() && packages[function.owner].layout.domain() == Some(domain)
            })
            .map(|function| function.owner)
            .collect::<BTreeSet<_>>();
        let port_owner = match port_owners.iter().copied().collect::<Vec<_>>().as_slice() {
            [port_owner] => *port_owner,
            [] => {
                errors.push(Diagnostic::error(
                    "C005",
                    "external Adapter requires one Port identity in its domain",
                    source.span,
                ));
                continue;
            }
            _ => {
                errors.push(Diagnostic::error(
                    "C005",
                    "external Adapter cannot choose between multiple Port identities in its domain",
                    source.span,
                ));
                continue;
            }
        };
        let Some(capabilities) = capabilities(source, errors) else {
            continue;
        };
        let mut lib_names = BTreeSet::new();
        let mut libs = Vec::new();
        for lib in &source.libs {
            let request = match source.ecosystem {
                crate::syntax::ExternalEcosystem::Exec => lib.text.clone(),
                ecosystem if lib.text.starts_with(&format!("{}:", ecosystem.keyword())) => {
                    lib.text.clone()
                }
                ecosystem => format!("{}:{}", ecosystem.keyword(), lib.text),
            };
            let valid = request
                .split_once(':')
                .is_some_and(|(ecosystem, name_version)| {
                    matches!(ecosystem, "pip" | "npm" | "go")
                        && name_version
                            .rsplit_once('@')
                            .is_some_and(|(name, version)| {
                                !name.is_empty()
                                    && !name.contains(':')
                                    && !version.is_empty()
                                    && !version.contains('@')
                            })
                        && !request.chars().any(char::is_whitespace)
                })
                && (source.ecosystem == crate::syntax::ExternalEcosystem::Exec
                    || request.starts_with(&format!("{}:", source.ecosystem.keyword())));
            if !valid {
                errors.push(Diagnostic::error(
                    "C005",
                    "external lib must use an exact name@version for its ecosystem",
                    lib.span,
                ));
            } else if !lib_names.insert(request.clone()) {
                errors.push(Diagnostic::error(
                    "C002",
                    "duplicate external lib declaration",
                    lib.span,
                ));
            } else {
                libs.push(crate::syntax::Name {
                    text: request,
                    span: lib.span,
                });
            }
        }
        if libs.len() != source.libs.len() {
            continue;
        }
        let contracts = functions
            .iter()
            .enumerate()
            .filter(|(_, function)| function.owner == port_owner && function.port.is_some())
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        let setting = package.declarations.iter().find_map(|declaration| {
            let crate::syntax::Declaration::Type(setting) = declaration else {
                return None;
            };
            setting.setting.then(|| {
                types
                    .iter()
                    .position(|ty| ty.owner == owner && ty.span == setting.span)
                    .expect("setting declaration registered as a private record")
            })
        });
        let schema = match external_schema(
            types,
            functions,
            groups,
            &contracts,
            setting,
            &libs,
            source.ecosystem,
        ) {
            Ok(schema) => schema,
            Err((span, error)) => {
                errors.push(Diagnostic::error("C005", error, span));
                continue;
            }
        };
        let name = match &package.layout {
            SourceLayout::Role { topics, .. } if !topics.is_empty() => topics.join("."),
            _ => "default".into(),
        };
        let entry = external_entry(package, &source.entry);
        let adapter_index = adapters.len();
        adapters.push(Adapter {
            owner,
            name,
            port_owner,
            identity: identity(&packages[port_owner]),
            operations: BTreeMap::new(),
            fake: false,
            setting: None,
            implementation: AdapterImplementation::External(ExternalAdapter {
                ecosystem: source.ecosystem,
                entry,
                libs: libs.iter().map(|lib| lib.text.clone()).collect(),
                capabilities,
                schema,
            }),
        });
        for contract in contracts {
            let implementation = functions.len();
            let operation = functions[contract]
                .port
                .as_ref()
                .expect("Port contract")
                .operation
                .clone();
            functions.push(Function {
                name: format!("{}.__external.{operation}", super::path_name(&package.name)),
                owner,
                public: false,
                parameters: functions[contract].parameters.clone(),
                outputs: functions[contract].outputs.clone(),
                clauses: Vec::new(),
                span: source.span,
                suspends: true,
                pure: false,
                recovery: None,
                kind: FunctionKind::Ordinary,
                port: None,
                implementation: true,
                setting: None,
                external: Some(ExternalOperation {
                    adapter: adapter_index,
                    contract,
                }),
            });
            adapters[adapter_index]
                .operations
                .insert(contract, implementation);
            functions[contract]
                .port
                .as_mut()
                .expect("Port contract")
                .implementations
                .push(implementation);
        }
    }
}

fn validate_adapter_completeness(
    packages: &[Package],
    functions: &[Function],
    adapters: &[Adapter],
    errors: &mut Vec<Diagnostic>,
) {
    for adapter in adapters {
        if functions.iter().enumerate().any(|(id, function)| {
            function.owner == adapter.port_owner
                && function.port.is_some()
                && !adapter.operations.contains_key(&id)
        }) {
            errors.push(Diagnostic::error(
                "C005",
                "Adapter must implement every operation of its Port",
                packages[adapter.owner].span,
            ));
        }
    }
    for (owner, package) in packages.iter().enumerate() {
        if package.layout.role() == Some(SourceRole::Adapter)
            && !adapters.iter().any(|adapter| adapter.owner == owner)
        {
            errors.push(Diagnostic::error(
                "C005",
                "Adapter source must implement a Port",
                package.span,
            ));
        }
    }
}

fn valid_entry(entry: &str) -> bool {
    !entry.is_empty()
        && !entry.starts_with('/')
        && !entry.contains(['\\', ':', '\0'])
        && entry
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".."))
}

fn external_entry(package: &Package, entry: &str) -> String {
    let directory = package.name[..package.name.len() - 1]
        .iter()
        .map(|name| name.text.as_str())
        .collect::<Vec<_>>()
        .join("/");
    format!("module/{directory}/{entry}")
}

fn capabilities(
    source: &crate::syntax::ExternalAdapter,
    errors: &mut Vec<Diagnostic>,
) -> Option<BTreeSet<&'static str>> {
    let mut capabilities = BTreeSet::new();
    let mut valid = true;
    for capability in &source.capabilities {
        let known = match capability.text.as_str() {
            "network" => "network",
            "file" => "file",
            "process" => "process",
            "gpu" => "gpu",
            _ => {
                valid = false;
                errors.push(Diagnostic::error(
                    "C005",
                    "unknown external capability",
                    capability.span,
                ));
                continue;
            }
        };
        if !capabilities.insert(known) {
            valid = false;
            errors.push(Diagnostic::error(
                "C002",
                "duplicate external capability",
                capability.span,
            ));
        }
    }
    valid.then_some(capabilities)
}

fn external_schema(
    types: &[Definition],
    functions: &[Function],
    groups: &[Vec<&FunctionClause>],
    contracts: &[usize],
    setting: Option<usize>,
    libs: &[crate::syntax::Name],
    ecosystem: crate::syntax::ExternalEcosystem,
) -> Result<String, (crate::source::Span, String)> {
    if ecosystem == crate::syntax::ExternalEcosystem::Command {
        validate_command_contract(types, functions, contracts)?;
    }
    let policy = crate::wire::Policy::external(ecosystem);
    let mut identity = String::from("dever-component-v1;");
    if ecosystem != crate::syntax::ExternalEcosystem::Exec {
        identity.push_str("ecosystem=");
        identity.push_str(ecosystem.keyword());
        identity.push(';');
    }
    for lib in libs {
        identity.push_str("lib=");
        identity.push_str(&lib.text);
        identity.push(';');
    }
    match setting {
        Some(setting) => {
            let schema = crate::wire::Schema::build(
                &Type::Named(setting),
                types,
                crate::wire::Policy::SettingInput,
            )
            .map_err(|error| (types[setting].span, error))?;
            identity.push_str("setting=");
            identity.push_str(schema.fingerprint());
            identity.push(';');
        }
        None => identity.push_str("setting=none;"),
    }
    for contract in contracts {
        let function = &functions[*contract];
        let operation = function.port.as_ref().expect("Port contract");
        identity.push_str("operation=");
        identity.push_str(&operation.operation);
        identity.push(';');
        for (index, parameter) in function.parameters.iter().enumerate() {
            let ty = parameter.value_type().expect("Port accepts value inputs");
            let field = crate::types::Field {
                name: operation.input_names[index].clone(),
                ty: ty.clone(),
                private: false,
                bounds: Vec::new(),
            };
            let schema = crate::wire::Schema::from_field(&field, types, policy)
                .map_err(|error| (groups[*contract][0].inputs[index].span, error))?;
            identity.push_str("input=");
            identity.push_str(&field.name);
            identity.push(':');
            identity.push_str(schema.fingerprint());
            identity.push(';');
        }
        for field in &function.outputs {
            let schema = crate::wire::Schema::from_field(field, types, policy)
                .map_err(|error| (function.span, error))?;
            identity.push_str("output=");
            identity.push_str(&field.name);
            identity.push(':');
            identity.push_str(schema.fingerprint());
            identity.push(';');
        }
        for failure in &operation.failures {
            let Shape::Choice(variants) = &types[failure.ty].shape else {
                unreachable!()
            };
            let variant = &variants[failure.variant];
            identity.push_str(&types[failure.ty].name);
            identity.push('.');
            identity.push_str(&variant.name);
            identity.push(';');
            for field in &variant.fields {
                let schema = crate::wire::Schema::from_field(field, types, policy)
                    .map_err(|error| (types[failure.ty].span, error))?;
                identity.push_str("error-field=");
                identity.push_str(&field.name);
                identity.push(':');
                identity.push_str(schema.fingerprint());
                identity.push(';');
            }
        }
    }
    Ok(crate::model::checksum(&identity))
}

fn validate_command_contract(
    types: &[Definition],
    functions: &[Function],
    contracts: &[usize],
) -> Result<(), (crate::source::Span, String)> {
    let function = &functions[contracts[0]];
    let fail = |message: &str| Err((function.span, message.to_owned()));
    if contracts.len() != 1 {
        return fail("external command requires exactly one Port operation");
    }
    let port = function.port.as_ref().expect("Port contract");
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| parameter.value_type().expect("Port value parameter"))
        .collect::<Vec<_>>();
    let args = Type::List(Box::new(Type::Text));
    let stdin = Type::Nullable(Box::new(Type::Bytes));
    if !(parameters.as_slice() == [&args] && port.input_names == ["args"]
        || parameters.as_slice() == [&args, &stdin] && port.input_names == ["args", "stdin"])
    {
        return fail("external command inputs must be args: List<Text> and optional stdin: Bytes?");
    }
    if !matches!(function.outputs.as_slice(), [field]
        if field.name == "output"
            && matches!(field.ty, Type::Named(id) if types[id].name == "dever.process.Output"))
    {
        return fail("external command must return output: dever.process.Output");
    }
    if port.failures.len() != 1
        || !port
            .failures
            .iter()
            .all(|failure| types[failure.ty].name == "dever.process.Error")
    {
        return fail("external command must declare fails dever.process.Error");
    }
    Ok(())
}

pub(super) fn expand_contract_types(
    packages: &[Package],
    symbols: &mut Symbols,
    types: &[Definition],
    sources: &[Option<&crate::syntax::TypeDeclaration>],
) {
    // Discover the complete signature type graph before shape access checks.
    // This grants only the App DTOs actually nested in a Port contract.
    for (owner, package) in packages.iter().enumerate() {
        if package.layout.role() != Some(SourceRole::Port) {
            continue;
        }
        let mut context = Context {
            symbols,
            types,
            functions: &[],
            models: &[],
            owner,
        };
        let mut pending = Vec::new();
        for declaration in &package.declarations {
            let crate::syntax::Declaration::Function(function) = declaration else {
                continue;
            };
            for input in &function.inputs {
                if let InputKind::Value(Pattern::Typed { ty, .. }) = &input.kind {
                    contract_reference(ty, &context, &mut pending);
                }
            }
            for field in &function.outputs {
                contract_reference(&field.ty, &context, &mut pending);
            }
            if let Some(fails) = &function.fails {
                contract_reference(fails, &context, &mut pending);
            }
        }
        let mut visited = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let Some(source) = sources[id] else {
                continue;
            };
            context.owner = types[id].owner;
            let fields: Vec<_> = match &source.shape {
                crate::syntax::TypeShape::Record(fields) => fields.iter().collect(),
                crate::syntax::TypeShape::Choice(variants) => variants
                    .iter()
                    .flat_map(|variant| &variant.payload)
                    .collect(),
            };
            for field in fields {
                contract_reference(&field.ty, &context, &mut pending);
            }
        }
        symbols.packages[owner].contract_types = visited
            .into_iter()
            .map(|id| types[id].name.clone())
            .collect();
    }
}

fn contract_reference(
    reference: &crate::syntax::TypeRef,
    context: &Context<'_>,
    pending: &mut Vec<usize>,
) {
    let name = super::path_name(&reference.name);
    if let Some(id) = context
        .candidates(&name)
        .iter()
        .find_map(|name| context.symbols.types.get(name))
    {
        pending.push(*id);
    }
    for argument in &reference.arguments {
        contract_reference(argument, context, pending);
    }
}

pub(super) fn settings(
    packages: &[Package],
    types: &[Definition],
    functions: &mut Vec<Function>,
    adapters: &mut [Adapter],
    errors: &mut Vec<Diagnostic>,
) {
    for (owner, package) in packages.iter().enumerate() {
        for declaration in &package.declarations {
            let crate::syntax::Declaration::Type(source) = declaration else {
                continue;
            };
            if !source.setting {
                continue;
            }
            let Some(adapter) = adapters
                .iter_mut()
                .find(|adapter| adapter.owner == owner && !adapter.fake)
            else {
                errors.push(Diagnostic::error(
                    "C005",
                    "setting belongs to an Adapter implementing one Port",
                    source.span,
                ));
                continue;
            };
            let ty = types
                .iter()
                .position(|ty| ty.owner == owner && ty.span == source.span)
                .expect("setting declaration registered as a private record");
            if let Err(error) = crate::wire::Schema::build(
                &Type::Named(ty),
                types,
                crate::wire::Policy::SettingInput,
            ) {
                errors.push(Diagnostic::error("C005", error, source.span));
                continue;
            }
            adapter.setting = Some(ty);
            functions.push(Function {
                name: format!("{}.__setting", super::path_name(&package.name)),
                owner,
                public: false,
                parameters: Vec::new(),
                outputs: vec![crate::types::Field {
                    name: "setting".into(),
                    ty: Type::Named(ty),
                    private: false,
                    bounds: Vec::new(),
                }],
                clauses: Vec::new(),
                span: source.span,
                suspends: false,
                pure: true,
                recovery: None,
                kind: FunctionKind::Ordinary,
                port: None,
                implementation: false,
                setting: Some(ty),
                external: None,
            });
        }
    }
}

fn identity(package: &Package) -> String {
    let mut identity = package.layout.app_prefix().expect("Port has a domain");
    if let SourceLayout::Role { topics, .. } = &package.layout {
        for topic in topics {
            identity.push('.');
            identity.push_str(topic);
        }
    }
    identity
}

pub(super) fn validate_failures(program: &Program, errors: &mut Vec<Diagnostic>) {
    let roots = program
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| {
            let package = &program.packages[function.owner];
            !package.bundled
                && (function.public
                    || function.name == "main.main"
                    || package.role == Some(SourceRole::Api))
        })
        .map(|(function, _)| crate::specialize::Specialization {
            function,
            handlers: Vec::new(),
        })
        .filter(|instance| {
            program.functions[instance.function]
                .parameters
                .iter()
                .all(|parameter| parameter.value_type().is_some())
        });
    if let Ok(reachable) = crate::specialize::reachable(&program.functions, roots) {
        for instance in reachable {
            let function = &program.functions[instance.function];
            if function
                .port
                .as_ref()
                .is_some_and(|port| port.implementations.is_empty())
            {
                errors.push(Diagnostic::error(
                    "C005",
                    "reachable Port requires a production Adapter",
                    function.span,
                ));
            }
        }
    }
    for adapter in program.adapters.iter().filter(|adapter| !adapter.fake) {
        validate_adapter_failures(program, adapter, errors);
    }
}

fn validate_adapter_failures(program: &Program, adapter: &Adapter, errors: &mut Vec<Diagnostic>) {
    for (contract, implementation) in &adapter.operations {
        let allowed = &program.functions[*contract].port.as_ref().unwrap().failures;
        if !program.failures[*implementation].is_subset(allowed)
            || !program.failure_handlers[*implementation].is_empty()
        {
            errors.push(Diagnostic::error(
                "C012",
                "Adapter failure set exceeds the declared Port contract",
                program.functions[*implementation].span,
            ));
        }
    }
}

/// Checking and native emission use the same closed graph for one isolated case.
pub(crate) fn test_program(program: &Program, index: usize) -> Result<Program, Vec<Diagnostic>> {
    let test = &program.tests[index];
    let owner = program.functions[test.function].owner;
    let mut case = program.clone();
    for (id, function) in case.functions.iter_mut().enumerate() {
        if let Some(port) = &mut function.port {
            port.implementations = test.port_bindings.get(&id).copied().into_iter().collect();
        }
    }
    // Also check unused fake operations/helpers; completeness does not make them reachable.
    let roots = case
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| {
            function.owner == owner
                && function
                    .parameters
                    .iter()
                    .all(|parameter| parameter.value_type().is_some())
        })
        .map(|(function, _)| crate::specialize::Specialization {
            function,
            handlers: Vec::new(),
        });
    let reachable =
        crate::specialize::execution_reachable(&case.functions, roots).map_err(|span| {
            vec![Diagnostic::error(
                "C007",
                "recursive calls through a test Port fake are not allowed",
                span,
            )]
        })?;
    if reachable.iter().any(|instance| {
        case.functions[instance.function].port.is_some()
            && !test.port_bindings.contains_key(&instance.function)
    }) {
        return Err(vec![Diagnostic::error(
            "C005",
            "each reachable Port requires a fake in this test file; production fallback is forbidden",
            test.span,
        )]);
    }
    // The static closure also retains uninstantiated handler helpers for ordinary
    // declaration checks; runtime reachability still uses concrete specialization.
    let functions = crate::contracts::Dependencies::new(&case).reachable(
        case.functions
            .iter()
            .enumerate()
            .filter(|(_, function)| function.owner == owner)
            .map(|(id, _)| id)
            .chain(reachable.iter().map(|instance| instance.function)),
    );
    for (id, function) in case.functions.iter_mut().enumerate() {
        if !functions.contains(&id) {
            function.clauses.clear();
            if let Some(port) = &mut function.port {
                port.implementations.clear();
            }
        }
    }
    let mut errors = Vec::new();
    crate::contracts::check(&mut case, &mut errors).map_err(|error| vec![error])?;
    for adapter in case
        .adapters
        .iter()
        .filter(|adapter| adapter.fake && adapter.owner == owner)
    {
        validate_adapter_failures(&case, adapter, &mut errors);
    }
    if errors.is_empty() {
        Ok(case)
    } else {
        Err(errors)
    }
}

pub(super) fn bind_tests(program: &mut Program) {
    for test in &mut program.tests {
        let owner = program.functions[test.function].owner;
        test.port_bindings = program
            .adapters
            .iter()
            .filter(|adapter| adapter.fake && adapter.owner == owner)
            .flat_map(|adapter| {
                adapter
                    .operations
                    .iter()
                    .map(|(port, implementation)| (*port, *implementation))
            })
            .collect();
    }
}
