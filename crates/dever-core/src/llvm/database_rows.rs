//! Concrete Model/SQL row codecs and borrowed persistence parameter arrays.
use super::*;
use crate::model::ModelFieldType;

impl FunctionEmitter<'_, '_> {
    pub(super) fn database_parameters(
        &mut self,
        values: &[(Type, ModelFieldType, String)],
        span: Span,
    ) -> String {
        let count = values.len();
        let pointer = self.entry_slot_ir(&format!("[{count} x %dever.db_value]"));
        for (index, (ty, storage, value)) in values.iter().enumerate() {
            let encoded = self.database_parameter(ty, storage, value, span);
            let destination = self.temp();
            self.line(format!("{destination} = getelementptr [{count} x %dever.db_value], ptr {pointer}, i32 0, i64 {index}"));
            self.line(format!(
                "store %dever.db_value {encoded}, ptr {destination}"
            ));
        }
        pointer
    }

    fn database_parameter(
        &mut self,
        ty: &Type,
        storage: &ModelFieldType,
        value: &str,
        span: Span,
    ) -> String {
        if let Type::Nullable(inner) = ty {
            let result = self.entry_slot_ir("%dever.db_value");
            self.line(format!(
                "store %dever.db_value zeroinitializer, ptr {result}"
            ));
            let present = self.temp();
            self.line(format!(
                "{present} = extractvalue {} {value}, 0",
                self.module.ty(ty)
            ));
            let some = self.label("database_parameter_some");
            let done = self.label("database_parameter_done");
            self.line(format!("br i1 {present}, label %{some}, label %{done}"));
            self.start(&some);
            let payload = self.temp();
            self.line(format!(
                "{payload} = extractvalue {} {value}, 1",
                self.module.ty(ty)
            ));
            let encoded = self.database_parameter(inner, storage, &payload, span);
            self.line(format!("store %dever.db_value {encoded}, ptr {result}"));
            self.line(format!("br label %{done}"));
            self.start(&done);
            let encoded = self.temp();
            self.line(format!("{encoded} = load %dever.db_value, ptr {result}"));
            return encoded;
        }
        let (kind, field, layout, value) = match ty {
            Type::Bool => {
                let integer = self.temp();
                self.line(format!("{integer} = zext i1 {value} to i64"));
                (1, 3, "i64", integer)
            }
            Type::Float => (3, 4, "double", value.into()),
            Type::Decimal => (4, 5, "{ i64, i64 }", value.into()),
            Type::Text | Type::Json => (5, 6, "ptr", value.into()),
            Type::Bytes => (6, 6, "ptr", value.into()),
            Type::Uuid => (7, 6, "ptr", value.into()),
            Type::Named(id) if self.module.program.types[*id].kind != DefinitionKind::ModelId => {
                let Shape::Choice(variants) = self.module.program.types[*id].shape.clone() else {
                    unreachable!()
                };
                let slot = self.entry_slot(&Type::Text);
                let tag = self.temp();
                self.line(format!("{tag} = extractvalue %T{id} {value}, 0"));
                let done = self.label("database_choice_done");
                let arms = variants
                    .iter()
                    .enumerate()
                    .map(|(index, _)| (index, self.label("database_choice")))
                    .collect::<Vec<_>>();
                let cases = arms
                    .iter()
                    .map(|(index, block)| format!("i32 {index}, label %{block}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let invalid = self.label("database_choice_invalid");
                self.line(format!("switch i32 {tag}, label %{invalid} [{cases}]"));
                self.start(&invalid);
                self.line("unreachable");
                for (index, block) in arms {
                    self.start(&block);
                    let text = self.application_text(&variants[index].name, span);
                    self.line(format!("store ptr {text}, ptr {slot}"));
                    self.line(format!("br label %{done}"));
                }
                self.start(&done);
                let text = self.temp();
                self.line(format!("{text} = load ptr, ptr {slot}"));
                (5, 6, "ptr", text)
            }
            _ => (2, 3, "i64", value.into()),
        };
        let tagged = self.temp();
        self.line(format!(
            "{tagged} = insertvalue %dever.db_value zeroinitializer, i32 {kind}, 0"
        ));
        let mut result = self.temp();
        self.line(format!(
            "{result} = insertvalue %dever.db_value {tagged}, {layout} {value}, {field}"
        ));
        if let ModelFieldType::Decimal { precision, scale } = storage {
            for (field, value) in [(1, precision), (2, scale)] {
                let next = self.temp();
                self.line(format!(
                    "{next} = insertvalue %dever.db_value {result}, i32 {value}, {field}"
                ));
                result = next;
            }
        }
        result
    }

    pub(super) fn database_scalar(
        &mut self,
        row: &str,
        index: usize,
        ty: &Type,
        span: Span,
    ) -> String {
        let arguments = vec![format!("ptr {row}"), format!("i64 {index}")];
        if let Type::Nullable(inner) = ty {
            let null = self.database_call("db_row_null", arguments, "i8", span);
            let absent = self.temp();
            self.line(format!("{absent} = icmp ne i8 {null}, 0"));
            let slot = self.entry_slot(ty);
            self.line(format!(
                "store {} zeroinitializer, ptr {slot}",
                self.module.ty(ty)
            ));
            let some = self.label("database_column_some");
            let done = self.label("database_column_done");
            self.line(format!("br i1 {absent}, label %{done}, label %{some}"));
            self.start(&some);
            let value = self.database_scalar(row, index, inner, span);
            let present = self.temp();
            self.line(format!(
                "{present} = insertvalue {} zeroinitializer, i1 1, 0",
                self.module.ty(ty)
            ));
            let wrapped = self.temp();
            self.line(format!(
                "{wrapped} = insertvalue {} {present}, {} {value}, 1",
                self.module.ty(ty),
                self.module.ty(inner)
            ));
            self.line(format!(
                "store {} {wrapped}, ptr {slot}",
                self.module.ty(ty)
            ));
            self.line(format!("br label %{done}"));
            self.start(&done);
            let value = self.temp();
            self.line(format!("{value} = load {}, ptr {slot}", self.module.ty(ty)));
            return value;
        }
        let kind = match ty {
            Type::Bool => "bool",
            Type::Float => "float",
            Type::Decimal => "decimal",
            Type::Text | Type::Json => "text",
            Type::Bytes => "bytes",
            Type::Uuid => "uuid",
            Type::Named(id) if self.module.program.types[*id].kind != DefinitionKind::ModelId => {
                "text"
            }
            _ => "int",
        };
        let choice = matches!(ty, Type::Named(id) if self.module.program.types[*id].kind != DefinitionKind::ModelId);
        let layout = if *ty == Type::Bool {
            "i8".into()
        } else if choice {
            "ptr".into()
        } else {
            self.module.ty(ty)
        };
        let value = self.database_call(&format!("db_row_{kind}"), arguments, &layout, span);
        if *ty == Type::Bool {
            let boolean = self.temp();
            self.line(format!("{boolean} = icmp ne i8 {value}, 0"));
            return boolean;
        }
        if choice {
            let Type::Named(id) = ty else { unreachable!() };
            let Shape::Choice(variants) = self.module.program.types[*id].shape.clone() else {
                unreachable!()
            };
            let owner = self.own_value(&Type::Text, &value, false);
            let output = self.entry_slot(ty);
            let done = self.label("database_choice_read");
            for (index, variant) in variants.iter().enumerate() {
                let expected = self.application_text(&variant.name, span);
                let equal = self.temp();
                self.line(format!(
                    "{equal} = call i8 @dever_rt_v1_text_equal(ptr {value}, ptr {expected})"
                ));
                let matched = self.temp();
                self.line(format!("{matched} = icmp ne i8 {equal}, 0"));
                let success = self.label("database_choice_found");
                let next = self.label("database_choice_next");
                self.line(format!("br i1 {matched}, label %{success}, label %{next}"));
                self.start(&success);
                self.write_variant(*id, index, &[], &output);
                self.line(format!("br label %{done}"));
                self.start(&next);
            }
            let prefix = self.application_text("invalid database choice '", span);
            let text = self.runtime_call(
                "dever_rt_v1_text_concat",
                vec![format!("ptr {prefix}"), format!("ptr {value}")],
                &Type::Text,
                false,
                None,
                span,
            );
            self.own_value(&Type::Text, &text, false);
            let suffix = self.application_text("'", span);
            let message = self.runtime_call(
                "dever_rt_v1_text_concat",
                vec![format!("ptr {text}"), format!("ptr {suffix}")],
                &Type::Text,
                false,
                None,
                span,
            );
            self.own_value(&Type::Text, &message, false);
            let errors = self.database_errors(span);
            let error = self.entry_slot_ir("{ ptr, i64 }");
            let status = self.temp();
            self.line(format!("{status} = call i32 @dever_rt_v1_db_error(i32 8, ptr {message}, ptr {errors}, ptr %fault, ptr {error})"));
            self.database_status(&status, &error, span);
            self.line("unreachable");
            self.start(&done);
            self.release_guard(owner);
            let decoded = self.temp();
            self.line(format!("{decoded} = load %T{id}, ptr {output}"));
            return decoded;
        }
        value
    }

    pub(super) fn database_record(
        &mut self,
        row: &str,
        id: usize,
        offset: usize,
        exact: bool,
        span: Span,
    ) -> String {
        let Shape::Record(fields) = self.module.program.types[id].shape.clone() else {
            unreachable!()
        };
        let slot = self.entry_slot(&Type::Named(id));
        self.line(format!("store %T{id} zeroinitializer, ptr {slot}"));
        let guard = self.register_guard(&Type::Named(id), slot.clone());
        self.mark_live(guard);
        let actual = self.protocol_call(
            "dever_rt_v1_db_row_len",
            vec![format!("ptr {row}")],
            "i64",
            span,
        );
        let record_name = self
            .module
            .program
            .models
            .iter()
            .find(|model| model.record == id)
            .map(|model| model.package.clone())
            .unwrap_or_else(|| self.module.program.types[id].name.clone());
        let mut column = offset;
        for (index, field) in fields.iter().enumerate() {
            if matches!(field.ty, Type::Related(_)) {
                continue;
            }
            self.database_column_present(
                &actual,
                column,
                &format!("{record_name}.{}", field.name),
                span,
            );
            let value = self.database_scalar(row, column, &field.ty, span);
            let destination = self.temp();
            self.line(format!(
                "{destination} = getelementptr %T{id}, ptr {slot}, i32 0, i32 {index}"
            ));
            self.line(format!(
                "store {} {value}, ptr {destination}",
                self.module.ty(&field.ty)
            ));
            column += 1;
        }
        if exact {
            self.database_row_length(
                row,
                column,
                &format!("database row for {record_name} has extra columns"),
                span,
            );
        }
        let value = self.temp();
        self.line(format!("{value} = load %T{id}, ptr {slot}"));
        self.disarm(guard);
        value
    }

    pub(super) fn database_column_present(
        &mut self,
        actual: &str,
        index: usize,
        name: &str,
        span: Span,
    ) {
        let present = self.temp();
        self.line(format!("{present} = icmp sgt i64 {actual}, {index}"));
        let found = self.label("database_column_found");
        let missing = self.label("database_column_missing");
        self.line(format!("br i1 {present}, label %{found}, label %{missing}"));
        self.start(&missing);
        self.database_fault(8, &format!("database row is missing column '{name}'"), span);
        self.start(&found);
    }

    pub(super) fn database_row_length(
        &mut self,
        row: &str,
        length: usize,
        message: &str,
        span: Span,
    ) {
        let actual = self.protocol_call(
            "dever_rt_v1_db_row_len",
            vec![format!("ptr {row}")],
            "i64",
            span,
        );
        let equal = self.temp();
        self.line(format!("{equal} = icmp eq i64 {actual}, {length}"));
        let okay = self.label("database_row_exact");
        let invalid = self.label("database_row_extra");
        self.line(format!("br i1 {equal}, label %{okay}, label %{invalid}"));
        self.start(&invalid);
        self.database_fault(8, message, span);
        self.start(&okay);
    }
}
