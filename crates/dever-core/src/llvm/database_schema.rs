//! Static schema metadata shares the native backend's SQL/DDL owner.
use super::*;
use crate::model::{DatabaseOwner, ModelFieldType, ModelSchema, ModelScope, ModelValue};
use crate::native::orm as sql;

pub(super) struct Migration {
    pub bindings: String,
    pub models: String,
    pub errors: String,
    pub count: usize,
}

pub(super) const DECLARATIONS: &str = "\
%dever.db_binding = type { ptr, { ptr, i64 }, i8 }
%dever.db_field = type { { ptr, i64 }, { ptr, i64 }, i8, i8, ptr, ptr }
%dever.db_index = type { { ptr, i64 }, ptr, i64, i8 }
%dever.db_migration = type { { ptr, i64 }, { ptr, i64 }, ptr, i64 }
%dever.db_seed = type { %dever.db_sql, ptr, i64 }
%dever.db_data_migration = type { { ptr, i64 }, i32, %dever.db_seed }
%dever.db_pg_column = type { { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, ptr }
%dever.db_pg_constraint = type { { ptr, i64 }, { ptr, i64 }, i8 }
%dever.db_model = type { { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, { ptr, i64 }, ptr, i64, ptr, i64, ptr, i64, %dever.db_sql, { ptr, i64 }, ptr, i64, ptr, i64, ptr, i64, ptr, i64, ptr, i64 }
declare ptr @dever_rt_v1_db_migrate_models(ptr, ptr, ptr, i64, ptr)
declare ptr @dever_rt_v1_api_migrate_models(ptr, ptr, ptr, ptr, i64, i64, ptr)
";

impl Module<'_> {
    fn database_name(&mut self, name: &str) -> String {
        let (pointer, length) = self.text_literal(name);
        format!("{{ ptr, i64 }} {{ ptr {pointer}, i64 {length} }}")
    }

    fn database_optional_name(&mut self, name: Option<&str>) -> String {
        name.map(|name| self.wire_names(&[name.into()]))
            .unwrap_or_else(|| "null".into())
    }

    fn database_array(&mut self, ty: &str, values: &[String]) -> String {
        if values.is_empty() {
            return "null".into();
        }
        let name = format!("@dever_db_metadata_{}", self.next_literal);
        self.next_literal += 1;
        let body = values
            .iter()
            .map(|value| format!("{ty} {value}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            self.declarations,
            "{name} = private constant [{} x {ty}] [{body}]",
            values.len()
        )
        .unwrap();
        name
    }

    pub(super) fn database_binding(&mut self, index: usize) -> String {
        let (explicit, root) = self.program.database_selector(DatabaseOwner::Model(index));
        let explicit = explicit.map(str::to_owned);
        let root = root.to_owned();
        let explicit = self.database_optional_name(explicit.as_deref());
        let root = self.database_name(&root);
        let tenant = u8::from(self.program.models[index].scope == ModelScope::Tenant);
        format!("{{ ptr {explicit}, {root}, i8 {tenant} }}")
    }

    pub(super) fn emit_database_slots(&mut self) {
        self.declarations.push_str(DECLARATIONS);
        self.declarations.push_str("@dever_database_session = private global ptr null\ndefine internal void @dever_db_append_cause(ptr %fault, ptr %message) {\nentry:\n  %cause = getelementptr %dever.fault, ptr %fault, i32 0, i32 8\n  call void @dever_rt_v1_db_cause_append(ptr %cause, ptr %message)\n  ret void\n}\n");
        for index in 0..self.program.models.len() {
            writeln!(
                self.declarations,
                "@dever_database_{index} = private global ptr null"
            )
            .unwrap();
        }
    }

    fn database_model(&mut self, model: &ModelSchema) -> String {
        let fields = model
            .fields
            .iter()
            .map(|field| {
                let name = self.database_name(&field.name);
                let ty = self.database_name(&crate::model::field_type(&field.ty));
                let default = field.default.as_ref().map(crate::model::value);
                let default = self.database_optional_name(default.as_deref());
                let rename = self.database_optional_name(field.rename_from.as_deref());
                format!(
                    "{{ {name}, {ty}, i8 {}, i8 {}, ptr {default}, ptr {rename} }}",
                    u8::from(field.nullable),
                    u8::from(field.generated)
                )
            })
            .collect::<Vec<_>>();
        let fields = self.database_array("%dever.db_field", &fields);
        let indexes = model
            .indexes
            .iter()
            .filter(|index| index.fields.as_slice() != ["id"])
            .map(|index| {
                let name = self.database_name(&sql::index_name(model, index));
                let fields = self.wire_names(&index.fields);
                format!(
                    "{{ {name}, ptr {fields}, i64 {}, i8 {} }}",
                    index.fields.len(),
                    u8::from(index.unique)
                )
            })
            .collect::<Vec<_>>();
        let index_count = indexes.len();
        let indexes = self.database_array("%dever.db_index", &indexes);
        let migrations = model
            .migrations
            .iter()
            .map(|migration| {
                let name = self.database_name(&migration.name);
                let revision = self.database_name(&migration.revision());
                let drops = self.wire_names(&migration.drops);
                format!(
                    "{{ {name}, {revision}, ptr {drops}, i64 {} }}",
                    migration.drops.len()
                )
            })
            .collect::<Vec<_>>();
        let migrations = self.database_array("%dever.db_migration", &migrations);
        let constraints = sql::postgres_constraints(model, &self.program.models);
        let pg_table = sql::postgres_table(model, &constraints, &model.table);
        let sqlite_table = sql::sqlite_table(model, &self.program.models, &model.table);
        let sqlite_table = self.database_name(&sqlite_table);
        let pg_table = self.database_name(&pg_table);
        let temporary = format!("_dever_{}_{}", model.table, &model.revision[..8]);
        let temporary = sql::sqlite_table(model, &self.program.models, &temporary);
        let temporary = self.database_name(&temporary);
        let create_indexes = sql::sqlite_indexes(model)
            .iter()
            .map(|query| {
                let name = self.database_name(query);
                format!("{{ {name}, {name} }}")
            })
            .collect::<Vec<_>>();
        let create_index_count = create_indexes.len();
        let create_indexes = self.database_array("%dever.db_sql", &create_indexes);
        let columns = model
            .fields
            .iter()
            .map(|field| {
                let name = self.database_name(&field.name);
                let definition = self.database_name(&sql::postgres_column(field));
                let ty = self.database_name(&sql::postgres_type(&field.ty));
                let default = sql::postgres_field_default(field);
                let default = self.database_optional_name(default.as_deref());
                format!("{{ {name}, {definition}, {ty}, ptr {default} }}")
            })
            .collect::<Vec<_>>();
        let columns = self.database_array("%dever.db_pg_column", &columns);
        let constraints = constraints
            .iter()
            .map(|(name, definition, foreign)| {
                let name = self.database_name(name);
                let definition = self.database_name(definition);
                format!("{{ {name}, {definition}, i8 {} }}", u8::from(*foreign))
            })
            .collect::<Vec<_>>();
        let constraint_count = constraints.len();
        let constraints = self.database_array("%dever.db_pg_constraint", &constraints);
        let name = self.database_name(&model.package);
        let table = self.database_name(&model.table);
        let revision = self.database_name(&model.revision);
        let seed_revision = self.database_name(&model.seed_revision);
        format!(
            "{{ {name}, {table}, {revision}, {seed_revision}, ptr {fields}, i64 {}, ptr {indexes}, i64 {index_count}, ptr {migrations}, i64 {}, %dever.db_sql {{ {sqlite_table}, {pg_table} }}, {temporary}, ptr {create_indexes}, i64 {create_index_count}, ptr {columns}, i64 {}, ptr {constraints}, i64 {constraint_count}, ptr null, i64 0, ptr null, i64 0 }}",
            model.fields.len(),
            model.migrations.len(),
            model.fields.len()
        )
    }
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn initialize_database(
        &mut self,
        span: Span,
        service_start: &str,
    ) -> Option<Migration> {
        if !self.module.database_enabled() {
            if self.module.api_enabled() {
                self.initialize_api("null", 0, "null", 0, service_start, span);
            }
            return None;
        }
        let models = self.module.program.models.clone();
        let mut bindings = (0..models.len())
            .map(|index| self.module.database_binding(index))
            .collect::<Vec<_>>();
        for job in self.module.program.jobs.clone() {
            bindings.push(self.module.owner_binding(DatabaseOwner::Job(job.function)));
        }
        let binding_count = bindings.len();
        let bindings = self.module.database_array("%dever.db_binding", &bindings);
        let mut transactions = self
            .module
            .instances
            .iter()
            .filter(|instance| {
                self.module.program.functions[instance.function].kind == FunctionKind::Transaction
            })
            .cloned()
            .collect::<Vec<_>>();
        transactions.extend(
            self.module
                .program
                .api_commands
                .iter()
                .map(|command| Specialization {
                    function: command.function,
                    handlers: Vec::new(),
                })
                .filter(|instance| specialize::writes_database(self.module.program, instance)),
        );
        transactions.extend(
            self.module
                .program
                .api_routes
                .iter()
                .filter(|route| matches!(route.method, "POST" | "PUT" | "DELETE"))
                .map(|route| Specialization {
                    function: route.function,
                    handlers: Vec::new(),
                })
                .filter(|instance| specialize::writes_database(self.module.program, instance)),
        );
        if let Some(application) = &self.module.application {
            transactions.extend(
                application
                    .rest_handlers
                    .iter()
                    .filter(|handler| handler.method != "GET")
                    .map(|handler| Specialization {
                        function: handler.function,
                        handlers: Vec::new(),
                    }),
            );
        }
        transactions.extend(
            self.module
                .program
                .jobs
                .iter()
                .map(|job| Specialization {
                    function: job.function,
                    handlers: Vec::new(),
                })
                .filter(|instance| specialize::writes_database(self.module.program, instance)),
        );
        let groups = transactions
            .iter()
            .map(|instance| {
                let owners = specialize::database_effects(self.module.program, instance);
                let values = owners
                    .iter()
                    .map(|owner| self.module.owner_binding(*owner))
                    .collect::<Vec<_>>();
                let pointer = self.module.database_array("%dever.db_binding", &values);
                format!("{{ ptr {pointer}, i64 {} }}", values.len())
            })
            .collect::<Vec<_>>();
        let transactions = self.module.database_array("{ ptr, i64 }", &groups);
        let session = if self.module.api_enabled() {
            let api_session = self.initialize_api(
                &bindings,
                binding_count,
                &transactions,
                groups.len(),
                service_start,
                span,
            );
            self.protocol_call(
                "dever_rt_v1_api_session_database",
                vec![format!("ptr {api_session}")],
                "ptr",
                span,
            )
        } else {
            self.protocol_call(
                "dever_rt_v1_db_session_new",
                vec![
                    format!("ptr {bindings}"),
                    format!("i64 {binding_count}"),
                    format!("ptr {transactions}"),
                    format!("i64 {}", groups.len()),
                ],
                "ptr",
                span,
            )
        };
        self.line(format!("store ptr {session}, ptr @dever_database_session"));
        self.initialize_jobs(span);
        if !self.module.api_enabled() {
            let prepare =
                self.database_operation("db_prepare", vec![format!("ptr {session}")], span);
            self.await_operation(&prepare, &Type::Unit, span, false);
        }
        let database_array = self.entry_slot_ir(&format!("[{} x ptr]", models.len()));
        let model_array = self.entry_slot_ir(&format!("[{} x %dever.db_model]", models.len()));
        let error_array = self.entry_slot_ir(&format!("[{} x ptr]", models.len()));
        for (index, model) in models.iter().enumerate() {
            let span = self.module.program.types[model.record].span;
            let binding = self.temp();
            self.line(format!("{binding} = getelementptr [{} x %dever.db_binding], ptr {bindings}, i32 0, i64 {index}", models.len()));
            let database = if self.module.api_enabled() {
                "null".into()
            } else {
                self.database_call(
                    "db_select",
                    vec![format!("ptr {session}"), format!("ptr {binding}")],
                    "ptr",
                    span,
                )
            };
            self.line(format!("store ptr {database}, ptr @dever_database_{index}"));
            let database_slot = self.temp();
            self.line(format!("{database_slot} = getelementptr [{} x ptr], ptr {database_array}, i32 0, i64 {index}", models.len()));
            self.line(format!("store ptr {database}, ptr {database_slot}"));
            let model_slot = self.temp();
            self.line(format!("{model_slot} = getelementptr [{} x %dever.db_model], ptr {model_array}, i32 0, i64 {index}", models.len()));
            let value = self.module.database_model(model);
            self.line(format!("store %dever.db_model {value}, ptr {model_slot}"));
            self.database_seeds(model, &model_slot, span);
            let errors = self.database_errors(span);
            let error_slot = self.temp();
            self.line(format!(
                "{error_slot} = getelementptr [{} x ptr], ptr {error_array}, i32 0, i64 {index}",
                models.len()
            ));
            self.line(format!("store ptr {errors}, ptr {error_slot}"));
        }
        let operation = if self.module.api_enabled() {
            let api_session = self.temp();
            self.line(format!("{api_session} = load ptr, ptr @dever_api_session"));
            let mut arguments = vec![
                format!("ptr {api_session}"),
                format!("ptr {bindings}"),
                format!("ptr {model_array}"),
                format!("ptr {error_array}"),
                format!("i64 {}", models.len()),
            ];
            let symbol = if self.module.jobs_enabled() {
                let errors = self.database_errors(span);
                arguments.push(format!("ptr {errors}"));
                "api_migrate_application"
            } else {
                "api_migrate_models"
            };
            arguments.push("i64 0".into());
            self.async_operation(symbol, arguments, span)
        } else {
            self.async_operation(
                "db_migrate_models",
                vec![
                    format!("ptr {database_array}"),
                    format!("ptr {model_array}"),
                    format!("ptr {error_array}"),
                    format!("i64 {}", models.len()),
                ],
                span,
            )
        };
        self.await_operation(&operation, &Type::Unit, span, false);
        Some(Migration {
            bindings,
            models: model_array,
            errors: error_array,
            count: models.len(),
        })
    }

    fn database_seed_value(
        &mut self,
        storage: &ModelFieldType,
        value: &ModelValue,
        span: Span,
    ) -> (Type, ModelFieldType, String) {
        let (ty, value) = match value {
            ModelValue::Null => (
                Type::Nullable(Box::new(Type::Int)),
                "zeroinitializer".into(),
            ),
            ModelValue::Bool(value) => (Type::Bool, u8::from(*value).to_string()),
            ModelValue::Number(value) => match storage {
                ModelFieldType::Float => (
                    Type::Float,
                    format!(
                        "0x{:016X}",
                        value.parse::<f64>().expect("checked seed Float").to_bits()
                    ),
                ),
                ModelFieldType::Decimal { .. } => {
                    let decimal =
                        dever_runtime::number::parse_decimal(value).expect("checked seed Decimal");
                    let bytes = decimal.to_bytes();
                    let low = i64::from_le_bytes(bytes[..8].try_into().unwrap());
                    let high = i64::from_le_bytes(bytes[8..].try_into().unwrap());
                    (Type::Decimal, format!("{{ i64 {low}, i64 {high} }}"))
                }
                _ => (Type::Int, value.clone()),
            },
            ModelValue::Text(value) | ModelValue::Choice(value) => {
                let value = if matches!(storage, ModelFieldType::Choice(_)) {
                    value.rsplit('.').next().unwrap()
                } else {
                    value.as_str()
                };
                let text = self.application_text(value, span);
                if matches!(storage, ModelFieldType::Uuid) {
                    let uuid = self.uuid_from_literal_text(&text, span);
                    self.own_value(&Type::Uuid, &uuid, false);
                    (Type::Uuid, uuid)
                } else {
                    (Type::Text, text)
                }
            }
        };
        (ty, storage.clone(), value)
    }

    fn database_seeds(&mut self, model: &ModelSchema, model_slot: &str, span: Span) {
        let mut seeds = Vec::new();
        for seed in &model.seed {
            let mut columns = Vec::new();
            let mut values = Vec::new();
            for (name, value) in &seed.fields {
                let storage = &model
                    .fields
                    .iter()
                    .find(|field| field.name == *name)
                    .unwrap()
                    .ty;
                columns.push(sql::quoted(name));
                values.push(self.database_seed_value(storage, value, span));
            }
            for field in model
                .fields
                .iter()
                .filter(|field| field.generated && field.ty == ModelFieldType::Uuid)
            {
                columns.push(sql::quoted(&field.name));
                let uuid = self.database_call("db_uuid_new", vec![], "ptr", span);
                self.own_value(&Type::Uuid, &uuid, false);
                values.push((Type::Uuid, ModelFieldType::Uuid, uuid));
            }
            let placeholders = values
                .iter()
                .enumerate()
                .map(|(index, (_, storage, _))| sql::parameter_sql(index + 1, storage))
                .collect::<Vec<_>>()
                .join(", ");
            let conflict = seed
                .identity
                .iter()
                .map(|name| sql::quoted(name))
                .collect::<Vec<_>>()
                .join(", ");
            let statement = format!(
                "INSERT INTO {} ({}) VALUES ({placeholders}) ON CONFLICT ({conflict}) DO NOTHING",
                sql::quoted(&model.table),
                columns.join(", ")
            );
            let query = self.module.planned_sql(&statement);
            seeds.push(self.database_seed(&query, &values, span));
        }
        self.database_schema_array(model_slot, 18, "%dever.db_seed", &seeds);
        let mut migrations = Vec::new();
        for migration in &model.migrations {
            for statement in &migration.statements {
                let values = statement
                    .parameters
                    .iter()
                    .map(|(storage, value)| self.database_seed_value(storage, value, span))
                    .collect::<Vec<_>>();
                let query = self
                    .module
                    .database_sql(&statement.sqlite, &statement.postgres);
                let seed = self.database_seed(&query, &values, span);
                let name = self.module.database_name(&migration.name);
                let phase = u8::from(matches!(
                    statement.phase,
                    crate::syntax::MigrationPhase::After
                ));
                let value = self.temp();
                self.line(format!("{value} = insertvalue %dever.db_data_migration {{ {name}, i32 {phase}, %dever.db_seed zeroinitializer }}, %dever.db_seed {seed}, 2"));
                migrations.push(value);
            }
        }
        self.database_schema_array(model_slot, 20, "%dever.db_data_migration", &migrations);
    }

    fn database_seed(
        &mut self,
        query: &str,
        values: &[(Type, ModelFieldType, String)],
        span: Span,
    ) -> String {
        let parameters = self.database_parameters(values, span);
        let sql = self.temp();
        self.line(format!("{sql} = load %dever.db_sql, ptr {query}"));
        let mut result = "zeroinitializer".to_string();
        for (index, ty, value) in [
            (0, "%dever.db_sql", sql),
            (1, "ptr", parameters),
            (2, "i64", values.len().to_string()),
        ] {
            let next = self.temp();
            self.line(format!(
                "{next} = insertvalue %dever.db_seed {result}, {ty} {value}, {index}"
            ));
            result = next;
        }
        result
    }

    fn database_schema_array(&mut self, model: &str, field: usize, ty: &str, values: &[String]) {
        if values.is_empty() {
            return;
        }
        let count = values.len();
        let array = self.entry_slot_ir(&format!("[{count} x {ty}]"));
        for (index, value) in values.iter().enumerate() {
            let slot = self.temp();
            self.line(format!(
                "{slot} = getelementptr [{count} x {ty}], ptr {array}, i32 0, i64 {index}"
            ));
            self.line(format!("store {ty} {value}, ptr {slot}"));
        }
        let pointer = self.temp();
        self.line(format!(
            "{pointer} = getelementptr %dever.db_model, ptr {model}, i32 0, i32 {field}"
        ));
        self.line(format!("store ptr {array}, ptr {pointer}"));
        let length = self.temp();
        self.line(format!(
            "{length} = getelementptr %dever.db_model, ptr {model}, i32 0, i32 {}",
            field + 1
        ));
        self.line(format!("store i64 {count}, ptr {length}"));
    }
}
