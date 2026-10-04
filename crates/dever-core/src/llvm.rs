//! Typed LLVM lowering for checked function kernels and switched-resume coroutines.
//!
//! Scalar kernels remain freestanding; managed values call the versioned
//! internal runtime ABI. Applications have a separate generated entry from
//! ordinary kernels. Unsupported reachable HIR is rejected with its
//! source location before any IR is returned.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write;

use crate::hir::{
    Atom, CallArgument, CallTarget, CollectionOp, Constant, Expression, ExpressionKind, Program,
    Projection, SequenceKind, Statement,
};
use crate::intrinsic::Intrinsic;
use crate::source::{SourceMap, Span};
use crate::specialize::{self, Specialization};
use crate::syntax::{BinaryOperator, FunctionKind, UnaryOperator};
use crate::types::{DefinitionKind, Parameter, Shape, Type, output_type};

pub use application::{
    BinaryResource, ResourceModule, emit_application, emit_application_with_resources,
    emit_executable, emit_executable_with_resources,
};
use ownership::{TypeOwner, collect_owned_types, contains_owned};
pub use testing::{emit_test_executable, emit_test_suite};

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultCode {
    Overflow = 1,
    DivisionByZero = 2,
    RuntimeAbi = 3,
    Business = 4,
}

/// Lower one checked, public, zero-input function into a directly callable
/// `i32 @dever_entry(ptr out, ptr fault)` kernel. `out` has the LLVM type
/// returned by `output_type`; the entry initializes both writable pointers.
/// Callers release transferred outputs and fault messages through the emitted
/// `dever_outputs_release` and `dever_fault_release` functions.
pub fn emit_kernel(program: &Program, sources: &SourceMap, entry: &str) -> Result<String, String> {
    let root = program.entry(entry)?;
    let application_span = program
        .api_routes
        .first()
        .map(|route| route.span)
        .or_else(|| program.api_rest.first().map(|route| route.span))
        .or_else(|| {
            program
                .api_commands
                .first()
                .map(|command| program.functions[command.function].span)
        })
        .or_else(|| {
            program
                .jobs
                .first()
                .map(|job| program.functions[job.function].span)
        });
    if let Some(span) = application_span {
        return Err(located(
            sources,
            span,
            "LLVM function kernels do not implement API, CMD, REST, or Job application entries",
        ));
    }
    let root = Specialization {
        function: root,
        handlers: Vec::new(),
    };
    let instances = specialize::reachable(&program.functions, [root.clone()])
        .map_err(|span| located(sources, span, "recursive specialized call"))?;
    for instance in &instances {
        validate_function(program, sources, instance, false)?;
    }
    let mut module = Module::new(program, sources, instances);
    module.validate_async_boundaries(sources)?;
    module.emit_types()?;
    module.emit_functions()?;
    module.emit_entry(&root);
    Ok(module.finish())
}

fn located(sources: &SourceMap, span: Span, message: &str) -> String {
    let source = sources.get(span.source);
    let (line, column) = source.position(span.start);
    format!("{}:{line}:{column}: {message}", source.path().display())
}

fn validate_function(
    program: &Program,
    sources: &SourceMap,
    instance: &Specialization,
    application: bool,
) -> Result<(), String> {
    let function = &program.functions[instance.function];
    if !application {
        let mut application_effect = function
            .parameters
            .iter()
            .any(|parameter| parameter.value_type() == Some(&Type::Upload))
            .then_some(function.span);
        crate::check::visit(function, |expression| {
            if expression.ty == Type::Upload {
                application_effect.get_or_insert(expression.span);
            }
            if let ExpressionKind::Intrinsic { operation, .. } = expression.kind
                && api_context::supported(operation)
            {
                application_effect.get_or_insert(expression.span);
            }
        });
        if let Some(span) = application_effect {
            return Err(located(
                sources,
                span,
                "LLVM function kernel does not support application context",
            ));
        }
    }
    if (!matches!(
        function.kind,
        FunctionKind::Ordinary | FunctionKind::Transaction | FunctionKind::Job { .. }
    ) || (!application && function.kind == FunctionKind::Transaction))
        || (!application && !specialize::database_effects(program, instance).is_empty())
        || (!application && function.port.is_some())
        || (!application && function.setting.is_some())
        || (!application && function.external.is_some())
    {
        return Err(located(
            sources,
            function.span,
            "LLVM function kernel requires an ordinary function without application effects",
        ));
    }
    for clause in &function.clauses {
        for statement in &clause.body {
            let expression = match statement {
                Statement::Assign { value, .. } | Statement::Call(value) => value,
            };
            validate_expression(sources, expression)?;
        }
    }
    Ok(())
}

fn validate_expression(sources: &SourceMap, expression: &Expression) -> Result<(), String> {
    let mut unsupported = None;
    crate::check::visit_expression(expression, |value| {
        let supported = match &value.kind {
            ExpressionKind::Intrinsic { operation, .. } => intrinsics::supported(*operation),
            ExpressionKind::Collection {
                operation,
                sequence,
                ..
            } => collections::supported(*operation, *sequence),
            ExpressionKind::Constant(_)
            | ExpressionKind::Local(_)
            | ExpressionKind::Field { .. }
            | ExpressionKind::Some(_)
            | ExpressionKind::Unary { .. }
            | ExpressionKind::Promote(_)
            | ExpressionKind::List(_)
            | ExpressionKind::Map(_)
            | ExpressionKind::Call { .. }
            | ExpressionKind::CaptureResult { .. }
            | ExpressionKind::Fail(_)
            | ExpressionKind::RunCall { .. }
            | ExpressionKind::ParallelCall { .. }
            | ExpressionKind::BlockingCall { .. }
            | ExpressionKind::AwaitTask(_)
            | ExpressionKind::StopTask(_)
            | ExpressionKind::Group(_)
            | ExpressionKind::AwaitGroup(_)
            | ExpressionKind::StopGroup(_)
            | ExpressionKind::ChannelReceive(_)
            | ExpressionKind::ChannelClose(_)
            | ExpressionKind::Channel { .. }
            | ExpressionKind::ChannelSend { .. }
            | ExpressionKind::JobEnqueue { .. }
            | ExpressionKind::Record { .. }
            | ExpressionKind::Variant { .. }
            | ExpressionKind::Binary { .. }
            | ExpressionKind::ModelOperation { .. } => true,
            _ => false,
        };
        if !supported {
            unsupported.get_or_insert(value.span);
        }
    });
    unsupported.map_or(Ok(()), |span| {
        Err(located(
            sources,
            span,
            "LLVM function kernel does not support this effect or expression",
        ))
    })
}

struct Module<'a> {
    program: &'a Program,
    sources: &'a SourceMap,
    instances: BTreeSet<Specialization>,
    names: BTreeMap<Specialization, String>,
    declarations: String,
    functions: String,
    locations: Vec<Span>,
    type_ids: BTreeSet<usize>,
    value_types: Vec<Type>,
    next_literal: usize,
    read_events: BTreeSet<usize>,
    parallel_functions: BTreeSet<Specialization>,
    http_handlers: BTreeSet<Specialization>,
    websocket_events: BTreeSet<usize>,
    upload_decoders: BTreeSet<usize>,
    application: Option<application::Application>,
    wire_codecs: BTreeMap<String, String>,
    database_errors: BTreeSet<(String, usize)>,
    database_decoders: BTreeSet<usize>,
    related_renderers: BTreeSet<usize>,
    api_permission_types: BTreeSet<usize>,
    rest_encoders: BTreeSet<(usize, bool)>,
}

impl<'a> Module<'a> {
    fn new(
        program: &'a Program,
        sources: &'a SourceMap,
        instances: BTreeSet<Specialization>,
    ) -> Self {
        let names = instances
            .iter()
            .cloned()
            .map(|instance| {
                let suffix = instance
                    .handlers
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("h");
                let name = format!("f{}h{suffix}", instance.function);
                (instance, name)
            })
            .collect();
        let mut type_ids = BTreeSet::new();
        let mut value_types = Vec::new();
        for instance in &instances {
            let function = &program.functions[instance.function];
            for parameter in &function.parameters {
                if let Parameter::Value(ty) = parameter {
                    collect_types(program, ty, &mut type_ids);
                    collect_owned_types(program, ty, &mut value_types);
                }
            }
            for field in &function.outputs {
                collect_types(program, &field.ty, &mut type_ids);
                collect_owned_types(program, &field.ty, &mut value_types);
            }
            let output = output_type(&function.outputs);
            collect_types(program, &output, &mut type_ids);
            collect_owned_types(program, &output, &mut value_types);
            collect_owned_types(
                program,
                &asynchronous::input_type(function),
                &mut value_types,
            );
            // A Port may declare failures that no implementation constructs.
            // Its typed fault ABI still needs their layouts and owner callbacks.
            for failure in specialize::failures(program, instance) {
                let ty = Type::Named(failure.ty);
                collect_types(program, &ty, &mut type_ids);
                collect_owned_types(program, &ty, &mut value_types);
            }
            for clause in &function.clauses {
                for ty in &clause.locals {
                    collect_types(program, ty, &mut type_ids);
                    collect_owned_types(program, ty, &mut value_types);
                }
            }
            crate::check::visit(function, |expression| {
                collect_types(program, &expression.ty, &mut type_ids);
                collect_owned_types(program, &expression.ty, &mut value_types);
            });
        }
        Self {
            program,
            sources,
            instances,
            names,
            declarations: String::new(),
            functions: String::new(),
            locations: Vec::new(),
            type_ids,
            value_types,
            next_literal: 0,
            read_events: BTreeSet::new(),
            parallel_functions: BTreeSet::new(),
            http_handlers: BTreeSet::new(),
            websocket_events: BTreeSet::new(),
            upload_decoders: BTreeSet::new(),
            application: None,
            wire_codecs: BTreeMap::new(),
            database_errors: BTreeSet::new(),
            database_decoders: BTreeSet::new(),
            related_renderers: BTreeSet::new(),
            api_permission_types: BTreeSet::new(),
            rest_encoders: BTreeSet::new(),
        }
    }

    fn location(&mut self, span: Span) -> usize {
        if let Some(index) = self.locations.iter().position(|item| *item == span) {
            return index;
        }
        let index = self.locations.len();
        self.locations.push(span);
        index
    }

    fn ty(&self, ty: &Type) -> String {
        match ty {
            Type::Unit => "{}".into(),
            Type::Bool => "i1".into(),
            Type::Int | Type::DateTime | Type::Date | Type::Time | Type::Duration => "i64".into(),
            Type::Float => "double".into(),
            Type::Decimal => "{ i64, i64 }".into(),
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
            | Type::Channel(_) => "ptr".into(),
            Type::Nullable(inner) => format!("{{ i1, {} }}", self.ty(inner)),
            Type::Named(id) if self.program.types[*id].kind == DefinitionKind::ModelId => {
                "i64".into()
            }
            Type::Named(id) => format!("%T{id}"),
            Type::MapEntry(key, value) => format!("{{ {}, {} }}", self.ty(key), self.ty(value)),
            Type::Outputs(fields) => format!(
                "{{ {} }}",
                fields
                    .iter()
                    .map(|field| self.ty(&field.ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    fn bytes(&self, ty: &Type) -> usize {
        match ty {
            Type::Unit => 0,
            Type::Bool => 1,
            Type::Int
            | Type::DateTime
            | Type::Date
            | Type::Time
            | Type::Duration
            | Type::Secret
            | Type::Uuid
            | Type::ClientTls
            | Type::ServerTls
            | Type::HttpClient
            | Type::HttpReply
            | Type::WebSocket
            | Type::Upload
            | Type::Float
            | Type::Text
            | Type::Json
            | Type::Id
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
            | Type::Channel(_) => 8,
            Type::Decimal => 16,
            Type::Nullable(inner) => align(
                align(1, self.alignment(inner)) + self.bytes(inner),
                self.alignment(inner),
            ),
            Type::Outputs(fields) => self.fields_bytes(fields),
            Type::MapEntry(key, value) => self.pair_bytes(key, value),
            Type::Named(id) if self.program.types[*id].kind == DefinitionKind::ModelId => 8,
            Type::Named(id) => match &self.program.types[*id].shape {
                Shape::Record(fields) => self.fields_bytes(fields),
                Shape::Choice(variants) => {
                    8 + variants
                        .iter()
                        .map(|variant| align(self.fields_bytes(&variant.fields), 8))
                        .max()
                        .unwrap_or(0)
                }
            },
        }
    }

    fn alignment(&self, ty: &Type) -> usize {
        match ty {
            Type::Unit | Type::Bool => 1,
            Type::Int
            | Type::DateTime
            | Type::Date
            | Type::Time
            | Type::Duration
            | Type::Secret
            | Type::Uuid
            | Type::ClientTls
            | Type::ServerTls
            | Type::HttpClient
            | Type::HttpReply
            | Type::WebSocket
            | Type::Upload
            | Type::Float
            | Type::Decimal
            | Type::Text
            | Type::Json
            | Type::Id
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
            | Type::Channel(_) => 8,
            Type::Nullable(inner) => self.alignment(inner),
            Type::MapEntry(key, value) => self.alignment(key).max(self.alignment(value)),
            Type::Outputs(fields) => fields
                .iter()
                .map(|field| self.alignment(&field.ty))
                .max()
                .unwrap_or(1),
            Type::Named(id) if self.program.types[*id].kind == DefinitionKind::ModelId => 8,
            Type::Named(id) => match &self.program.types[*id].shape {
                Shape::Record(fields) => fields
                    .iter()
                    .map(|field| self.alignment(&field.ty))
                    .max()
                    .unwrap_or(1),
                Shape::Choice(_) => 8,
            },
        }
    }

    fn fields_bytes(&self, fields: &[crate::types::Field]) -> usize {
        let mut size = 0;
        let mut max_align = 1;
        for field in fields {
            let alignment = self.alignment(&field.ty);
            max_align = max_align.max(alignment);
            size = align(size, alignment) + self.bytes(&field.ty);
        }
        align(size, max_align)
    }

    fn pair_bytes(&self, first: &Type, second: &Type) -> usize {
        let second_start = align(self.bytes(first), self.alignment(second));
        align(
            second_start + self.bytes(second),
            self.alignment(first).max(self.alignment(second)),
        )
    }

    fn type_index(&self, ty: &Type) -> usize {
        self.value_types
            .iter()
            .position(|known| known == ty)
            .expect("reachable type collected before emission")
    }

    fn uses_runtime(&self) -> bool {
        self.instances
            .iter()
            .any(|instance| specialize::suspends(self.program, instance))
            || self
                .value_types
                .iter()
                .any(|ty| matches!(ty, Type::Decimal) || contains_owned(self.program, ty))
    }

    fn emit_types(&mut self) -> Result<(), String> {
        let capacity = self.instances.len() + 1;
        let failure_words = self
            .instances
            .iter()
            .flat_map(|instance| specialize::failures(self.program, instance))
            .map(|failure| align(self.bytes(&Type::Named(failure.ty)), 8) / 8)
            .max()
            .unwrap_or(0);
        writeln!(
            self.declarations,
            "%dever.fault = type {{ i32, i32, i32, [{capacity} x i32], {{ ptr, i64 }}, i32, i32, [{failure_words} x i64], ptr }}"
        )
        .unwrap();
        writeln!(
            self.declarations,
            "@dever_fault_capacity = constant i32 {capacity}"
        )
        .unwrap();
        for id in self.type_ids.iter().copied() {
            let definition = &self.program.types[id];
            if definition.kind == DefinitionKind::ModelId {
                continue;
            }
            match &definition.shape {
                Shape::Record(fields) => {
                    let body = fields
                        .iter()
                        .map(|field| self.ty(&field.ty))
                        .collect::<Vec<_>>()
                        .join(", ");
                    writeln!(self.declarations, "%T{id} = type {{ {body} }}").unwrap();
                }
                Shape::Choice(variants) => {
                    let words = variants
                        .iter()
                        .map(|variant| align(self.fields_bytes(&variant.fields), 8) / 8)
                        .max()
                        .unwrap_or(0);
                    writeln!(
                        self.declarations,
                        "%T{id} = type {{ i32, [{words} x i64] }}"
                    )
                    .unwrap();
                    for (variant, definition) in variants.iter().enumerate() {
                        let body = definition
                            .fields
                            .iter()
                            .map(|field| self.ty(&field.ty))
                            .collect::<Vec<_>>()
                            .join(", ");
                        writeln!(self.declarations, "%T{id}V{variant} = type {{ {body} }}")
                            .unwrap();
                    }
                }
            }
        }
        self.declarations
            .push_str("declare { i64, i1 } @llvm.sadd.with.overflow.i64(i64, i64)\n");
        self.declarations
            .push_str("declare { i64, i1 } @llvm.ssub.with.overflow.i64(i64, i64)\n");
        self.declarations
            .push_str("declare { i64, i1 } @llvm.smul.with.overflow.i64(i64, i64)\n");
        if self.uses_runtime() {
            self.declarations
                .push_str("declare void @dever_rt_v1_buffer_free(ptr, i64)\n");
            for kind in [
                "text",
                "bytes",
                "list",
                "map",
                "file",
                "stream",
                "secret",
                "uuid",
                "client_tls",
                "server_tls",
                "http_client",
                "http_reply",
                "websocket",
                "upload",
            ] {
                writeln!(
                    self.declarations,
                    "declare ptr @dever_rt_v1_{kind}_retain(ptr)\ndeclare void @dever_rt_v1_{kind}_release(ptr)"
                )
                .unwrap();
            }
            self.declarations.push_str(abi::DECLARATIONS);
            self.declarations.push_str(system::DECLARATIONS);
            self.declarations.push_str(http::DECLARATIONS);
        }
        self.declarations
            .push_str("%dever.type = type { i64, i64, ptr, ptr, ptr, ptr }\n");
        let helpers = self
            .value_types
            .iter()
            .enumerate()
            .map(|(index, ty)| TypeOwner::new(self).emit(index, ty))
            .collect::<String>();
        self.declarations.push_str(&helpers);
        for (index, ty) in self.value_types.clone().iter().enumerate() {
            let equal = self.emit_type_equal(index, ty)?;
            let hash_function = self.emit_type_hash(index, ty);
            self.declarations.push_str(&equal);
            self.declarations.push_str(&hash_function);
            let hash = if ownership::hashable_key(self.program, ty) {
                format!("@dever_hash_{index}")
            } else {
                "null".into()
            };
            writeln!(
                self.declarations,
                "@dever_type_{index} = private constant %dever.type {{ i64 {}, i64 {}, ptr @dever_clone_{index}, ptr @dever_drop_{index}, ptr @dever_equal_{index}, ptr {hash} }}",
                self.bytes(ty), self.alignment(ty),
            )
            .unwrap();
        }
        self.emit_async_types();
        Ok(())
    }

    fn emit_functions(&mut self) -> Result<(), String> {
        for instance in self.instances.clone() {
            if let Some(index) = self.application.as_ref().and_then(|app| {
                app.job_handlers
                    .iter()
                    .position(|function| *function == instance.function)
            }) {
                self.emit_job_body(&instance, index)?;
                continue;
            }
            if let Some(handler) = self
                .application
                .as_ref()
                .and_then(|app| {
                    app.rest_handlers
                        .iter()
                        .find(|handler| handler.function == instance.function)
                })
                .cloned()
            {
                self.emit_rest_body(&instance, &handler)?;
                continue;
            }
            if let Some(index) = self.application.as_ref().and_then(|app| {
                app.command_handlers
                    .iter()
                    .position(|function| *function == instance.function)
            }) {
                self.emit_command_body(&instance, index)?;
                continue;
            }
            if let Some(index) = self.application.as_ref().and_then(|app| {
                app.api_handlers
                    .iter()
                    .position(|function| *function == instance.function)
            }) {
                self.emit_api_body(&instance, index)?;
                continue;
            }
            if self
                .application
                .as_ref()
                .is_some_and(|app| app.root == instance.function)
            {
                if self.application.as_ref().unwrap().test.is_some() {
                    self.emit_test_body(&instance)?;
                } else {
                    self.emit_application_body(&instance)?;
                }
                continue;
            }
            let emitter = FunctionEmitter::new(self, &instance);
            let body = emitter.emit()?;
            self.functions.push_str(&body);
        }
        Ok(())
    }

    fn emit_entry(&mut self, root: &Specialization) {
        self.emit_named_entry(root, "dever_entry", false);
    }

    fn emit_named_entry(&mut self, root: &Specialization, name: &str, internal: bool) {
        let function = &self.program.functions[root.function];
        let output = output_type(&function.outputs);
        let linkage = if internal { "internal " } else { "" };
        writeln!(
            self.functions,
            "define {linkage}i32 @{name}(ptr %out, ptr %fault) {{\nentry:"
        )
        .unwrap();
        writeln!(
            self.functions,
            "  store {} zeroinitializer, ptr %out",
            self.ty(&output)
        )
        .unwrap();
        self.functions
            .push_str("  store %dever.fault zeroinitializer, ptr %fault\n");
        if specialize::suspends(self.program, root) {
            self.emit_async_entry(root);
        } else {
            writeln!(
                self.functions,
                "  %status = call i32 @{}(ptr %out, ptr %fault)",
                self.names[root]
            )
            .unwrap();
        }
        let uses_log = self.instances.iter().any(|instance| {
            let mut found = false;
            crate::check::visit(&self.program.functions[instance.function], |expression| {
                found |= matches!(
                    expression.kind,
                    ExpressionKind::Intrinsic {
                        operation: Intrinsic::LogDebug
                            | Intrinsic::LogInfo
                            | Intrinsic::LogWarn
                            | Intrinsic::LogError
                            | Intrinsic::HttpServe
                            | Intrinsic::HttpServeTls
                            | Intrinsic::HttpServeLive
                            | Intrinsic::HttpServeLiveTls,
                        ..
                    }
                );
            });
            found
        });
        if uses_log {
            self.functions
                .push_str("  call void @dever_rt_v1_log_flush()\n");
        }
        self.functions.push_str("  ret i32 %status\n}\n");
        self.functions
            .push_str("define void @dever_outputs_release(ptr %out) {\nentry:\n");
        if contains_owned(self.program, &output) {
            writeln!(
                self.functions,
                "  call void @dever_drop_{}(ptr %out)",
                self.type_index(&output)
            )
            .unwrap();
        }
        self.functions.push_str("  ret void\n}\n");
        self.functions
            .push_str("define void @dever_fault_release(ptr %fault) {\nentry:\n");
        let failures = self
            .instances
            .iter()
            .flat_map(|instance| specialize::failures(self.program, instance))
            .map(|failure| failure.ty)
            .collect::<BTreeSet<_>>();
        if !failures.is_empty() {
            self.functions.push_str("  %kind = getelementptr %dever.fault, ptr %fault, i32 0, i32 5\n  %failure_type = load i32, ptr %kind\n");
            let arms = failures
                .iter()
                .map(|id| format!("i32 {}, label %drop_failure_{id}", id + 1))
                .collect::<Vec<_>>()
                .join(" ");
            writeln!(
                self.functions,
                "  switch i32 %failure_type, label %message_release [{arms}]"
            )
            .unwrap();
            for id in failures {
                writeln!(self.functions, "drop_failure_{id}:").unwrap();
                if contains_owned(self.program, &Type::Named(id)) {
                    writeln!(self.functions, "  %payload_{id} = getelementptr %dever.fault, ptr %fault, i32 0, i32 7\n  call void @dever_drop_{}(ptr %payload_{id})", self.type_index(&Type::Named(id))).unwrap();
                }
                self.functions.push_str("  br label %message_release\n");
            }
            self.functions.push_str("message_release:\n");
        }
        if self.uses_runtime() {
            self.functions.push_str(concat!(
                "  %message = getelementptr %dever.fault, ptr %fault, i32 0, i32 4\n",
                "  %buffer = load { ptr, i64 }, ptr %message\n",
                "  %bytes = extractvalue { ptr, i64 } %buffer, 0\n",
                "  %length = extractvalue { ptr, i64 } %buffer, 1\n",
                "  call void @dever_rt_v1_buffer_free(ptr %bytes, i64 %length)\n",
                "  %cause_ptr = getelementptr %dever.fault, ptr %fault, i32 0, i32 8\n",
                "  %cause = load ptr, ptr %cause_ptr\n",
                "  call void @dever_rt_v1_text_release(ptr %cause)\n",
            ));
        }
        self.functions
            .push_str("  store %dever.fault zeroinitializer, ptr %fault\n  ret void\n}\n");
        debug_assert!(function.parameters.is_empty());
    }

    fn finish(mut self) -> String {
        if !self.http_handlers.is_empty()
            || self
                .application
                .as_ref()
                .is_some_and(|app| app.test.is_some() || app.executable)
        {
            self.emit_location_renderer();
        }
        let mut ir = String::from("; Dever checked typed function kernel\n");
        ir.push_str(&self.declarations);
        writeln!(
            ir,
            "@dever_source_span_count = constant i32 {}",
            self.locations.len()
        )
        .unwrap();
        let spans = self
            .locations
            .iter()
            .map(|span| {
                format!(
                    "{{ i64, i64, i64 }} {{ i64 {}, i64 {}, i64 {} }}",
                    span.source.0, span.start, span.end
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            ir,
            "@dever_source_spans = constant [{} x {{ i64, i64, i64 }}] [{spans}]",
            self.locations.len()
        )
        .unwrap();
        ir.push_str(&self.functions);
        match self.application.as_ref().and_then(|app| app.test) {
            Some(index) => testing::namespace_module(&ir, index),
            None => ir,
        }
    }
}

fn align(size: usize, alignment: usize) -> usize {
    (size + alignment - 1) & !(alignment - 1)
}

fn collect_types(program: &Program, ty: &Type, ids: &mut BTreeSet<usize>) {
    match ty {
        Type::Named(id) if ids.insert(*id) => match &program.types[*id].shape {
            Shape::Record(fields) => fields
                .iter()
                .for_each(|field| collect_types(program, &field.ty, ids)),
            Shape::Choice(variants) => variants
                .iter()
                .flat_map(|variant| &variant.fields)
                .for_each(|field| collect_types(program, &field.ty, ids)),
        },
        Type::Nullable(inner)
        | Type::List(inner)
        | Type::Stream(inner)
        | Type::AsyncStream(inner)
        | Type::RowStream(inner)
        | Type::Related(inner)
        | Type::Channel(inner) => collect_types(program, inner, ids),
        Type::Map(key, value) | Type::MapEntry(key, value) => {
            collect_types(program, key, ids);
            collect_types(program, value, ids);
        }
        Type::Outputs(fields) | Type::Task(fields) => fields
            .iter()
            .for_each(|field| collect_types(program, &field.ty, ids)),
        _ => {}
    }
}

struct FunctionEmitter<'a, 'b> {
    module: &'a mut Module<'b>,
    instance: &'a Specialization,
    prologue: String,
    body: String,
    next: usize,
    block: String,
    locals: Vec<(Type, String)>,
    local_guards: Vec<Option<usize>>,
    guards: Vec<OwnedSlot>,
    moves: HashSet<*const Expression>,
    bindings: BTreeMap<usize, usize>,
    asynchronous: bool,
    parameters: BTreeMap<usize, String>,
    database_context: String,
    database_errors_override: Option<String>,
    protocol_callback: bool,
    transactions: Vec<transaction::TransactionScope>,
}

struct OwnedSlot {
    kind: OwnedKind,
    pointer: String,
    live: String,
}

enum OwnedKind {
    Fault,
    Value(Type),
    Output(Type),
    Protocol(&'static str),
    ListCursor,
    MapCursor,
    AsyncOp,
    AsyncStreamScope,
    DatabaseStreamScope,
}

impl<'a, 'b> FunctionEmitter<'a, 'b> {
    fn new(module: &'a mut Module<'b>, instance: &'a Specialization) -> Self {
        let function = &module.program.functions[instance.function];
        let bindings = specialize::handler_bindings(function, instance);
        let asynchronous = specialize::suspends(module.program, instance);
        Self {
            module,
            instance,
            prologue: String::new(),
            body: String::new(),
            next: 0,
            block: String::new(),
            locals: Vec::new(),
            local_guards: Vec::new(),
            guards: Vec::new(),
            moves: HashSet::new(),
            bindings,
            asynchronous,
            parameters: BTreeMap::new(),
            database_context: "null".into(),
            database_errors_override: None,
            protocol_callback: false,
            transactions: Vec::new(),
        }
    }

    fn temp(&mut self) -> String {
        let id = self.next;
        self.next += 1;
        format!("%v{id}")
    }
    fn label(&mut self, prefix: &str) -> String {
        let id = self.next;
        self.next += 1;
        format!("{prefix}{id}")
    }
    fn start(&mut self, label: &str) {
        self.block = label.to_owned();
        if label != "entry" {
            writeln!(self.body, "{label}:").unwrap();
        }
    }
    fn line(&mut self, line: impl AsRef<str>) {
        writeln!(self.body, "  {}", line.as_ref()).unwrap();
    }

    fn exit(&mut self, status: &str) {
        self.line(format!("store i32 {status}, ptr %exit_status"));
        self.line("br label %cleanup");
    }

    fn entry_slot(&mut self, ty: &Type) -> String {
        self.entry_slot_ir(&self.module.ty(ty))
    }

    fn entry_slot_ir(&mut self, ty: &str) -> String {
        let pointer = self.temp();
        writeln!(self.prologue, "  {pointer} = alloca {ty}").unwrap();
        pointer
    }

    fn register_guard(&mut self, ty: &Type, pointer: String) -> usize {
        self.register_owned(OwnedKind::Value(ty.clone()), pointer)
    }

    fn register_owned(&mut self, kind: OwnedKind, pointer: String) -> usize {
        let live = self.temp();
        writeln!(
            self.prologue,
            "  {live} = alloca i1\n  store i1 0, ptr {live}"
        )
        .unwrap();
        let index = self.guards.len();
        self.guards.push(OwnedSlot {
            kind,
            pointer,
            live,
        });
        index
    }

    fn mark_live(&mut self, guard: usize) {
        self.line(format!("store i1 1, ptr {}", self.guards[guard].live));
    }

    fn own_value(&mut self, ty: &Type, value: &str, borrowed: bool) -> usize {
        let pointer = self.entry_slot(ty);
        let guard = self.register_guard(ty, pointer.clone());
        if borrowed {
            let source = self.entry_slot(ty);
            self.line(format!(
                "store {} {value}, ptr {source}",
                self.module.ty(ty)
            ));
            self.line(format!(
                "call void @dever_clone_{}(ptr {source}, ptr {pointer})",
                self.module.type_index(ty)
            ));
        } else {
            self.line(format!(
                "store {} {value}, ptr {pointer}",
                self.module.ty(ty)
            ));
        }
        self.mark_live(guard);
        guard
    }

    fn load_owned_value(&mut self, ty: &Type, guard: usize) -> String {
        let value = self.temp();
        self.line(format!(
            "{value} = load {}, ptr {}",
            self.module.ty(ty),
            self.guards[guard].pointer
        ));
        value
    }

    fn disarm(&mut self, guard: usize) {
        self.line(format!("store i1 0, ptr {}", self.guards[guard].live));
    }

    fn release_guard(&mut self, guard: usize) {
        let live_ptr = self.guards[guard].live.clone();
        let pointer = self.guards[guard].pointer.clone();
        let live = self.temp();
        self.line(format!("{live} = load i1, ptr {live_ptr}"));
        let release = self.label("release_owned");
        let done = self.label("release_done");
        self.line(format!("br i1 {live}, label %{release}, label %{done}"));
        self.start(&release);
        match &self.guards[guard].kind {
            OwnedKind::Fault => self.line(format!("call void @dever_fault_release(ptr {pointer})")),
            OwnedKind::Protocol(kind) => {
                let kind = *kind;
                let handle = self.temp();
                self.line(format!("{handle} = load ptr, ptr {pointer}"));
                self.line(format!(
                    "call void @dever_rt_v1_{kind}_release(ptr {handle})"
                ));
            }
            OwnedKind::Value(ty) => self.line(format!(
                "call void @dever_drop_{}(ptr {pointer})",
                self.module.type_index(ty)
            )),
            OwnedKind::Output(ty) => {
                let ty = ty.clone();
                self.line(format!(
                    "call void @dever_drop_{}(ptr {pointer})",
                    self.module.type_index(&ty)
                ));
                self.line(format!(
                    "store {} zeroinitializer, ptr {pointer}",
                    self.module.ty(&ty)
                ));
            }
            OwnedKind::ListCursor | OwnedKind::MapCursor => {
                let kind = if matches!(self.guards[guard].kind, OwnedKind::ListCursor) {
                    "list"
                } else {
                    "map"
                };
                let cursor = self.temp();
                self.line(format!("{cursor} = load ptr, ptr {pointer}"));
                self.line(format!(
                    "call void @dever_rt_v1_{kind}_cursor_release(ptr {cursor})"
                ));
            }
            OwnedKind::AsyncOp => {
                let operation = self.temp();
                self.line(format!("{operation} = load ptr, ptr {pointer}"));
                self.line(format!(
                    "call void @dever_rt_v1_async_op_release(ptr {operation})"
                ));
            }
            OwnedKind::AsyncStreamScope | OwnedKind::DatabaseStreamScope => {
                let kind = if matches!(self.guards[guard].kind, OwnedKind::DatabaseStreamScope) {
                    "db_stream"
                } else {
                    "async_stream"
                };
                let stream = self.temp();
                self.line(format!("{stream} = load ptr, ptr {pointer}"));
                let output = self.entry_slot_ir("i8");
                let error = self.entry_slot_ir("{ ptr, i64 }");
                // Closing a compiler-owned valid stream is infallible and must
                // not replace the original fault during structured cleanup.
                self.line(format!(
                    "call i32 @dever_rt_v1_{kind}_close(ptr {stream}, ptr {output}, ptr {error})"
                ));
            }
        }
        self.line(format!("store i1 0, ptr {live_ptr}"));
        self.line(format!("br label %{done}"));
        self.start(&done);
    }

    fn release_from(&mut self, first: usize) {
        for guard in (first..self.guards.len()).rev() {
            self.release_guard(guard);
        }
    }

    fn release_between(&mut self, first: usize, end: usize) {
        for guard in (first..end).rev() {
            self.release_guard(guard);
        }
    }

    fn emit(mut self) -> Result<String, String> {
        let function = self.module.program.functions[self.instance.function].clone();
        let name = self.module.names[self.instance].clone();
        let parameters = function
            .parameters
            .iter()
            .enumerate()
            .filter_map(|(index, parameter)| {
                parameter
                    .value_type()
                    .map(|ty| format!("{} %p{index}", self.module.ty(ty)))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut parameters = if parameters.is_empty() {
            String::new()
        } else {
            format!("{parameters}, ")
        };
        if self.module.database_function(self.instance) {
            parameters.push_str("ptr %database, ");
            self.database_context = "%database".into();
        }
        self.start("entry");
        self.prologue
            .push_str("  %exit_status = alloca i32\n  store i32 0, ptr %exit_status\n");
        if self.asynchronous {
            self.begin_coroutine(&function);
        }
        self.begin_transaction(&function);
        if self.emit_external_body(&function)? {
            return Ok(self.finish_function(&name, &parameters));
        }
        if self.emit_port_body(&function)? {
            return Ok(self.finish_function(&name, &parameters));
        }
        self.line("br label %clause0");
        for (index, clause) in function.clauses.iter().enumerate() {
            self.moves = crate::native::liveness::last_uses(clause);
            self.start(&format!("clause{index}"));
            let guard = self.clause_guard(clause)?;
            self.line(format!(
                "br i1 {guard}, label %body{index}, label %clause{}",
                index + 1
            ));
            self.start(&format!("body{index}"));
            self.locals = clause
                .locals
                .iter()
                .map(|ty| (ty.clone(), String::new()))
                .collect();
            self.local_guards.clear();
            for (slot, ty) in clause.locals.iter().enumerate() {
                let pointer = self.entry_slot(ty);
                let guard = contains_owned(self.module.program, ty)
                    .then(|| self.register_guard(ty, pointer.clone()));
                self.local_guards.push(guard);
                self.locals[slot].1 = pointer;
            }
            for binding in &clause.bindings {
                self.emit_binding(binding, &function)?;
            }
            for statement in &clause.body {
                self.emit_statement(statement)?;
            }
            if !clause.terminates {
                let output = output_type(&function.outputs);
                if !matches!(output, Type::Unit) {
                    let result = self.outputs(clause, &output)?;
                    self.line(format!(
                        "store {} {result}, ptr %out",
                        self.module.ty(&output)
                    ));
                    for slot in &clause.outputs {
                        if let Some(guard) = self.local_guards[*slot] {
                            self.disarm(guard);
                        }
                    }
                }
                self.exit("0");
            } else {
                self.line("unreachable");
            }
        }
        self.start(&format!("clause{}", function.clauses.len()));
        self.line("unreachable");
        Ok(self.finish_function(&name, &parameters))
    }

    fn finish_function(mut self, name: &str, parameters: &str) -> String {
        self.start("cleanup");
        self.finish_transaction();
        self.release_from(0);
        let status = self.temp();
        self.line(format!("{status} = load i32, ptr %exit_status"));
        if self.asynchronous {
            self.end_coroutine(&status);
            return format!(
                "define ptr @{name}({parameters}ptr %out, ptr %fault, ptr %state) presplitcoroutine {{\nentry:\n{}{} }}\n",
                self.prologue, self.body
            );
        }
        self.line(format!("ret i32 {status}"));
        format!(
            "define i32 @{name}({parameters}ptr %out, ptr %fault) {{\nentry:\n{}{} }}\n",
            self.prologue, self.body
        )
    }

    fn clause_guard(&mut self, clause: &crate::hir::Clause) -> Result<String, String> {
        let parameters = &self.module.program.functions[self.instance.function].parameters;
        let mut guards = Vec::new();
        for ((index, parameter), domain) in parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| p.value_type().is_some())
            .zip(&clause.patterns)
        {
            let ty = parameter.value_type().expect("filtered value parameter");
            let mut alternatives = Vec::new();
            for atom in domain {
                let parameter = self.parameter(index);
                alternatives.push(self.atom(atom, ty, &parameter)?);
            }
            guards.push(self.reduce_bool("or", alternatives, "0"));
        }
        Ok(self.reduce_bool("and", guards, "1"))
    }

    fn reduce_bool(&mut self, op: &str, values: Vec<String>, empty: &str) -> String {
        let mut values = values.into_iter();
        let Some(mut result) = values.next() else {
            return empty.into();
        };
        for value in values {
            let next = self.temp();
            self.line(format!("{next} = {op} i1 {result}, {value}"));
            result = next;
        }
        result
    }

    fn emit_binding(
        &mut self,
        binding: &crate::hir::Binding,
        function: &crate::hir::Function,
    ) -> Result<(), String> {
        let ty = function.parameters[binding.input]
            .value_type()
            .expect("checked value binding");
        let source = self.parameter(binding.input);
        if let (Projection::Payload { .. }, Type::Related(inner)) = (&binding.projection, ty.base())
        {
            let related = if matches!(ty, Type::Nullable(_)) {
                let payload = self.temp();
                self.line(format!(
                    "{payload} = extractvalue {} {source}, 1",
                    self.module.ty(ty)
                ));
                payload
            } else {
                source
            };
            let value = self.runtime_call(
                "dever_rt_v1_db_related_get",
                vec![format!("ptr {related}")],
                inner,
                false,
                None,
                function.span,
            );
            let pointer = self.locals[binding.slot].1.clone();
            self.line(format!(
                "store {} {value}, ptr {pointer}",
                self.module.ty(inner)
            ));
            if let Some(guard) = self.local_guards[binding.slot] {
                self.mark_live(guard);
            }
            return Ok(());
        }
        let (value, projected) = match binding.projection {
            Projection::Whole => (source, ty.clone()),
            Projection::NonNull => {
                let Type::Nullable(inner) = ty else {
                    unreachable!("checked non-null binding")
                };
                let value = self.temp();
                self.line(format!(
                    "{value} = extractvalue {} {source}, 1",
                    self.module.ty(ty)
                ));
                (value, *inner.clone())
            }
            Projection::Payload { variant, field } => {
                let nominal = ty.base();
                let Type::Named(id) = nominal else {
                    unreachable!("checked choice binding")
                };
                let Shape::Choice(variants) = &self.module.program.types[*id].shape else {
                    unreachable!()
                };
                let projected = variants[variant].fields[field].ty.clone();
                let value = if matches!(ty, Type::Nullable(_)) {
                    let temp = self.temp();
                    self.line(format!(
                        "{temp} = extractvalue {} {source}, 1",
                        self.module.ty(ty)
                    ));
                    temp
                } else {
                    source
                };
                let alloca = self.temp();
                self.line(format!("{alloca} = alloca %T{id}"));
                self.line(format!("store %T{id} {value}, ptr {alloca}"));
                let payload = self.temp();
                self.line(format!(
                    "{payload} = getelementptr %T{id}, ptr {alloca}, i32 0, i32 1"
                ));
                let field_ptr = self.temp();
                self.line(format!("{field_ptr} = getelementptr %T{id}V{variant}, ptr {payload}, i32 0, i32 {field}"));
                let result = self.temp();
                self.line(format!(
                    "{result} = load {}, ptr {field_ptr}",
                    self.module.ty(&projected)
                ));
                (result, projected)
            }
        };
        debug_assert_eq!(projected, self.locals[binding.slot].0);
        let destination = self.locals[binding.slot].1.clone();
        if let Some(guard) = self.local_guards[binding.slot] {
            let source = self.entry_slot(&projected);
            self.line(format!(
                "store {} {value}, ptr {source}",
                self.module.ty(&projected)
            ));
            self.line(format!(
                "call void @dever_clone_{}(ptr {source}, ptr {destination})",
                self.module.type_index(&projected)
            ));
            self.line(format!("store i1 1, ptr {}", self.guards[guard].live));
        } else {
            self.line(format!(
                "store {} {value}, ptr {destination}",
                self.module.ty(&projected)
            ));
        }
        Ok(())
    }

    fn outputs(&mut self, clause: &crate::hir::Clause, output: &Type) -> Result<String, String> {
        if let [slot] = clause.outputs.as_slice() {
            return Ok(self.load_local(*slot));
        }
        let Type::Outputs(fields) = output else {
            unreachable!("checked output arity")
        };
        let mut aggregate = "undef".to_owned();
        for (index, (slot, field)) in clause.outputs.iter().zip(fields).enumerate() {
            let value = self.load_local(*slot);
            let next = self.temp();
            self.line(format!(
                "{next} = insertvalue {} {aggregate}, {} {value}, {index}",
                self.module.ty(output),
                self.module.ty(&field.ty)
            ));
            aggregate = next;
        }
        Ok(aggregate)
    }

    fn load_local(&mut self, slot: usize) -> String {
        let ty = self.module.ty(&self.locals[slot].0);
        let value = self.temp();
        self.line(format!("{value} = load {ty}, ptr {}", self.locals[slot].1));
        value
    }

    fn field_pointer(&mut self, slot: usize, fields: &[usize]) -> (String, Type) {
        let mut pointer = self.locals[slot].1.clone();
        let mut ty = self.locals[slot].0.clone();
        for index in fields {
            let element_ty = match &ty {
                Type::Named(id) => match &self.module.program.types[*id].shape {
                    Shape::Record(fields) => fields[*index].ty.clone(),
                    Shape::Choice(_) => unreachable!("checked field place"),
                },
                Type::MapEntry(key, value) => {
                    if *index == 0 {
                        key.as_ref().clone()
                    } else {
                        value.as_ref().clone()
                    }
                }
                Type::Outputs(fields) => fields[*index].ty.clone(),
                _ => unreachable!("checked field place"),
            };
            let next = self.temp();
            self.line(format!(
                "{next} = getelementptr {}, ptr {pointer}, i32 0, i32 {index}",
                self.module.ty(&ty)
            ));
            pointer = next;
            ty = element_ty;
        }
        (pointer, ty)
    }

    fn emit_statement(&mut self, statement: &Statement) -> Result<(), String> {
        match statement {
            Statement::Assign {
                slot,
                fields,
                value,
            } => {
                let result = self.expression(value)?;
                let result_guard =
                    contains_owned(self.module.program, &value.ty).then(|| self.guards.len() - 1);
                let (pointer, ty) = self.field_pointer(*slot, fields);
                if fields.is_empty() {
                    if let Some(guard) = self.local_guards[*slot] {
                        self.release_guard(guard);
                    }
                } else if contains_owned(self.module.program, &ty) {
                    self.line(format!(
                        "call void @dever_drop_{}(ptr {pointer})",
                        self.module.type_index(&ty)
                    ));
                }
                self.line(format!(
                    "store {} {result}, ptr {pointer}",
                    self.module.ty(&ty)
                ));
                if let Some(guard) = result_guard {
                    self.disarm(guard);
                }
                if fields.is_empty()
                    && let Some(guard) = self.local_guards[*slot]
                {
                    self.line(format!("store i1 1, ptr {}", self.guards[guard].live));
                }
            }
            Statement::Call(value) => {
                let first = self.guards.len();
                self.expression(value)?;
                self.release_from(first);
            }
        }
        Ok(())
    }
}

mod abi;
mod api;
mod api_context;
mod application;
mod application_commands;
mod asynchronous;
mod collections;
mod concurrency;
mod database;
mod database_rows;
mod database_schema;
mod expressions;
mod external;
mod failures;
mod http;
mod http_handlers;
mod http_values;
mod intrinsics;
mod jobs;
mod network;
mod orm;
mod orm_read;
mod orm_relations;
mod orm_stream;
mod ownership;
mod parallel;
mod ports;
mod process_entry;
mod render;
mod resources;
mod rest;
mod system;
mod testing;
mod transaction;
mod traversal;
mod wire;
