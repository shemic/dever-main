use crate::diagnostic::Diagnostic;
use crate::source::{SourceFile, SourceMap, Span};
use crate::syntax::*;
use crate::token::{Kind, Token};

mod expression;

type ParseResult<T> = Result<T, Diagnostic>;

pub(crate) fn parse(tokens: Vec<Token>, source: &SourceFile) -> Result<Package, Vec<Diagnostic>> {
    let layout = source.layout().map_err(|error| vec![error])?;
    let mut comments = Vec::new();
    let tokens = tokens
        .into_iter()
        .filter(|token| {
            if let Kind::Comment(text) = &token.kind {
                comments.push(Comment {
                    text: text.clone(),
                    span: token.span,
                });
                false
            } else {
                true
            }
        })
        .collect();
    let mut parser = Parser {
        tokens,
        cursor: 0,
        brace_depth: 0,
        nesting: 0,
        model: source.is_model(),
        application: !SourceMap::is_standard(source.id())
            && match &layout {
                crate::source::SourceLayout::Main => source.strict_layout(),
                crate::source::SourceLayout::Test { .. } => true,
                crate::source::SourceLayout::Role { .. } => true,
                crate::source::SourceLayout::Loose => false,
            },
    };
    let name = source.package_path().map_err(|error| vec![error])?;
    parser.package(name, layout, comments)
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    brace_depth: usize,
    nesting: usize,
    model: bool,
    application: bool,
}

impl Parser {
    fn current(&self) -> &Token {
        &self.tokens[self.cursor]
    }

    fn at(&self, kind: &Kind) -> bool {
        &self.current().kind == kind
    }

    fn at_name(&self, name: &str) -> bool {
        matches!(&self.current().kind, Kind::Name(text) if text == name)
    }

    fn modifier(&mut self, name: &str) -> Option<Span> {
        if self.at_name(name)
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|token| matches!(token.kind, Kind::Name(_)))
        {
            Some(self.advance().span)
        } else {
            None
        }
    }

    fn advance(&mut self) -> Token {
        let token = self.current().clone();
        match token.kind {
            Kind::LeftBrace => self.brace_depth += 1,
            Kind::RightBrace => self.brace_depth = self.brace_depth.saturating_sub(1),
            _ => {}
        }
        if !self.at(&Kind::End) {
            self.cursor += 1;
        }
        token
    }

    fn take(&mut self, kind: &Kind) -> Option<Token> {
        if self.at(kind) {
            Some(self.advance())
        } else {
            None
        }
    }

    fn expect(&mut self, kind: &Kind, description: &str) -> ParseResult<Token> {
        self.take(kind)
            .ok_or_else(|| self.error(format!("expected {description}")))
    }

    fn error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error("P001", message, self.current().span)
    }

    fn skip_lines(&mut self) {
        while self.take(&Kind::Newline).is_some() {}
    }

    fn previous_span(&self) -> Span {
        self.tokens[self.cursor - 1].span
    }

    fn name(&mut self) -> ParseResult<Name> {
        if let Kind::Name(text) = &self.current().kind {
            let name = Name {
                text: text.clone(),
                span: self.current().span,
            };
            self.advance();
            Ok(name)
        } else {
            Err(self.error("expected an identifier"))
        }
    }

    fn path(&mut self) -> ParseResult<Path> {
        let mut path = vec![self.name()?];
        while self.take(&Kind::Dot).is_some() {
            path.push(self.name()?);
        }
        Ok(path)
    }

    // Cap structural recursion while parsing untrusted source, before using the host stack.
    fn nested<T>(&mut self, parse: impl FnOnce(&mut Self) -> ParseResult<T>) -> ParseResult<T> {
        if self.nesting == MAX_SYNTAX_DEPTH {
            return Err(Diagnostic::error(
                "P009",
                "syntax nesting exceeds the compiler limit",
                self.current().span,
            ));
        }
        self.nesting += 1;
        let result = parse(self);
        self.nesting -= 1;
        result
    }

    fn line_end(&mut self, closing: &Kind) -> ParseResult<()> {
        if self.take(&Kind::Newline).is_some() {
            self.skip_lines();
            return Ok(());
        }
        if self.at(closing) {
            return Ok(());
        }
        Err(self.error("expected a newline or the closing delimiter"))
    }

    fn separated<T>(
        &mut self,
        closing: Kind,
        parse: impl Fn(&mut Self) -> ParseResult<T>,
    ) -> ParseResult<Vec<T>> {
        let mut members = Vec::new();
        self.skip_lines();
        if self.take(&closing).is_some() {
            return Ok(members);
        }
        loop {
            members.push(parse(self)?);
            self.skip_lines();
            if self.take(&closing).is_some() {
                return Ok(members);
            }
            self.expect(&Kind::Comma, "',' between positional entries")?;
            self.skip_lines();
        }
    }

    fn package(
        &mut self,
        name: Path,
        layout: crate::source::SourceLayout,
        comments: Vec<Comment>,
    ) -> Result<Package, Vec<Diagnostic>> {
        self.skip_lines();
        let start = self.current().span;
        let mut declarations = Vec::new();
        let mut errors = Vec::new();
        while !self.at(&Kind::End) {
            let visibility = self.take(&Kind::Public).map(|token| token.span);
            let anonymous_http = layout.role() == Some(crate::source::SourceRole::Api)
                && ["get", "post", "put", "delete"]
                    .iter()
                    .any(|name| self.at_name(name));
            if self.application
                && !anonymous_http
                && let Some(span) = visibility
            {
                errors.push(Diagnostic::error(
                    "P001",
                    "application visibility is defined by the source role; remove 'public'",
                    span,
                ));
            }
            let declaration = if self.at(&Kind::Package) || self.at(&Kind::Exposes) {
                Err(self.error("package/exposes declarations were removed; the source path and role define identity and visibility"))
            } else if layout.role() == Some(crate::source::SourceRole::Api)
                && ["get", "post", "put", "delete", "cmd", "rest"]
                    .iter()
                    .any(|name| self.at_name(name))
            {
                self.api_declaration()
            } else if layout.role() == Some(crate::source::SourceRole::Adapter)
                && self.at_name("external")
            {
                self.external_adapter().map(Declaration::External)
            } else if self.at(&Kind::Type) {
                self.type_declaration(None).map(Declaration::Type)
            } else if self.at_name("global")
                && self
                    .tokens
                    .get(self.cursor + 1)
                    .is_some_and(|token| token.kind == Kind::Type)
            {
                let start = self.advance().span;
                self.type_declaration(Some(start)).map(Declaration::Type)
            } else if self.at_name("setting")
                && self
                    .tokens
                    .get(self.cursor + 1)
                    .is_some_and(|token| token.kind == Kind::LeftBrace)
            {
                self.setting_declaration().map(Declaration::Type)
            } else if (self.model || layout.role() == Some(crate::source::SourceRole::Job))
                && self.at_name("database")
            {
                self.database_binding().map(Declaration::Database)
            } else if self.at_name("schedule")
                && layout.role() == Some(crate::source::SourceRole::Job)
            {
                self.schedule().map(Declaration::Schedule)
            } else if self.model && self.at_name("index") {
                self.model_index(false).map(Declaration::ModelIndex)
            } else if self.model && self.at_name("unique") {
                self.model_index(true).map(Declaration::ModelIndex)
            } else if self.model && self.at_name("relation") {
                self.relation().map(Declaration::Relation)
            } else if self.model && self.at_name("seed") {
                self.seed().map(Declaration::Seed)
            } else if self.model && self.at_name("migrate") {
                self.migration().map(Declaration::Migration)
            } else if self.model && self.at_name("sql") {
                self.model_sql().map(Declaration::ModelSql)
            } else if self.at_name("job") {
                let start = self.advance().span;
                self.function(
                    FunctionKind::Job {
                        attempts: 0,
                        timeout_ms: 0,
                    },
                    Some(start),
                )
                .map(Declaration::Function)
            } else if self.at_name("transaction") {
                let start = self.advance().span;
                self.function(FunctionKind::Transaction, Some(start))
                    .map(Declaration::Function)
            } else {
                self.function(FunctionKind::Ordinary, None)
                    .map(Declaration::Function)
            };
            match declaration {
                Ok(mut declaration) => {
                    if let Declaration::Api(binding) = &mut declaration {
                        binding.anonymous = visibility.is_some() && anonymous_http;
                        if let Some(start) = visibility {
                            binding.span = start.through(binding.span);
                        }
                    }
                    if let Some(start) = visibility.filter(|_| !self.application) {
                        match &mut declaration {
                            Declaration::Type(ty) => {
                                ty.public = true;
                                ty.span = start.through(ty.span);
                            }
                            Declaration::Function(function) => {
                                function.public = true;
                                function.span = start.through(function.span);
                            }
                            _ => errors.push(Diagnostic::error(
                                "P001",
                                "'public' is allowed only on types and functions",
                                start,
                            )),
                        }
                    }
                    declarations.push(declaration);
                }
                Err(error) => {
                    errors.push(error);
                    self.recover_declaration();
                }
            }
            self.skip_lines();
        }
        if errors.is_empty() {
            Ok(Package {
                name,
                layout,
                declarations,
                comments,
                span: start.through(self.current().span),
            })
        } else {
            Err(errors)
        }
    }

    fn external_adapter(&mut self) -> ParseResult<ExternalAdapter> {
        let start = self.advance().span;
        let ecosystem = self.name()?;
        let ecosystem = match ecosystem.text.as_str() {
            "exec" => ExternalEcosystem::Exec,
            "command" => ExternalEcosystem::Command,
            "pip" => ExternalEcosystem::Pip,
            "npm" => ExternalEcosystem::Npm,
            "go" => ExternalEcosystem::Go,
            _ => {
                return Err(Diagnostic::error(
                    "P001",
                    "external Adapter ecosystem must be exec, command, pip, npm, or go",
                    ecosystem.span,
                ));
            }
        };
        let Kind::Text(entry) = &self.current().kind else {
            return Err(self.error("external requires a literal relative entry path"));
        };
        let entry = entry.clone();
        let entry_span = self.advance().span;
        self.expect(&Kind::LeftBrace, "'{' after the external entry")?;
        self.skip_lines();
        let mut libs = Vec::new();
        let mut capabilities = Vec::new();
        while !self.at(&Kind::RightBrace) {
            if self.at_name("lib") {
                self.advance();
                let Kind::Text(lib) = &self.current().kind else {
                    return Err(self.error("external lib requires a literal name@version or <ecosystem>:<name>@<version>"));
                };
                libs.push(Name {
                    text: lib.clone(),
                    span: self.current().span,
                });
                self.advance();
                self.line_end(&Kind::RightBrace)?;
                continue;
            }
            if !self.at_name("allow") {
                return Err(self
                    .error("external accepts only 'lib <spec>' or 'allow <capability>' entries"));
            }
            self.advance();
            capabilities.push(self.name()?);
            self.line_end(&Kind::RightBrace)?;
        }
        let end = self.advance().span;
        Ok(ExternalAdapter {
            ecosystem,
            entry,
            entry_span,
            libs,
            capabilities,
            span: start.through(end),
        })
    }

    fn api_declaration(&mut self) -> ParseResult<Declaration> {
        let keyword = self.name()?;
        if keyword.text == "rest" {
            let model = if self.at(&Kind::Newline) || self.at(&Kind::End) {
                None
            } else {
                Some(self.path()?)
            };
            let span = keyword.span.through(self.previous_span());
            self.line_end(&Kind::End)?;
            return Ok(Declaration::Rest(RestDeclaration { model, span }));
        }
        let kind = match keyword.text.as_str() {
            "get" => ApiKind::Get,
            "post" => ApiKind::Post,
            "put" => ApiKind::Put,
            "delete" => ApiKind::Delete,
            "cmd" => ApiKind::Command,
            _ => unreachable!("checked API declaration keyword"),
        };
        let action = self.name()?;
        self.expect(&Kind::Assign, "'=' before the App binding")?;
        let target = self.path()?;
        let span = keyword.span.through(self.previous_span());
        self.line_end(&Kind::End)?;
        Ok(Declaration::Api(ApiBinding {
            anonymous: false,
            kind,
            action,
            target,
            span,
        }))
    }

    fn schedule(&mut self) -> ParseResult<Schedule> {
        let start = self.advance().span;
        let target = self.name()?;
        self.expect(&Kind::Assign, "'=' before UTC cron expression")?;
        let Kind::Text(cron) = &self.current().kind else {
            return Err(self.error("schedule requires a literal five-field UTC cron expression"));
        };
        let cron = cron.clone();
        let span = start.through(self.advance().span);
        Ok(Schedule { target, cron, span })
    }

    fn database_binding(&mut self) -> ParseResult<DatabaseBinding> {
        let start = self.advance().span;
        let name = self.name()?;
        Ok(DatabaseBinding {
            span: start.through(name.span),
            name,
        })
    }

    fn model_index(&mut self, unique: bool) -> ParseResult<ModelIndex> {
        let start = self.advance().span;
        self.expect(&Kind::LeftParen, "'(' after the index kind")?;
        let fields = self.separated(Kind::RightParen, Self::name)?;
        if fields.is_empty() {
            return Err(Diagnostic::error(
                "P001",
                "an index requires at least one field",
                start,
            ));
        }
        Ok(ModelIndex {
            unique,
            fields,
            span: start.through(self.previous_span()),
        })
    }

    fn relation(&mut self) -> ParseResult<Relation> {
        let start = self.advance().span;
        let name = self.name()?;
        self.expect(&Kind::Assign, "'=' after the relation name")?;
        let field = self.path()?;
        Ok(Relation {
            name,
            field,
            span: start.through(self.previous_span()),
        })
    }

    fn seed(&mut self) -> ParseResult<Seed> {
        let start = self.advance().span;
        self.expect(&Kind::LeftBrace, "'{' after 'seed'")?;
        self.skip_lines();
        let mut rows = Vec::new();
        while !self.at(&Kind::RightBrace) {
            rows.push(self.seed_row()?);
            self.line_end(&Kind::RightBrace)?;
        }
        let end = self.advance().span;
        Ok(Seed {
            rows,
            span: start.through(end),
        })
    }

    fn seed_row(&mut self) -> ParseResult<Vec<RecordField>> {
        self.expect(&Kind::LeftBrace, "'{' around a seed row")?;
        self.skip_lines();
        let mut fields = Vec::new();
        while !self.at(&Kind::RightBrace) {
            let name = self.name()?;
            let start = name.span;
            self.expect(&Kind::Assign, "'=' after the seed field name")?;
            let value = self.expression(0, false)?;
            fields.push(RecordField {
                name,
                value,
                span: start.through(self.previous_span()),
            });
            self.line_end(&Kind::RightBrace)?;
        }
        self.advance();
        Ok(fields)
    }

    fn migration(&mut self) -> ParseResult<Migration> {
        let start = self.advance().span;
        let name = self.name()?;
        self.expect(&Kind::LeftBrace, "'{' after the migration name")?;
        self.skip_lines();
        let mut operations = Vec::new();
        while !self.at(&Kind::RightBrace) {
            if self.at_name("drop") {
                self.advance();
                operations.push(MigrationOperation::Drop(self.name()?));
            } else if self.at_name("before") || self.at_name("after") {
                operations.push(MigrationOperation::Sql(self.migration_sql()?));
            } else {
                return Err(self.error("expected a migration operation"));
            }
            self.line_end(&Kind::RightBrace)?;
        }
        let end = self.advance().span;
        Ok(Migration {
            name,
            operations,
            span: start.through(end),
        })
    }

    fn migration_sql(&mut self) -> ParseResult<MigrationSql> {
        let phase = if self.at_name("before") {
            MigrationPhase::Before
        } else {
            MigrationPhase::After
        };
        let start = self.advance().span;
        self.expect(&Kind::LeftBrace, "'{' after the migration phase")?;
        self.skip_lines();
        let mut sqlite = None;
        let mut postgres = None;
        let mut parameters = None;
        while !self.at(&Kind::RightBrace) {
            let field = self.name()?;
            self.expect(&Kind::Assign, "'=' after the migration SQL field")?;
            if field.text == "parameters" {
                self.expect(&Kind::LeftBracket, "'[' for migration parameters")?;
                let values = self.separated(Kind::RightBracket, |this| this.expression(0, true))?;
                if parameters.replace(values).is_some() {
                    return Err(Diagnostic::error(
                        "P001",
                        "duplicate migration parameters",
                        field.span,
                    ));
                }
            } else {
                let slot = match field.text.as_str() {
                    "sqlite" => &mut sqlite,
                    "postgres" => &mut postgres,
                    _ => {
                        return Err(Diagnostic::error(
                            "P001",
                            "migration SQL requires sqlite, postgres and parameters",
                            field.span,
                        ));
                    }
                };
                if slot.replace(self.sql_text()?).is_some() {
                    return Err(Diagnostic::error(
                        "P001",
                        "duplicate migration SQL dialect",
                        field.span,
                    ));
                }
            }
            self.line_end(&Kind::RightBrace)?;
        }
        let end = self.advance().span;
        Ok(MigrationSql {
            phase,
            sqlite: sqlite.ok_or_else(|| {
                Diagnostic::error("P001", "migration SQL requires sqlite text", start)
            })?,
            postgres: postgres.ok_or_else(|| {
                Diagnostic::error("P001", "migration SQL requires postgres text", start)
            })?,
            parameters: parameters.ok_or_else(|| {
                Diagnostic::error(
                    "P001",
                    "migration SQL requires an explicit parameters list",
                    start,
                )
            })?,
            span: start.through(end),
        })
    }

    fn sql_text(&mut self) -> ParseResult<SqlText> {
        let Kind::Text(value) = &self.current().kind else {
            return Err(self.error("SQL text must be a static Text literal"));
        };
        let value = SqlText {
            value: value.clone(),
            span: self.current().span,
        };
        self.advance();
        Ok(value)
    }

    fn model_sql(&mut self) -> ParseResult<ModelSql> {
        let start = self.advance().span;
        let name = self.name()?;
        self.expect(&Kind::LeftParen, "'(' after a SQL operation name")?;
        let inputs = self.separated(Kind::RightParen, Self::field)?;
        self.expect(&Kind::LeftParen, "the SQL result list '('")?;
        let outputs = self.separated(Kind::RightParen, Self::field)?;
        self.expect(&Kind::LeftBrace, "the SQL body '{'")?;
        self.skip_lines();
        let mut sqlite = None;
        let mut postgres = None;
        while !self.at(&Kind::RightBrace) {
            let dialect = self.name()?;
            if !matches!(dialect.text.as_str(), "sqlite" | "postgres") {
                return Err(Diagnostic::error(
                    "P001",
                    "a SQL body accepts only 'sqlite' and 'postgres'",
                    dialect.span,
                ));
            }
            self.expect(&Kind::Assign, "'=' after the SQL dialect")?;
            let value = self.sql_text()?;
            let slot = if dialect.text == "sqlite" {
                &mut sqlite
            } else {
                &mut postgres
            };
            if slot.replace(value).is_some() {
                return Err(Diagnostic::error(
                    "P001",
                    format!("duplicate '{}' SQL dialect", dialect.text),
                    dialect.span,
                ));
            }
            self.line_end(&Kind::RightBrace)?;
        }
        let end = self.advance().span;
        let sqlite = sqlite.ok_or_else(|| {
            Diagnostic::error("P001", "a SQL declaration requires sqlite text", name.span)
        })?;
        let postgres = postgres.ok_or_else(|| {
            Diagnostic::error(
                "P001",
                "a SQL declaration requires postgres text",
                name.span,
            )
        })?;
        Ok(ModelSql {
            name,
            inputs,
            outputs,
            sqlite,
            postgres,
            span: start.through(end),
        })
    }

    fn recover_declaration(&mut self) {
        // Resume only after a balanced body or a top-level line, never inside a record.
        while !self.at(&Kind::End) {
            let token = self.advance();
            if self.brace_depth == 0 && matches!(token.kind, Kind::RightBrace | Kind::Newline) {
                return;
            }
        }
    }

    fn type_declaration(&mut self, global: Option<Span>) -> ParseResult<TypeDeclaration> {
        let type_start = self.expect(&Kind::Type, "'type' after 'global'")?.span;
        let start = global.unwrap_or(type_start);
        let name = self.name()?;
        self.expect(&Kind::LeftBrace, "'{' after the type name")?;
        self.skip_lines();
        if let Some(end) = self.take(&Kind::RightBrace) {
            return Ok(TypeDeclaration {
                public: false,
                setting: false,
                global: global.is_some(),
                name,
                shape: TypeShape::Record(Vec::new()),
                span: start.through(end.span),
            });
        }
        let private = self.modifier("private");
        let owner = self.modifier("owner");
        let first = self.name()?;
        let shape = if self.at(&Kind::Colon) {
            let mut fields = vec![self.field_after_name(first, private, owner)?];
            self.line_end(&Kind::RightBrace)?;
            while !self.at(&Kind::RightBrace) {
                let private = self.modifier("private");
                let owner = self.modifier("owner");
                let name = self.name()?;
                fields.push(self.field_after_name(name, private, owner)?);
                self.line_end(&Kind::RightBrace)?;
            }
            TypeShape::Record(fields)
        } else {
            if private.is_some() {
                return Err(self.error("'private' is allowed only on record fields"));
            }
            let mut variants = vec![self.variant(first)?];
            self.line_end(&Kind::RightBrace)?;
            while !self.at(&Kind::RightBrace) {
                let name = self.name()?;
                variants.push(self.variant(name)?);
                self.line_end(&Kind::RightBrace)?;
            }
            TypeShape::Choice(variants)
        };
        let end = self.expect(&Kind::RightBrace, "'}'")?.span;
        Ok(TypeDeclaration {
            public: false,
            setting: false,
            global: global.is_some(),
            name,
            shape,
            span: start.through(end),
        })
    }

    fn setting_declaration(&mut self) -> ParseResult<TypeDeclaration> {
        let start = self.advance().span;
        self.expect(&Kind::LeftBrace, "'{' after setting")?;
        self.skip_lines();
        let mut fields = Vec::new();
        while !self.at(&Kind::RightBrace) {
            fields.push(self.field()?);
            self.line_end(&Kind::RightBrace)?;
        }
        let end = self.advance().span;
        Ok(TypeDeclaration {
            public: false,
            setting: true,
            global: false,
            name: Name {
                text: "Setting".into(),
                span: start,
            },
            shape: TypeShape::Record(fields),
            span: start.through(end),
        })
    }

    fn field(&mut self) -> ParseResult<Field> {
        let name = self.name()?;
        self.field_after_name(name, None, None)
    }

    fn field_after_name(
        &mut self,
        name: Name,
        private: Option<Span>,
        owner: Option<Span>,
    ) -> ParseResult<Field> {
        self.expect(&Kind::Colon, "':' followed by a type")?;
        let ty = self.type_ref()?;
        let bounds = self.bounds()?;
        let mut storage = self.field_storage()?;
        if owner.is_some() {
            self.expect(
                &Kind::Assign,
                "'=' and a trusted identity source after an owner field",
            )?;
            storage.owner = Some(self.expression(0, false)?);
        }
        let span = private
            .or(owner)
            .unwrap_or(name.span)
            .through(self.previous_span());
        Ok(Field {
            name,
            ty,
            private: private.is_some(),
            bounds,
            storage,
            span,
        })
    }

    fn field_storage(&mut self) -> ParseResult<FieldStorage> {
        let mut storage = FieldStorage::default();
        while matches!(
            &self.current().kind,
            Kind::Name(name)
                if matches!(name.as_str(), "generated" | "default" | "index" | "unique" | "from" | "create" | "replace" | "search")
        ) {
            let modifier = self.name()?;
            match modifier.text.as_str() {
                "generated" if !storage.generated => storage.generated = true,
                "default" if storage.default.is_none() => {
                    storage.default = Some(self.expression(0, false)?);
                }
                "index" if !storage.index => storage.index = true,
                "unique" if !storage.unique => storage.unique = true,
                "from" if storage.from.is_none() => storage.from = Some(self.name()?),
                "create" if storage.create.is_none() => {
                    self.expect(&Kind::Assign, "'=' and a REST create binding expression")?;
                    storage.create = Some(self.expression(0, false)?);
                }
                "replace" if storage.replace.is_none() => {
                    self.expect(&Kind::Assign, "'=' and a REST replace binding expression")?;
                    storage.replace = Some(self.expression(0, false)?);
                }
                "search" if storage.search.is_none() => {
                    self.expect(&Kind::Assign, "'=' and a REST search binding expression")?;
                    storage.search = Some(self.expression(0, false)?);
                }
                _ => {
                    return Err(Diagnostic::error(
                        "P001",
                        format!("duplicate '{}' field modifier", modifier.text),
                        modifier.span,
                    ));
                }
            }
        }
        Ok(storage)
    }

    fn variant(&mut self, name: Name) -> ParseResult<Variant> {
        let start = name.span;
        let error = name.text == "error" && matches!(self.current().kind, Kind::Name(_));
        let name = if error { self.name()? } else { name };
        let payload = if self.take(&Kind::LeftParen).is_some() {
            self.separated(Kind::RightParen, Self::field)?
        } else {
            Vec::new()
        };
        let label = if self.take(&Kind::Assign).is_some() {
            let Kind::Text(label) = &self.current().kind else {
                return Err(self.error("expected a choice display label string"));
            };
            let label = label.clone();
            self.advance();
            Some(label)
        } else {
            None
        };
        let span = start.through(self.previous_span());
        Ok(Variant {
            name,
            error,
            payload,
            label,
            span,
        })
    }

    fn type_ref(&mut self) -> ParseResult<TypeRef> {
        self.nested(Self::type_ref_inner)
    }

    fn type_ref_inner(&mut self) -> ParseResult<TypeRef> {
        let name = self.path()?;
        let generic = name.len() == 1
            && matches!(
                name[0].text.as_str(),
                "List" | "Map" | "MapEntry" | "Stream" | "AsyncStream" | "Channel" | "Related"
            )
            || name.len() == 2
                && name[0].text == "Related"
                && matches!(name[1].text.as_str(), "Loaded" | "Unloaded");
        let arguments = if generic && self.take(&Kind::Less).is_some() {
            self.separated(Kind::Greater, Self::type_ref)?
        } else {
            Vec::new()
        };
        let storage_parameters = self.at(&Kind::LeftParen)
            && self
                .tokens
                .get(self.cursor + 1)
                .is_some_and(|token| matches!(token.kind, Kind::Number(_)));
        let parameters = if storage_parameters {
            self.advance();
            self.separated(Kind::RightParen, Self::number_parameter)?
        } else {
            Vec::new()
        };
        let nullable = self.take(&Kind::Question).is_some();
        if self.at(&Kind::Question) {
            return Err(self.error("a type may have only one '?' suffix"));
        }
        let span = name[0].span.through(self.previous_span());
        Ok(TypeRef {
            name,
            arguments,
            parameters,
            nullable,
            span,
        })
    }

    fn number_parameter(&mut self) -> ParseResult<NumberParameter> {
        let Kind::Number(value) = &self.current().kind else {
            return Err(self.error("expected a numeric type parameter"));
        };
        let value = value.clone();
        let span = self.advance().span;
        Ok(NumberParameter { value, span })
    }

    fn function(
        &mut self,
        mut kind: FunctionKind,
        declaration_start: Option<Span>,
    ) -> ParseResult<FunctionClause> {
        let mut name = self.name()?;
        while self.take(&Kind::Dot).is_some() {
            let segment = self.name()?;
            name.text.push('.');
            name.text.push_str(&segment.text);
            name.span = name.span.through(segment.span);
        }
        self.expect(&Kind::LeftParen, "'(' after a function name")?;
        let inputs = self.separated(Kind::RightParen, Self::input)?;
        self.expect(&Kind::LeftParen, "the named output list '('")?;
        let outputs = self.separated(Kind::RightParen, Self::field)?;
        if matches!(kind, FunctionKind::Job { .. }) {
            let attempts = self.job_policy("retry", 100)?;
            let timeout_ms =
                self.job_policy("timeout", dever_runtime::config::MAX_JOB_TIMEOUT_MS)?;
            kind = FunctionKind::Job {
                attempts,
                timeout_ms,
            };
        }
        let (pure, recovery) = self.function_contracts()?;
        let fails = if self.at_name("fails") {
            self.advance();
            Some(self.type_ref()?)
        } else {
            None
        };
        if let Some(failure) = &fails
            && !self.at(&Kind::LeftBrace)
        {
            let span = declaration_start.unwrap_or(name.span).through(failure.span);
            return Ok(FunctionClause {
                public: false,
                name,
                kind,
                inputs,
                outputs,
                pure,
                recovery,
                fails,
                bodyless: true,
                body: Vec::new(),
                span,
            });
        }
        self.expect(&Kind::LeftBrace, "the function body '{'")?;
        self.skip_lines();
        let mut body = Vec::new();
        while !self.at(&Kind::RightBrace) {
            body.push(self.statement()?);
            self.line_end(&Kind::RightBrace)?;
        }
        let end = self.advance().span;
        let span = declaration_start.unwrap_or(name.span).through(end);
        Ok(FunctionClause {
            public: false,
            name,
            kind,
            inputs,
            outputs,
            pure,
            recovery,
            fails,
            bodyless: false,
            body,
            span,
        })
    }

    fn job_policy(&mut self, name: &str, maximum: u32) -> ParseResult<u32> {
        if !self.at_name(name) {
            return Err(self.error(format!("job requires {name}(positive integer)")));
        }
        self.advance();
        self.expect(&Kind::LeftParen, "'(' before job policy")?;
        let number = self.signed_number()?;
        let value = number
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0 && *value <= maximum)
            .ok_or_else(|| self.error(format!("{name} must be an integer in 1..={maximum}")))?;
        self.expect(&Kind::RightParen, "')' after job policy")?;
        Ok(value)
    }

    fn function_contracts(&mut self) -> ParseResult<(bool, Option<String>)> {
        let mut pure = false;
        let mut recovery = None;
        self.skip_lines();
        while self.at_name("pure") || self.at_name("recover") {
            if self.at_name("pure") {
                if pure {
                    return Err(self.error("duplicate 'pure' contract"));
                }
                self.advance();
                pure = true;
            } else {
                if recovery.is_some() {
                    return Err(self.error("duplicate 'recover' contract"));
                }
                self.advance();
                self.skip_lines();
                self.expect(&Kind::LeftParen, "'(' after 'recover'")?;
                self.skip_lines();
                let Kind::Text(reason) = &self.current().kind else {
                    return Err(self.error("expected a nonempty recovery reason string"));
                };
                if reason.trim().is_empty() {
                    return Err(self.error("recovery reason must not be empty"));
                }
                recovery = Some(reason.clone());
                self.advance();
                self.skip_lines();
                self.expect(&Kind::RightParen, "')' after the recovery reason")?;
            }
            self.skip_lines();
        }
        Ok((pure, recovery))
    }

    fn input(&mut self) -> ParseResult<Input> {
        let name = self.name()?;
        self.expect(&Kind::Colon, "':' followed by an input type or pattern")?;
        let kind = if matches!(&self.current().kind, Kind::Name(value) if value == "handler") {
            let handler_span = self.advance().span;
            self.expect(&Kind::LeftParen, "'(' after 'handler'")?;
            let inputs = self.separated(Kind::RightParen, Self::field)?;
            self.expect(&Kind::LeftParen, "the handler output list '('")?;
            let outputs = self.separated(Kind::RightParen, Self::field)?;
            InputKind::Handler(HandlerSignature {
                inputs,
                outputs,
                span: handler_span.through(self.previous_span()),
            })
        } else {
            InputKind::Value(self.pattern()?)
        };
        let span = name.span.through(self.previous_span());
        Ok(Input { name, kind, span })
    }

    fn pattern(&mut self) -> ParseResult<Pattern> {
        let start = self.current().span;
        if let Some(token) = self.take(&Kind::Other) {
            return Ok(Pattern::Other(token.span));
        }
        if let Some(value) = self.literal() {
            return Ok(Pattern::Literal {
                value,
                span: start.through(self.previous_span()),
            });
        }
        if self.at(&Kind::Minus) {
            let number = self.signed_number()?;
            return Ok(Pattern::Literal {
                value: Literal::Number(number),
                span: start.through(self.previous_span()),
            });
        }
        let ty = self.type_ref()?;
        if self.take(&Kind::LeftParen).is_some() {
            if ty.nullable {
                return Err(self.error("expected a qualified choice variant"));
            }
            let bindings = self.separated(Kind::RightParen, Self::name)?;
            return Ok(Pattern::Variant {
                name: ty.name,
                arguments: ty.arguments,
                bindings,
                span: start.through(self.previous_span()),
            });
        }
        let bounds = self.bounds()?;
        Ok(Pattern::Typed { ty, bounds })
    }

    fn bounds(&mut self) -> ParseResult<Vec<Bound>> {
        let mut bounds = Vec::new();
        if self.range_operator().is_some() {
            bounds.push(self.bound()?);
            if self.take(&Kind::And).is_some() {
                bounds.push(self.bound()?);
            }
        }
        Ok(bounds)
    }

    fn range_operator(&self) -> Option<BinaryOperator> {
        match self.current().kind {
            Kind::Less => Some(BinaryOperator::Less),
            Kind::LessEqual => Some(BinaryOperator::LessEqual),
            Kind::Greater => Some(BinaryOperator::Greater),
            Kind::GreaterEqual => Some(BinaryOperator::GreaterEqual),
            _ => None,
        }
    }

    fn bound(&mut self) -> ParseResult<Bound> {
        let comparison = self
            .range_operator()
            .ok_or_else(|| self.error("expected '<', '<=', '>' or '>='"))?;
        let start = self.advance().span;
        let number = self.signed_number()?;
        Ok(Bound {
            comparison,
            number,
            span: start.through(self.previous_span()),
        })
    }

    fn signed_number(&mut self) -> ParseResult<String> {
        let negative = self.take(&Kind::Minus).is_some();
        if let Kind::Number(number) = &self.current().kind {
            let spelling = if negative {
                format!("-{number}")
            } else {
                number.clone()
            };
            self.advance();
            Ok(spelling)
        } else {
            Err(self.error("expected a numeric literal"))
        }
    }

    fn literal(&mut self) -> Option<Literal> {
        let value = match &self.current().kind {
            Kind::True => Literal::Bool(true),
            Kind::False => Literal::Bool(false),
            Kind::Null => Literal::Null,
            Kind::Text(text) => Literal::Text(text.clone()),
            Kind::Number(number) => Literal::Number(number.clone()),
            _ => return None,
        };
        self.advance();
        Some(value)
    }

    fn statement(&mut self) -> ParseResult<Statement> {
        let expression = self.expression(0, false)?;
        let start = expression.span;
        let kind = if self.take(&Kind::Assign).is_some() {
            let ExpressionKind::Name(target) = expression.kind else {
                return Err(Diagnostic::error(
                    "P002",
                    "assignment requires a local name or field path",
                    start,
                ));
            };
            let value = self.expression(0, false)?;
            StatementKind::Assign { target, value }
        } else if matches!(
            expression.kind,
            ExpressionKind::Call { .. } | ExpressionKind::Fail(_) | ExpressionKind::Contextual(_)
        ) {
            StatementKind::Call(expression)
        } else {
            return Err(Diagnostic::error(
                "P002",
                "a body statement must be an assignment or function call",
                start,
            ));
        };
        Ok(Statement {
            kind,
            span: start.through(self.previous_span()),
        })
    }
}
