//! Normalize the existing application CLI into one typed CMD dispatch path.
use super::*;

impl FunctionEmitter<'_, '_> {
    pub(super) fn application_command_arguments(
        &mut self,
        first: &str,
        cursor: &str,
        length: &str,
        migration: Option<&database_schema::Migration>,
        span: Span,
    ) -> Result<(String, String, String), String> {
        let name_slot = self.entry_slot(&Type::Text);
        let body_slot = self.entry_slot(&Type::Text);
        let tenant_slot = self.entry_slot(&Type::Int);
        self.line(format!("store i64 0, ptr {tenant_slot}"));
        if self.module.api_enabled() {
            self.application_management(first, cursor, length, migration, span)?;
            let flag = self.application_text("--dever-tenant-cmd", span);
            let matches = self.equals(&Type::Text, first, &flag)?;
            self.line(format!(
                "br i1 {matches}, label %command_tenant, label %command_plain"
            ));
            self.start("command_tenant");
            self.application_arity(length, 4, "expected --dever-tenant-cmd, one positive tenant id, CMD name, and one JSON object", span);
            let tenant = self.application_positive_argument(
                cursor,
                "tenant id must be a positive integer",
                span,
            );
            let name = self.application_argument(cursor, span);
            let body = self.application_argument(cursor, span);
            self.line(format!("store ptr {name}, ptr {name_slot}"));
            self.line(format!("store ptr {body}, ptr {body_slot}"));
            self.line(format!("store i64 {tenant}, ptr {tenant_slot}"));
            self.line("br label %command_dispatch");
            self.start("command_plain");
        }
        self.application_arity(length, 2, "expected CMD name and one JSON object", span);
        let body = self.application_argument(cursor, span);
        self.line(format!("store ptr {first}, ptr {name_slot}"));
        self.line(format!("store ptr {body}, ptr {body_slot}"));
        self.line("br label %command_dispatch");
        self.start("command_dispatch");
        let name = self.temp();
        let body = self.temp();
        let tenant = self.temp();
        self.line(format!("{name} = load ptr, ptr {name_slot}"));
        self.line(format!("{body} = load ptr, ptr {body_slot}"));
        self.line(format!("{tenant} = load i64, ptr {tenant_slot}"));
        Ok((name, body, tenant))
    }

    fn application_management(
        &mut self,
        first: &str,
        cursor: &str,
        length: &str,
        migration: Option<&database_schema::Migration>,
        span: Span,
    ) -> Result<(), String> {
        for (flag, arity) in [
            ("--dever-tenant-migrate", 2),
            ("--dever-tenant-owner", 4),
            ("--dever-tenant-component", 4),
        ] {
            if flag == "--dever-tenant-owner" && self.module.program.permissions.is_empty() {
                continue;
            }
            let expected = self.application_text(flag, span);
            let matches = self.equals(&Type::Text, first, &expected)?;
            let matched = self.label("management");
            let next = self.label("management_next");
            self.line(format!("br i1 {matches}, label %{matched}, label %{next}"));
            self.start(&matched);
            let usage = match flag {
                "--dever-tenant-migrate" => {
                    "expected --dever-tenant-migrate and one positive tenant id"
                }
                "--dever-tenant-owner" => {
                    "expected --dever-tenant-owner, one positive tenant id, one site, and one positive user id"
                }
                _ => {
                    "expected --dever-tenant-component, one positive tenant id, enable or disable, and one component"
                }
            };
            self.application_arity(length, arity, usage, span);
            let tenant = self.application_positive_argument(
                cursor,
                "tenant id must be a positive integer",
                span,
            );
            let session = self.application_session();
            match flag {
                "--dever-tenant-migrate" => {
                    if let Some(migration) = migration {
                        let mut arguments = vec![
                            format!("ptr {session}"),
                            format!("ptr {}", migration.bindings),
                            format!("ptr {}", migration.models),
                            format!("ptr {}", migration.errors),
                            format!("i64 {}", migration.count),
                        ];
                        let symbol = if self.module.jobs_enabled() {
                            let errors = self.database_errors(span);
                            arguments.push(format!("ptr {errors}"));
                            "api_migrate_application"
                        } else {
                            "api_migrate_models"
                        };
                        arguments.push(format!("i64 {tenant}"));
                        let operation = self.async_operation(symbol, arguments, span);
                        self.await_operation(&operation, &Type::Unit, span, false);
                        self.exit("0");
                    } else {
                        self.application_fault("application has no database runtime", span);
                    }
                }
                "--dever-tenant-owner" => {
                    let site = self.application_argument(cursor, span);
                    let user = self.application_positive_argument(
                        cursor,
                        "user id must be a positive integer",
                        span,
                    );
                    let operation = self.async_operation(
                        "api_tenant_owner",
                        vec![
                            format!("ptr {session}"),
                            format!("i64 {tenant}"),
                            format!("ptr {site}"),
                            format!("i64 {user}"),
                        ],
                        span,
                    );
                    self.await_operation(&operation, &Type::Unit, span, false);
                    self.exit("0");
                }
                _ => {
                    if self.module.program.tenant_components.is_empty() {
                        self.application_fault("application has no tenant components", span);
                    } else {
                        let action = self.application_argument(cursor, span);
                        let component = self.application_argument(cursor, span);
                        let enable = self.application_text("enable", span);
                        let disable = self.application_text("disable", span);
                        let enabled = self.equals(&Type::Text, &action, &enable)?;
                        let disabled = self.equals(&Type::Text, &action, &disable)?;
                        let valid = self.temp();
                        self.line(format!("{valid} = or i1 {enabled}, {disabled}"));
                        self.application_require(
                            &valid,
                            "operation must be enable or disable",
                            span,
                        );
                        let byte = self.temp();
                        self.line(format!("{byte} = zext i1 {enabled} to i8"));
                        let manifest = self
                            .module
                            .wire_names(&self.module.program.tenant_components.clone());
                        let operation = self.async_operation(
                            "api_tenant_component",
                            vec![
                                format!("ptr {session}"),
                                format!("i64 {tenant}"),
                                format!("ptr {component}"),
                                format!("i8 {byte}"),
                                format!("ptr {manifest}"),
                                format!("i64 {}", self.module.program.tenant_components.len()),
                            ],
                            span,
                        );
                        self.await_operation(&operation, &Type::Unit, span, false);
                        self.exit("0");
                    }
                }
            }
            self.start(&next);
        }
        Ok(())
    }

    pub(super) fn application_invoke_command(
        &mut self,
        command: &crate::hir::ApiCommand,
        target: &Specialization,
        body: &str,
        tenant: &str,
        span: Span,
    ) {
        if self.module.api_enabled() {
            let selected = self.temp();
            self.line(format!("{selected} = icmp sgt i64 {tenant}, 0"));
            let scoped = self.label("command_scoped");
            let plain = self.label("command_unscoped");
            self.line(format!("br i1 {selected}, label %{scoped}, label %{plain}"));
            self.start(&scoped);
            if command.components.is_empty() {
                self.application_fault(
                    "selected CMD does not use tenant storage; remove --tenant",
                    span,
                );
            } else {
                let configured = self.application_has_tenant(span);
                self.application_require(&configured, "tenant storage is not configured", span);
                let input =
                    asynchronous::input_type(&self.module.program.functions[target.function]);
                let row = self.temp();
                self.line(format!(
                    "{row} = insertvalue {} zeroinitializer, ptr {body}, 0",
                    self.module.ty(&input)
                ));
                let pointer = self.row_pointer(&input, &row);
                if self.module.jobs_enabled() {
                    self.invoke_system(target, &pointer, tenant, span);
                } else {
                    let function = self.module.names[target].clone();
                    let operation = self.async_operation(
                        "api_tenant_scope",
                        vec![
                            format!("i64 {tenant}"),
                            format!("ptr @dever_async_{function}"),
                            format!("ptr {pointer}"),
                        ],
                        span,
                    );
                    self.await_operation(&operation, &Type::Unit, span, false);
                }
                self.exit("0");
            }
            self.start(&plain);
            if !command.components.is_empty() {
                let configured = self.application_has_tenant(span);
                let ordinary = self.temp();
                self.line(format!("{ordinary} = xor i1 {configured}, true"));
                self.application_require(
                    &ordinary,
                    "selected CMD requires an explicit tenant; use dever run --tenant",
                    span,
                );
            }
        }
        if self.module.jobs_enabled() {
            let input = asynchronous::input_type(&self.module.program.functions[target.function]);
            let row = self.temp();
            self.line(format!(
                "{row} = insertvalue {} zeroinitializer, ptr {body}, 0",
                self.module.ty(&input)
            ));
            let pointer = self.row_pointer(&input, &row);
            self.invoke_system(target, &pointer, "0", span);
        } else {
            let (status, _) =
                self.invoke_values(target, &[(Type::Text, body.into())], &Type::Unit, span);
            self.propagate_status(&status, span, false);
        }
    }

    pub(super) fn application_require_components(&mut self, components: &[String], span: Span) {
        if !self.module.api_enabled() || components.is_empty() {
            return;
        }
        let session = self.application_session();
        let names = self.module.wire_names(components);
        let manifest = self
            .module
            .wire_names(&self.module.program.tenant_components.clone());
        let operation = self.async_operation(
            "api_require_components",
            vec![
                format!("ptr {session}"),
                format!("ptr {names}"),
                format!("i64 {}", components.len()),
                format!("ptr {manifest}"),
                format!("i64 {}", self.module.program.tenant_components.len()),
            ],
            span,
        );
        self.await_operation(&operation, &Type::Unit, span, false);
    }

    fn application_session(&mut self) -> String {
        let session = self.temp();
        self.line(format!("{session} = load ptr, ptr @dever_api_session"));
        session
    }

    fn application_has_tenant(&mut self, span: Span) -> String {
        let session = self.application_session();
        self.runtime_call(
            "dever_rt_v1_api_has_tenant",
            vec![format!("ptr {session}")],
            &Type::Bool,
            false,
            Some("i8"),
            span,
        )
    }

    fn application_positive_argument(&mut self, cursor: &str, message: &str, span: Span) -> String {
        let text = self.application_argument(cursor, span);
        let optional = self.runtime_call(
            "dever_rt_v1_int_parse",
            vec![format!("ptr {text}")],
            &Type::Nullable(Box::new(Type::Int)),
            true,
            None,
            span,
        );
        let present = self.temp();
        let number = self.temp();
        let positive = self.temp();
        let valid = self.temp();
        self.line(format!(
            "{present} = extractvalue {{ i1, i64 }} {optional}, 0"
        ));
        self.line(format!(
            "{number} = extractvalue {{ i1, i64 }} {optional}, 1"
        ));
        self.line(format!("{positive} = icmp sgt i64 {number}, 0"));
        self.line(format!("{valid} = and i1 {present}, {positive}"));
        self.application_require(&valid, message, span);
        number
    }

    fn application_arity(&mut self, length: &str, expected: i64, message: &str, span: Span) {
        let valid = self.temp();
        self.line(format!("{valid} = icmp eq i64 {length}, {expected}"));
        self.application_require(&valid, message, span);
    }

    pub(super) fn application_require(&mut self, valid: &str, message: &str, span: Span) {
        let success = self.label("argument_valid");
        let invalid = self.label("argument_invalid");
        self.line(format!("br i1 {valid}, label %{success}, label %{invalid}"));
        self.start(&invalid);
        self.application_fault(message, span);
        self.start(&success);
    }
}
