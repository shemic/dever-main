use super::*;

pub(super) fn contains_owned(program: &Program, ty: &Type) -> bool {
    match ty {
        Type::Text
        | Type::Json
        | Type::Id
        | Type::Secret
        | Type::Uuid
        | Type::ClientTls
        | Type::ServerTls
        | Type::HttpClient
        | Type::HttpReply
        | Type::WebSocket
        | Type::Upload
        | Type::Bytes
        | Type::List(_)
        | Type::Map(_, _)
        | Type::File
        | Type::Stream(_)
        | Type::AsyncStream(_)
        | Type::RowStream(_)
        | Type::Related(_)
        | Type::Socket
        | Type::Listener
        | Type::Task(_)
        | Type::Group
        | Type::Channel(_) => true,
        Type::Nullable(inner) => contains_owned(program, inner),
        Type::MapEntry(key, value) => {
            contains_owned(program, key) || contains_owned(program, value)
        }
        Type::Outputs(fields) => fields
            .iter()
            .any(|field| contains_owned(program, &field.ty)),
        Type::Named(id) => match &program.types[*id].shape {
            Shape::Record(fields) => fields
                .iter()
                .any(|field| contains_owned(program, &field.ty)),
            Shape::Choice(variants) => variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .any(|field| contains_owned(program, &field.ty)),
        },
        _ => false,
    }
}

pub(super) fn hashable_key(program: &Program, ty: &Type) -> bool {
    match ty {
        Type::Bool | Type::Int | Type::Text | Type::Id => true,
        Type::Named(id) if program.types[*id].kind == DefinitionKind::ModelId => true,
        Type::Named(id) => matches!(
            &program.types[*id].shape,
            Shape::Choice(variants) if variants.iter().all(|variant| variant.fields.is_empty())
        ),
        _ => false,
    }
}

impl Module<'_> {
    pub(super) fn emit_type_equal(&mut self, index: usize, ty: &Type) -> Result<String, String> {
        let instance = self
            .instances
            .iter()
            .next()
            .expect("reachable entry")
            .clone();
        let llvm_type = self.ty(ty);
        let mut emitter = FunctionEmitter::new(self, &instance);
        emitter.start("entry");
        let left = emitter.temp();
        emitter.line(format!("{left} = load {llvm_type}, ptr %left"));
        let right = emitter.temp();
        emitter.line(format!("{right} = load {llvm_type}, ptr %right"));
        let equal = emitter.equals(ty, &left, &right)?;
        let value = emitter.temp();
        emitter.line(format!("{value} = zext i1 {equal} to i8"));
        emitter.line(format!("ret i8 {value}"));
        Ok(format!(
            "define internal i8 @dever_equal_{index}(ptr %left, ptr %right) {{\nentry:\n{} }}\n",
            emitter.body
        ))
    }

    pub(super) fn emit_type_hash(&self, index: usize, ty: &Type) -> String {
        if !hashable_key(self.program, ty) {
            return String::new();
        }
        let value = match ty {
            Type::Bool => "  %value = load i1, ptr %source\n  %hash = zext i1 %value to i64\n".into(),
            Type::Int => "  %hash = load i64, ptr %source\n".into(),
            Type::Named(id) if self.program.types[*id].kind == DefinitionKind::ModelId => "  %hash = load i64, ptr %source\n".into(),
            Type::Text | Type::Id => "  %value = load ptr, ptr %source\n  %hash = call i64 @dever_rt_v1_text_hash(ptr %value)\n".into(),
            Type::Named(id) => format!(
                "  %tag = getelementptr %T{id}, ptr %source, i32 0, i32 0\n  %value = load i32, ptr %tag\n  %hash = zext i32 %value to i64\n"
            ),
            _ => unreachable!("checked Map key"),
        };
        format!(
            "define internal i64 @dever_hash_{index}(ptr %source) {{\nentry:\n{value}  ret i64 %hash\n}}\n"
        )
    }
}

pub(super) fn collect_owned_types(program: &Program, ty: &Type, types: &mut Vec<Type>) {
    if types.contains(ty) {
        return;
    }
    types.push(ty.clone());
    match ty {
        Type::Uuid => collect_owned_types(program, &Type::Text, types),
        Type::Nullable(inner)
        | Type::List(inner)
        | Type::Stream(inner)
        | Type::AsyncStream(inner)
        | Type::RowStream(inner)
        | Type::Related(inner)
        | Type::Channel(inner) => collect_owned_types(program, inner, types),
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            collect_owned_types(program, key, types);
            collect_owned_types(program, value, types);
        }
        Type::Outputs(fields) | Type::Task(fields) => {
            for field in fields {
                collect_owned_types(program, &field.ty, types);
            }
            if matches!(ty, Type::Task(_)) {
                collect_owned_types(program, &output_type(fields), types);
            }
        }
        Type::Named(id) => match &program.types[*id].shape {
            Shape::Record(fields) => {
                for field in fields {
                    collect_owned_types(program, &field.ty, types);
                }
            }
            Shape::Choice(variants) => {
                for field in variants.iter().flat_map(|variant| &variant.fields) {
                    collect_owned_types(program, &field.ty, types);
                }
            }
        },
        _ => {}
    }
}

pub(super) struct TypeOwner<'a> {
    module: &'a Module<'a>,
    ir: String,
    next: usize,
}

impl<'a> TypeOwner<'a> {
    pub(super) fn new(module: &'a Module<'a>) -> Self {
        Self {
            module,
            ir: String::new(),
            next: 0,
        }
    }

    fn temp(&mut self) -> String {
        let next = self.next;
        self.next += 1;
        format!("%o{next}")
    }

    fn label(&mut self, name: &str) -> String {
        let next = self.next;
        self.next += 1;
        format!("{name}{next}")
    }

    fn line(&mut self, line: impl AsRef<str>) {
        writeln!(self.ir, "  {}", line.as_ref()).unwrap();
    }

    fn start(&mut self, label: &str) {
        writeln!(self.ir, "{label}:").unwrap();
    }

    pub(super) fn emit(mut self, index: usize, ty: &Type) -> String {
        let llvm_ty = self.module.ty(ty);
        writeln!(
            self.ir,
            "define internal void @dever_clone_{index}(ptr %src, ptr %dst) {{\nentry:"
        )
        .unwrap();
        let value = self.temp();
        self.line(format!("{value} = load {llvm_ty}, ptr %src"));
        self.line(format!("store {llvm_ty} {value}, ptr %dst"));
        self.adjust(ty, "%dst", true);
        self.line("ret void");
        self.ir.push_str("}\n");

        writeln!(
            self.ir,
            "define internal void @dever_drop_{index}(ptr %value) {{\nentry:"
        )
        .unwrap();
        self.adjust(ty, "%value", false);
        self.line("ret void");
        self.ir.push_str("}\n");
        self.ir
    }

    fn adjust(&mut self, ty: &Type, address: &str, retain: bool) {
        match ty {
            Type::Text
            | Type::Json
            | Type::Id
            | Type::Secret
            | Type::Uuid
            | Type::ClientTls
            | Type::ServerTls
            | Type::HttpClient
            | Type::HttpReply
            | Type::WebSocket
            | Type::Upload
            | Type::Bytes
            | Type::List(_)
            | Type::Map(_, _)
            | Type::File
            | Type::Stream(_)
            | Type::AsyncStream(_)
            | Type::RowStream(_)
            | Type::Related(_)
            | Type::Socket
            | Type::Listener
            | Type::Task(_)
            | Type::Group
            | Type::Channel(_) => {
                let kind = match ty {
                    Type::Text | Type::Json | Type::Id => "text",
                    Type::Secret => "secret",
                    Type::Uuid => "uuid",
                    Type::ClientTls => "client_tls",
                    Type::ServerTls => "server_tls",
                    Type::HttpClient => "http_client",
                    Type::HttpReply => "http_reply",
                    Type::WebSocket => "websocket",
                    Type::Upload => "upload",
                    Type::Bytes => "bytes",
                    Type::List(_) => "list",
                    Type::Map(_, _) => "map",
                    Type::File => "file",
                    Type::Stream(_) => "stream",
                    Type::AsyncStream(_) => "async_stream",
                    Type::RowStream(_) => "db_stream",
                    Type::Related(_) => "db_related",
                    Type::Socket => "socket",
                    Type::Listener => "listener",
                    Type::Task(_) => "async_task",
                    Type::Group => "async_group",
                    Type::Channel(_) => "async_channel",
                    _ => unreachable!(),
                };
                let handle = self.temp();
                self.line(format!("{handle} = load ptr, ptr {address}"));
                if retain {
                    let owned = self.temp();
                    self.line(format!(
                        "{owned} = call ptr @dever_rt_v1_{kind}_retain(ptr {handle})"
                    ));
                    self.line(format!("store ptr {owned}, ptr {address}"));
                } else {
                    self.line(format!(
                        "call void @dever_rt_v1_{kind}_release(ptr {handle})"
                    ));
                }
            }
            Type::Nullable(inner) => {
                let present_ptr = self.temp();
                self.line(format!(
                    "{present_ptr} = getelementptr {}, ptr {address}, i32 0, i32 0",
                    self.module.ty(ty)
                ));
                let present = self.temp();
                self.line(format!("{present} = load i1, ptr {present_ptr}"));
                let some = self.label("owned_some");
                let done = self.label("owned_done");
                self.line(format!("br i1 {present}, label %{some}, label %{done}"));
                self.start(&some);
                let payload = self.temp();
                self.line(format!(
                    "{payload} = getelementptr {}, ptr {address}, i32 0, i32 1",
                    self.module.ty(ty)
                ));
                self.adjust(inner, &payload, retain);
                self.line(format!("br label %{done}"));
                self.start(&done);
            }
            Type::Named(id) => match &self.module.program.types[*id].shape {
                Shape::Record(fields) => {
                    let fields = fields
                        .iter()
                        .map(|field| field.ty.clone())
                        .collect::<Vec<_>>();
                    self.fields(ty, address, &fields, retain);
                }
                Shape::Choice(variants) => {
                    let variants = variants
                        .iter()
                        .map(|variant| {
                            variant
                                .fields
                                .iter()
                                .map(|field| field.ty.clone())
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<Vec<Type>>>();
                    self.choice(*id, address, &variants, retain);
                }
            },
            Type::MapEntry(key, value) => {
                self.fields(
                    ty,
                    address,
                    &[key.as_ref().clone(), value.as_ref().clone()],
                    retain,
                );
            }
            Type::Outputs(fields) => {
                let fields = fields
                    .iter()
                    .map(|field| field.ty.clone())
                    .collect::<Vec<_>>();
                self.fields(ty, address, &fields, retain);
            }
            _ => {}
        }
    }

    fn fields(&mut self, ty: &Type, address: &str, fields: &[Type], retain: bool) {
        for (index, field) in fields.iter().enumerate() {
            if !contains_owned(self.module.program, field) {
                continue;
            }
            let pointer = self.temp();
            self.line(format!(
                "{pointer} = getelementptr {}, ptr {address}, i32 0, i32 {index}",
                self.module.ty(ty)
            ));
            self.adjust(field, &pointer, retain);
        }
    }

    fn choice(&mut self, id: usize, address: &str, variants: &[Vec<Type>], retain: bool) {
        let tag_ptr = self.temp();
        self.line(format!(
            "{tag_ptr} = getelementptr %T{id}, ptr {address}, i32 0, i32 0"
        ));
        let tag = self.temp();
        self.line(format!("{tag} = load i32, ptr {tag_ptr}"));
        let done = self.label("owned_choice_done");
        let arms = variants
            .iter()
            .map(|_| self.label("owned_variant"))
            .collect::<Vec<_>>();
        let cases = arms
            .iter()
            .enumerate()
            .map(|(index, arm)| format!("i32 {index}, label %{arm}"))
            .collect::<Vec<_>>()
            .join(" ");
        self.line(format!("switch i32 {tag}, label %{done} [{cases}]"));
        for (variant, fields) in variants.iter().enumerate() {
            self.start(&arms[variant]);
            let payload = self.temp();
            self.line(format!(
                "{payload} = getelementptr %T{id}, ptr {address}, i32 0, i32 1"
            ));
            for (index, field) in fields.iter().enumerate() {
                if !contains_owned(self.module.program, field) {
                    continue;
                }
                let pointer = self.temp();
                self.line(format!(
                    "{pointer} = getelementptr %T{id}V{variant}, ptr {payload}, i32 0, i32 {index}"
                ));
                self.adjust(field, &pointer, retain);
            }
            self.line(format!("br label %{done}"));
        }
        self.start(&done);
    }
}
