//! Closed Port dispatch and entry-owned, immutable selected Adapter settings.
use super::*;

pub(super) const DECLARATIONS: &str = "\
declare i32 @dever_rt_v1_adapter_settings_load(ptr, ptr)
declare void @dever_rt_v1_adapter_settings_release(ptr)
declare i32 @dever_rt_v1_adapter_settings_validate_names(ptr, ptr, i64, ptr)
declare i32 @dever_rt_v1_adapter_settings_select(ptr, ptr, ptr, i64, ptr, ptr, ptr, ptr)
";

impl Module<'_> {
    pub(super) fn emit_port_slots(&mut self) {
        let app = self.application.as_ref().unwrap();
        for port in &app.ports {
            writeln!(
                self.declarations,
                "@dever_port_{port} = private global i64 0"
            )
            .unwrap();
        }
        for adapter in &app.adapters {
            let adapter = &self.program.adapters[*adapter];
            if let Some(ty) = adapter.setting {
                writeln!(self.declarations, "@dever_setting_{} = private global %T{ty} zeroinitializer\n@dever_setting_live_{} = private global i1 false", adapter.owner, adapter.owner).unwrap();
            }
        }
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn emit_port_body(
        &mut self,
        function: &crate::hir::Function,
    ) -> Result<bool, String> {
        if function.setting.is_some() {
            let ty = output_type(&function.outputs);
            self.line(format!(
                "call void @dever_clone_{}(ptr @dever_setting_{}, ptr %out)",
                self.module.type_index(&ty),
                function.owner
            ));
            self.exit("0");
            return Ok(true);
        }
        let Some(port) = &function.port else {
            return Ok(false);
        };
        let selected = self.temp();
        self.line(format!(
            "{selected} = load i64, ptr @dever_port_{}",
            self.instance.function
        ));
        let cases = port
            .implementations
            .iter()
            .map(|id| format!("i64 {id}, label %port_{id}"))
            .collect::<Vec<_>>()
            .join(" ");
        self.line(format!(
            "switch i64 {selected}, label %port_invalid [{cases}]"
        ));
        self.start("port_invalid");
        self.line("unreachable");
        let arguments = function
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                parameter
                    .value_type()
                    .map(|ty| (ty.clone(), self.parameter(index)))
            })
            .collect::<Vec<_>>();
        let output = output_type(&function.outputs);
        for implementation in &port.implementations {
            self.start(&format!("port_{implementation}"));
            let target = Specialization {
                function: *implementation,
                handlers: Vec::new(),
            };
            let (status, row) = self.invoke_values(&target, &arguments, &output, function.span);
            self.propagate_status(&status, function.span, false);
            let value = self.temp();
            self.line(format!(
                "{value} = load {}, ptr {row}",
                self.module.ty(&output)
            ));
            self.line(format!(
                "store {} {value}, ptr %out",
                self.module.ty(&output)
            ));
            self.exit("0");
        }
        Ok(true)
    }

    pub(super) fn initialize_ports(&mut self, span: Span) {
        let app = self.module.application.as_ref().unwrap().clone();
        if let Some(index) = app.test {
            for port in app.ports {
                let implementation = self.module.program.tests[index].port_bindings[&port];
                self.line(format!(
                    "store i64 {implementation}, ptr @dever_port_{port}"
                ));
            }
            return;
        }
        if app.ports.is_empty() {
            return;
        }
        let settings = self.protocol_call("dever_rt_v1_adapter_settings_load", vec![], "ptr", span);
        let settings_guard = self.protocol_owner("adapter_settings", &settings);
        let identities = self
            .module
            .program
            .functions
            .iter()
            .filter_map(|function| function.port.as_ref().map(|port| port.identity.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let names = self.module.wire_names(&identities);
        self.wire_write(
            "dever_rt_v1_adapter_settings_validate_names",
            vec![
                format!("ptr {settings}"),
                format!("ptr {names}"),
                format!("i64 {}", identities.len()),
            ],
        );
        let owners = app
            .ports
            .iter()
            .map(|port| self.module.program.functions[*port].owner)
            .collect::<BTreeSet<_>>();
        for owner in owners {
            let adapters = app
                .adapters
                .iter()
                .filter_map(|id| {
                    let adapter = &self.module.program.adapters[*id];
                    (adapter.port_owner == owner).then_some(adapter.clone())
                })
                .collect::<Vec<_>>();
            let names = self.module.wire_names(
                &adapters
                    .iter()
                    .map(|adapter| adapter.name.clone())
                    .collect::<Vec<_>>(),
            );
            let identity = self.module.wire_names(&[adapters[0].identity.clone()]);
            let index = self.entry_slot_ir("i64");
            let setting = self.entry_slot_ir("ptr");
            let present = self.entry_slot_ir("i8");
            self.wire_write(
                "dever_rt_v1_adapter_settings_select",
                vec![
                    format!("ptr {settings}"),
                    format!("ptr {identity}"),
                    format!("ptr {names}"),
                    format!("i64 {}", adapters.len()),
                    format!("ptr {index}"),
                    format!("ptr {setting}"),
                    format!("ptr {present}"),
                ],
            );
            let optional = self.optional_row(
                &Type::Nullable(Box::new(Type::Text)),
                &Type::Text,
                &setting,
                &present,
            );
            let setting_guard =
                self.own_value(&Type::Nullable(Box::new(Type::Text)), &optional, false);
            let has_setting = self.temp();
            self.line(format!(
                "{has_setting} = extractvalue {{ i1, ptr }} {optional}, 0"
            ));
            let selected = self.temp();
            self.line(format!("{selected} = load i64, ptr {index}"));
            let done = self.label("adapter_ready");
            let invalid = self.label("adapter_invalid");
            let labels = adapters
                .iter()
                .map(|_| self.label("adapter_selected"))
                .collect::<Vec<_>>();
            let arms = labels
                .iter()
                .enumerate()
                .map(|(index, label)| format!("i64 {index}, label %{label}"))
                .collect::<Vec<_>>()
                .join(" ");
            self.line(format!("switch i64 {selected}, label %{invalid} [{arms}]"));
            self.start(&invalid);
            self.line("unreachable");
            for (adapter, label) in adapters.iter().zip(labels) {
                self.start(&label);
                let valid = self.label("setting_valid");
                let failed = self.label("setting_invalid");
                self.line(if adapter.setting.is_some() {
                    format!("br i1 {has_setting}, label %{valid}, label %{failed}")
                } else {
                    format!("br i1 {has_setting}, label %{failed}, label %{valid}")
                });
                self.start(&failed);
                self.application_fault(
                    if adapter.setting.is_some() {
                        "selected Adapter requires setting"
                    } else {
                        "selected Adapter does not declare setting"
                    },
                    span,
                );
                self.start(&valid);
                if let Some(id) = adapter.setting {
                    let ty = Type::Named(id);
                    let callback =
                        self.module
                            .wire_callback(&ty, crate::wire::Policy::SettingInput, false);
                    let text = self.temp();
                    self.line(format!("{text} = extractvalue {{ i1, ptr }} {optional}, 1"));
                    let value = self.runtime_call(
                        "dever_rt_v1_wire_decode",
                        vec![
                            format!("ptr {text}"),
                            format!("ptr @dever_type_{}", self.module.type_index(&ty)),
                            format!("ptr @{callback}"),
                        ],
                        &ty,
                        false,
                        None,
                        span,
                    );
                    self.line(format!(
                        "store {} {value}, ptr @dever_setting_{}",
                        self.module.ty(&ty),
                        adapter.owner
                    ));
                    self.line(format!(
                        "store i1 true, ptr @dever_setting_live_{}",
                        adapter.owner
                    ));
                }
                let external_setting = if adapter.setting.is_some() {
                    let text = self.temp();
                    self.line(format!("{text} = extractvalue {{ i1, ptr }} {optional}, 1"));
                    text
                } else {
                    "null".into()
                };
                self.start_external_adapter(adapter, &external_setting, span);
                for (port, implementation) in &adapter.operations {
                    if app.ports.contains(port) {
                        self.line(format!(
                            "store i64 {implementation}, ptr @dever_port_{port}"
                        ));
                    }
                }
                self.line(format!("br label %{done}"));
            }
            self.start(&done);
            self.release_guard(setting_guard);
        }
        self.release_guard(settings_guard);
    }
}
