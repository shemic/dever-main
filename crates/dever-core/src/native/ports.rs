use super::*;

pub(super) fn definitions(
    emitter: &Emitter<'_>,
    output: &mut String,
    instances: &BTreeSet<Specialization>,
) {
    for instance in instances {
        let id = instance.function;
        let function = &emitter.program.functions[id];
        if function.port.is_some() {
            writeln!(
                output,
                "static PORT_{id}: std::sync::OnceLock<usize> = std::sync::OnceLock::new();"
            )
            .expect("string formatting");
        }
    }
    for adapter in &emitter.program.adapters {
        let Some(ty) = adapter.setting else {
            continue;
        };
        if !adapter
            .operations
            .values()
            .any(|id| instances.iter().any(|instance| instance.function == *id))
        {
            continue;
        }
        let schema = crate::wire::Schema::build(
            &Type::Named(ty),
            &emitter.program.types,
            crate::wire::Policy::SettingInput,
        )
        .expect("checked setting schema");
        // Type IDs occupy their own range after API route schema indexes.
        let codec = emitter.program.api_routes.len() + ty;
        output.push_str(&wire::emit(&schema, codec));
        writeln!(
            output,
            "static ADAPTER_SETTING_{}: std::sync::OnceLock<T{ty}> = std::sync::OnceLock::new();",
            adapter.owner
        )
        .expect("string formatting");
    }
    let external = instances
        .iter()
        .filter_map(|instance| {
            emitter.program.functions[instance.function]
                .external
                .map(|_| instance.function)
        })
        .collect::<BTreeSet<_>>();
    for implementation in external {
        external_codecs(emitter, output, implementation);
    }
}

pub(super) fn function(
    emitter: &Emitter<'_>,
    output: &mut String,
    instance: &Specialization,
) -> bool {
    let function = &emitter.program.functions[instance.function];
    if function.setting.is_some() {
        writeln!(output, "Ok(ADAPTER_SETTING_{}.get().expect(\"Adapter setting initialized before entry\").clone())\n}}", function.owner).expect("string formatting");
        return true;
    }
    if let Some(external) = function.external {
        let adapter = &emitter.program.adapters[external.adapter];
        let contract = emitter.program.functions[external.contract]
            .port
            .as_ref()
            .expect("external Port contract");
        let arguments = (0..function.parameters.len())
            .map(|index| format!("&_p{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let location = emitter.location(function.span);
        writeln!(output, "let payload = external_{0}_inputs({arguments}).map_err(|error| AppError::fault({location}, error))?;", instance.function).expect("string formatting");
        writeln!(output, "match dever_runtime::component::call({}, {}, &payload).await.map_err(|error| AppError::fault({location}, error))? {{", rust_string(&external_key(adapter)), rust_string(&contract.operation)).expect("string formatting");
        writeln!(output, "dever_runtime::component::Reply::Result(payload) => external_{0}_outputs(&payload).map_err(|error| AppError::fault({location}, error)),", instance.function).expect("string formatting");
        writeln!(output, "dever_runtime::component::Reply::Error {{ identity, payload }} => Err(external_{0}_error(&identity, &payload, {location}).map_err(|error| AppError::fault({location}, error))?),\n}}\n}}", instance.function).expect("string formatting");
        return true;
    }
    let Some(port) = &function.port else {
        return false;
    };
    if port.implementations.is_empty() {
        output.push_str("unreachable!(\"checked Port has an implementation\")\n}\n");
        return true;
    }
    let arguments = (0..function.parameters.len())
        .map(|index| format!("_p{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let invoke = |id: usize| {
        let target = Specialization {
            function: id,
            handlers: Vec::new(),
        };
        format!(
            "{}({arguments}){}",
            emitter.names[&target],
            if specialize::suspends(emitter.program, &target) {
                ".await"
            } else {
                ""
            }
        )
    };
    if let [implementation] = port.implementations.as_slice() {
        writeln!(output, "{}\n}}", invoke(*implementation)).expect("string formatting");
    } else {
        writeln!(
            output,
            "match *PORT_{}.get().expect(\"Port initialized before entry\") {{",
            instance.function
        )
        .expect("string formatting");
        for implementation in &port.implementations {
            writeln!(output, "{implementation} => {},", invoke(*implementation))
                .expect("string formatting");
        }
        output.push_str("_ => unreachable!(\"closed Port binding\"),\n}\n}\n");
    }
    true
}

pub(super) fn initialize(
    emitter: &Emitter<'_>,
    output: &mut String,
    instances: &BTreeSet<Specialization>,
    span: Span,
) -> Result<(), String> {
    let program = emitter.program;
    let ports: BTreeSet<_> = instances
        .iter()
        .filter_map(|instance| {
            program.functions[instance.function]
                .port
                .as_ref()
                .map(|_| program.functions[instance.function].owner)
        })
        .collect();
    if ports.is_empty() {
        return Ok(());
    }
    let identities: BTreeSet<_> = program
        .functions
        .iter()
        .filter_map(|function| {
            function
                .port
                .as_ref()
                .map(|port| rust_string(&port.identity))
        })
        .collect();
    let location = emitter.location(span);
    let asynchronous = has_external(program, instances);
    let closure = if asynchronous {
        "|| async {"
    } else {
        "|| -> Result<(), String> {"
    };
    let settings = if asynchronous {
        "dever_runtime::component::adapter_settings()"
    } else {
        "dever_runtime::config::Settings::load_adapter_settings()"
    };
    writeln!(output, "let initialize_ports = {closure}\nlet settings = {settings}?;\nsettings.validate_adapter_names(&[{}])?;", identities.into_iter().collect::<Vec<_>>().join(", ")).expect("string formatting");
    for owner in ports {
        let adapters: Vec<_> = program
            .adapters
            .iter()
            .filter(|adapter| !adapter.fake && adapter.port_owner == owner)
            .collect();
        if adapters.is_empty() {
            return Err("reachable Port has no production Adapter".into());
        }
        let identity = &adapters[0].identity;
        let names = adapters
            .iter()
            .map(|adapter| rust_string(&adapter.name))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(output, "let (selected, setting) = settings.select_adapter({}, &[{names}])?;\nmatch selected {{", rust_string(identity)).expect("string formatting");
        for (index, adapter) in adapters.iter().enumerate() {
            writeln!(output, "{index} => {{").expect("string formatting");
            if let Some(ty) = adapter.setting {
                let codec = program.api_routes.len() + ty;
                writeln!(output, "let value = wire_{codec}_decode(setting.as_deref().ok_or(\"selected Adapter requires setting\")?)?;\nADAPTER_SETTING_{}.set(value).map_err(|_| String::from(\"Adapter setting initialized twice\"))?;", adapter.owner).expect("string formatting");
            } else {
                output.push_str("if setting.is_some() { return Err(String::from(\"selected Adapter does not declare setting\")); }\n");
            }
            if let crate::hir::AdapterImplementation::External(external) = &adapter.implementation {
                let capabilities = external
                    .capabilities
                    .iter()
                    .map(|capability| format!("{}.into()", rust_string(capability)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let operations = adapter
                    .operations
                    .keys()
                    .map(|port| {
                        format!(
                            "{}.into()",
                            rust_string(
                                &program.functions[*port]
                                    .port
                                    .as_ref()
                                    .expect("Port operation")
                                    .operation,
                            )
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let setting = if adapter.setting.is_some() {
                    "setting"
                } else {
                    "None"
                };
                writeln!(output, "dever_runtime::component::start(dever_runtime::component::Definition {{ key: {}.into(), ecosystem: {}.into(), entry: {}.into(), port: {}.into(), adapter: {}.into(), schema: {}.into(), capabilities: vec![{capabilities}], operations: vec![{operations}], setting: {setting}, timeout_ms: 30_000 }}).await?;", rust_string(&external_key(adapter)), rust_string(external.ecosystem.keyword()), rust_string(&external.entry), rust_string(&adapter.identity), rust_string(&adapter.name), rust_string(&external.schema)).expect("string formatting");
            }
            for (port, implementation) in &adapter.operations {
                if instances.iter().any(|instance| instance.function == *port) {
                    writeln!(output, "PORT_{port}.set({implementation}).map_err(|_| String::from(\"Port initialized twice\"))?;").expect("string formatting");
                }
            }
            output.push_str("},\n");
        }
        output.push_str("_ => unreachable!(\"validated Adapter selection\"),\n}\n");
    }
    writeln!(output, "Ok::<(), String>(())\n}};\ninitialize_ports(){}.map_err(|error| AppError::fault({location}, error))?;", if asynchronous { ".await" } else { "" }).expect("string formatting");
    Ok(())
}

pub(super) fn has_external(program: &Program, instances: &BTreeSet<Specialization>) -> bool {
    instances
        .iter()
        .any(|instance| program.functions[instance.function].external.is_some())
}

fn external_key(adapter: &crate::hir::Adapter) -> String {
    format!("{}:{}", adapter.identity, adapter.name)
}

fn external_codecs(emitter: &Emitter<'_>, output: &mut String, implementation: usize) {
    let function = &emitter.program.functions[implementation];
    let external = function.external.expect("external implementation");
    let crate::hir::AdapterImplementation::External(adapter) =
        &emitter.program.adapters[external.adapter].implementation
    else {
        unreachable!("external implementation Adapter")
    };
    let policy = crate::wire::Policy::external(adapter.ecosystem);
    let contract = emitter.program.functions[external.contract]
        .port
        .as_ref()
        .expect("Port contract");
    let mut input_schemas = Vec::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        let field = crate::types::Field {
            name: contract.input_names[index].clone(),
            ty: parameter
                .value_type()
                .expect("Port value parameter")
                .clone(),
            private: false,
            bounds: Vec::new(),
        };
        let schema = crate::wire::Schema::from_field(&field, &emitter.program.types, policy)
            .expect("checked external input schema");
        let prefix = format!("external_{implementation}_input_{index}");
        output.push_str(&wire::emit_named(&schema, &prefix));
        input_schemas.push((field, schema, prefix));
    }
    let input_parameters = input_schemas
        .iter()
        .enumerate()
        .map(|(index, (field, _, _))| format!("p{index}: &{}", rust_type(&field.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(output, "fn external_{implementation}_inputs({input_parameters}) -> Result<dever_runtime::wire::Encoded, String> {{\nlet mut writer = dever_runtime::wire::Encoder::default();\nwriter.begin_object()?;").expect("string formatting");
    for (index, (field, schema, prefix)) in input_schemas.iter().enumerate() {
        writeln!(
            output,
            "writer.key({})?; {prefix}_e{}(p{index}, &mut writer)?;",
            rust_string(&field.name),
            schema.root
        )
        .expect("string formatting");
    }
    output.push_str("writer.end()?; writer.finish()\n}\n");

    let mut output_schemas = Vec::new();
    for (index, field) in function.outputs.iter().enumerate() {
        let schema = crate::wire::Schema::from_field(field, &emitter.program.types, policy)
            .expect("checked external output schema");
        let prefix = format!("external_{implementation}_output_{index}");
        output.push_str(&wire::emit_named(&schema, &prefix));
        output_schemas.push((field, schema, prefix));
    }
    let output_names = function
        .outputs
        .iter()
        .map(|field| rust_string(&field.name))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(output, "fn external_{implementation}_outputs(text: &str) -> Result<{}, String> {{\nlet node = dever_runtime::wire::parse(text)?;\nlet fields = node.fields(&[{output_names}])?;\nif fields.len() != {} {{ return Err(\"external Adapter result has missing fields\".into()); }}", rust_type(&output_type(&function.outputs)), function.outputs.len()).expect("string formatting");
    for (index, (field, schema, prefix)) in output_schemas.iter().enumerate() {
        writeln!(output, "let value_{index} = {prefix}_d{}(fields.get({}).ok_or(\"external Adapter result has missing fields\")?)?;", schema.root, rust_string(&field.name)).expect("string formatting");
    }
    let value = match function.outputs.len() {
        0 => "()".into(),
        1 => "value_0".into(),
        count => format!(
            "({})",
            (0..count)
                .map(|index| format!("value_{index}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    writeln!(output, "Ok({value})\n}}").expect("string formatting");

    writeln!(output, "fn external_{implementation}_error(identity: &str, text: &str, location: &'static str) -> Result<AppError, String> {{\nmatch identity {{").expect("string formatting");
    for failure in &contract.failures {
        let crate::types::Shape::Choice(variants) = &emitter.program.types[failure.ty].shape else {
            unreachable!()
        };
        let variant = &variants[failure.variant];
        let identity = format!(
            "{}.{}",
            emitter.program.types[failure.ty].name, variant.name
        );
        writeln!(output, "{} => {{\nlet node = dever_runtime::wire::parse(text)?;\nlet fields = node.fields(&[{}])?;\nif fields.len() != {} {{ return Err(\"external Adapter error has missing fields\".into()); }}", rust_string(&identity), variant.fields.iter().map(|field| rust_string(&field.name)).collect::<Vec<_>>().join(", "), variant.fields.len()).expect("string formatting");
        for (index, field) in variant.fields.iter().enumerate() {
            let schema = crate::wire::Schema::from_field(field, &emitter.program.types, policy)
                .expect("checked external error schema");
            let prefix = format!("external_{implementation}_error_{}_{}", failure.ty, index);
            output.push_str(&wire::emit_named(&schema, &prefix));
            writeln!(output, "let field_{index} = {prefix}_d{}(fields.get({}).ok_or(\"external Adapter error has missing fields\")?)?;", schema.root, rust_string(&field.name)).expect("string formatting");
        }
        let payload = if variant.fields.is_empty() {
            String::new()
        } else {
            format!(
                "({})",
                (0..variant.fields.len())
                    .map(|index| format!("field_{index}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        writeln!(
            output,
            "Ok(AppError::business(AppErrorKind::E{}V{}{payload}, location))\n}},",
            failure.ty, failure.variant
        )
        .expect("string formatting");
    }
    output.push_str(
        "_ => Err(\"external Adapter returned an undeclared business error\".into()),\n}\n}\n",
    );
}
