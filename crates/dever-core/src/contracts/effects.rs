use std::collections::BTreeSet;

use crate::diagnostic::{Diagnostic, Label};
use crate::hir::{
    CallArgument, CallTarget, CollectionOp, Expression, ExpressionKind, HandlerTarget, Program,
    SequenceKind,
};
use crate::intrinsic::Intrinsic;
use crate::types::{Parameter, Type};

use super::dependencies::Dependencies;

#[derive(Clone, Default, PartialEq, Eq)]
struct Effects {
    known: BTreeSet<&'static str>,
    handlers: BTreeSet<usize>,
    recoveries: BTreeSet<String>,
    databases: BTreeSet<crate::model::DatabaseOwner>,
    blocking: bool,
    suspends: bool,
    unmediated_io: bool,
    starts_api: bool,
}

impl Effects {
    fn include(&mut self, other: &Self) {
        self.known.extend(&other.known);
        self.handlers.extend(&other.handlers);
        self.recoveries.extend(other.recoveries.iter().cloned());
        self.databases.extend(&other.databases);
        self.blocking |= other.blocking;
        self.suspends |= other.suspends;
        self.unmediated_io |= other.unmediated_io;
        self.starts_api |= other.starts_api;
    }
}

pub(super) fn check(program: &mut Program, graph: &Dependencies, errors: &mut Vec<Diagnostic>) {
    let summaries = infer(program, graph);
    for (function, summary) in program.functions.iter_mut().zip(&summaries) {
        function.suspends = summary.suspends;
    }
    program.suspension_handlers = summaries
        .iter()
        .map(|summary| summary.handlers.clone())
        .collect();
    program.recoveries = summaries
        .iter()
        .map(|summary| summary.recoveries.clone())
        .collect();
    program.effects = summaries
        .iter()
        .map(|summary| {
            let mut effects = summary.known.clone();
            if !summary.handlers.is_empty() {
                effects.insert("handler");
            }
            effects
        })
        .collect();
    program.database_effects = summaries
        .iter()
        .map(|summary| summary.databases.clone())
        .collect();
    for (id, function) in program.functions.iter().enumerate() {
        if !function.pure || program.effects[id].is_empty() {
            continue;
        }
        let mut diagnostic = Diagnostic::error(
            "C013",
            format!(
                "pure function '{}' has effects: {}",
                function.name,
                program.effects[id]
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            function.span,
        );
        crate::check::visit(function, |expression| {
            let effects = expression_effects(expression, &summaries);
            if diagnostic.related.is_empty()
                && (!effects.known.is_empty() || !effects.handlers.is_empty())
            {
                diagnostic.related.push(Label {
                    span: expression.span,
                    message:
                        "this operation introduces an effect or invokes an unconstrained handler"
                            .into(),
                });
            }
        });
        errors.push(diagnostic);
    }
    validate_blocking_calls(program, &summaries, errors);
    validate_role_effects(program, &summaries, errors);
    validate_concurrency_calls(program, &summaries, errors);
    validate_suspending_signatures(program, &summaries, errors);
    validate_parallel_each_handlers(program, &summaries, errors);
    validate_suspending_blocking_boundaries(program, &summaries, errors);
    validate_transactions(program, &summaries, errors);
    crate::check::check_suspensions(program, errors);
}

fn validate_role_effects(program: &Program, summaries: &[Effects], errors: &mut Vec<Diagnostic>) {
    use crate::source::SourceRole;
    for (id, function) in program.functions.iter().enumerate() {
        let package = &program.packages[function.owner];
        if package.bundled {
            continue;
        }
        if matches!(package.role, Some(SourceRole::App | SourceRole::Domain))
            && summaries[id].unmediated_io
        {
            errors.push(Diagnostic::error(
                "C006",
                "App and Domain external I/O must pass through a Port",
                function.span,
            ));
        }
        if package.role.is_some() && summaries[id].starts_api {
            errors.push(Diagnostic::error(
                "C006",
                "only main may start the application API or Job worker",
                function.span,
            ));
        }
    }
    if let Ok(instances) = crate::specialize::concrete(program) {
        for instance in instances {
            let function = &program.functions[instance.function];
            if !matches!(
                program.packages[function.owner].role,
                Some(crate::source::SourceRole::App | crate::source::SourceRole::Domain)
            ) {
                continue;
            }
            let bindings = crate::specialize::handler_bindings(function, &instance);
            if summaries[instance.function]
                .handlers
                .iter()
                .any(|parameter| summaries[bindings[parameter]].unmediated_io)
            {
                errors.push(Diagnostic::error(
                    "C006",
                    "App and Domain handler I/O must pass through a Port",
                    function.span,
                ));
            }
        }
    }
}

fn infer(program: &Program, dependencies: &Dependencies) -> Vec<Effects> {
    let mut summaries = vec![Effects::default(); program.functions.len()];
    let mut work = dependencies.work();
    while let Some(id) = work.next() {
        let function = &program.functions[id];
        let mut next = summaries[id].clone();
        if let Some(external) = function.external {
            let crate::hir::AdapterImplementation::External(adapter) =
                &program.adapters[external.adapter].implementation
            else {
                unreachable!("external operation belongs to an external Adapter")
            };
            next.known.extend(&adapter.capabilities);
            next.known.insert("concurrency");
            next.suspends = true;
            next.unmediated_io = adapter.capabilities.contains("file")
                || adapter.capabilities.contains("network")
                || adapter.capabilities.contains("process");
        }
        if let Some(port) = &function.port {
            for implementation in &port.implementations {
                next.include(&summaries[*implementation]);
            }
            next.unmediated_io = false;
        }
        if let Some(reason) = &function.recovery {
            next.recoveries.insert(reason.clone());
        }
        crate::check::visit(function, |expression| {
            next.include(&expression_effects(expression, &summaries))
        });
        if next != summaries[id] {
            summaries[id] = next;
            work.changed(id);
        }
    }
    summaries
}

fn handler_effects(target: HandlerTarget, summaries: &[Effects]) -> Effects {
    match target {
        HandlerTarget::Function(id) => summaries[id].clone(),
        HandlerTarget::Parameter(id) => Effects {
            handlers: BTreeSet::from([id]),
            ..Effects::default()
        },
    }
}

fn expression_effects(expression: &Expression, summaries: &[Effects]) -> Effects {
    let mut result = Effects::default();
    match &expression.kind {
        ExpressionKind::ModelOperation { model, operation } => {
            result.known.insert("database");
            result
                .databases
                .insert(crate::model::DatabaseOwner::Model(*model));
            result.suspends = true;
            if matches!(
                operation.as_ref(),
                crate::hir::ModelOperation::Stream { .. }
            ) {
                result.known.insert("stream");
                result.known.insert("concurrency");
            }
        }
        ExpressionKind::JobEnqueue { target, .. } => {
            result.known.insert("database");
            result
                .databases
                .insert(crate::model::DatabaseOwner::Job(*target));
            result.suspends = true;
        }
        ExpressionKind::Intrinsic {
            operation, handler, ..
        } => {
            if let Some(handler) = handler {
                result.include(&handler_effects(*handler, summaries));
            }
            if let Some(effect) = intrinsic_effect(*operation) {
                result.known.insert(effect);
                result.unmediated_io |= matches!(effect, "file" | "network");
            }
            result.starts_api |= matches!(operation, Intrinsic::ApiServe | Intrinsic::JobServe);
            if operation.is_async() {
                if !matches!(operation, Intrinsic::UploadStore) {
                    result.known.insert("concurrency");
                }
                result.suspends = true;
            }
            result.blocking |= intrinsic_blocking(*operation);
        }
        ExpressionKind::Collection {
            operation,
            handler,
            sequence,
            ..
        } => {
            if let Some(handler) = handler {
                result.include(&handler_effects(*handler, summaries));
            }
            if matches!(
                sequence,
                Some(SequenceKind::Stream | SequenceKind::AsyncStream | SequenceKind::RowStream)
            ) || *operation == CollectionOp::Close
            {
                result.known.insert("stream");
            }
            if *operation == CollectionOp::ParallelEach
                || (*sequence == Some(SequenceKind::AsyncStream)
                    && *operation != CollectionOp::Close)
                || (*sequence == Some(SequenceKind::RowStream) && *operation != CollectionOp::Close)
            {
                result.known.insert("concurrency");
            } else if *sequence == Some(SequenceKind::Stream) {
                result.blocking = true;
            }
            if matches!(
                sequence,
                Some(SequenceKind::AsyncStream | SequenceKind::RowStream)
            ) && *operation != CollectionOp::Close
            {
                result.suspends = true;
            }
        }
        _ => {
            if let Some((target, arguments)) = expression.kind.static_call() {
                result.include(&call_effects(target, arguments, summaries));
                if matches!(
                    expression.kind,
                    ExpressionKind::RunCall { .. }
                        | ExpressionKind::ParallelCall { .. }
                        | ExpressionKind::BlockingCall { .. }
                ) {
                    result.known.insert("concurrency");
                    result.suspends = true;
                }
            } else if matches!(
                expression.kind,
                ExpressionKind::AwaitTask(_)
                    | ExpressionKind::StopTask(_)
                    | ExpressionKind::Group(_)
                    | ExpressionKind::AwaitGroup(_)
                    | ExpressionKind::StopGroup(_)
                    | ExpressionKind::Channel { .. }
                    | ExpressionKind::ChannelSend { .. }
                    | ExpressionKind::ChannelReceive(_)
                    | ExpressionKind::ChannelClose(_)
            ) {
                result.known.insert("concurrency");
                result.suspends |= !matches!(
                    expression.kind,
                    ExpressionKind::Group(_) | ExpressionKind::Channel { .. }
                );
            }
        }
    }
    result
}

fn call_effects(target: CallTarget, arguments: &[CallArgument], summaries: &[Effects]) -> Effects {
    match target {
        CallTarget::Handler(handler) => handler_effects(handler, summaries),
        CallTarget::Function(id) => {
            let mut result = Effects {
                known: summaries[id].known.clone(),
                recoveries: summaries[id].recoveries.clone(),
                databases: summaries[id].databases.clone(),
                blocking: summaries[id].blocking,
                suspends: summaries[id].suspends,
                unmediated_io: summaries[id].unmediated_io,
                starts_api: summaries[id].starts_api,
                ..Effects::default()
            };
            for parameter in &summaries[id].handlers {
                if let CallArgument::Handler(handler) = arguments[*parameter] {
                    result.include(&handler_effects(handler, summaries));
                }
            }
            result
        }
    }
}

fn validate_blocking_calls(program: &Program, summaries: &[Effects], errors: &mut Vec<Diagnostic>) {
    for function in &program.functions {
        crate::check::visit(function, |expression| {
            let ExpressionKind::BlockingCall { target, arguments } = &expression.kind else {
                return;
            };
            let effects = call_effects(*target, arguments, summaries);
            if call_may_suspend(&effects) {
                errors.push(Diagnostic::error(
                    "C005",
                    "blocking requires a non-suspending function call",
                    expression.span,
                ));
            } else if !effects.blocking {
                errors.push(Diagnostic::error(
                    "C005",
                    "blocking requires a synchronous call with a transitive blocking system effect",
                    expression.span,
                ));
            } else if !effects.handlers.is_empty() {
                errors.push(Diagnostic::error(
                    "C005",
                    "blocking cannot wrap a call with unconstrained handler effects",
                    expression.span,
                ));
            } else if effects.known.contains("concurrency") {
                errors.push(Diagnostic::error(
                    "C005",
                    "blocking cannot wrap a call that uses parallel_each; call parallel_each directly from suspending code",
                    expression.span,
                ));
            }
        });
    }
}

fn validate_concurrency_calls(
    program: &Program,
    summaries: &[Effects],
    errors: &mut Vec<Diagnostic>,
) {
    for function in &program.functions {
        crate::check::visit(function, |expression| {
            if let ExpressionKind::ParallelCall { target, arguments } = &expression.kind
                && call_may_suspend(&call_effects(*target, arguments, summaries))
            {
                errors.push(Diagnostic::error(
                    "C005",
                    "parallel requires a non-suspending pure function call",
                    expression.span,
                ));
            }
        });
    }
}

fn validate_suspending_signatures(
    program: &Program,
    summaries: &[Effects],
    errors: &mut Vec<Diagnostic>,
) {
    for (id, function) in program.functions.iter().enumerate() {
        if !call_may_suspend(&summaries[id]) {
            continue;
        }
        let inputs_transfer = function.parameters.iter().all(|parameter| match parameter {
            // Upload is owned by the same request across sequential suspension;
            // detached calls and containers still require transferable(), which is false.
            Parameter::Value(ty) => ty == &Type::Upload || ty.transferable(&program.types),
            Parameter::Handler(_) => true,
        });
        let outputs_transfer = function
            .outputs
            .iter()
            .all(|field| field.ty.transferable(&program.types));
        if !inputs_transfer || !outputs_transfer {
            errors.push(Diagnostic::error(
                "C005",
                "suspending function inputs and outputs must be transferable between threads",
                function.span,
            ));
        }
    }
}

fn validate_parallel_each_handlers(
    program: &Program,
    summaries: &[Effects],
    errors: &mut Vec<Diagnostic>,
) {
    for (id, function) in program.functions.iter().enumerate() {
        if !call_may_suspend(&summaries[id]) {
            continue;
        }
        crate::check::visit(function, |expression| {
            let ExpressionKind::Collection {
                operation: CollectionOp::ParallelEach,
                handler: Some(handler),
                sequence,
                ..
            } = &expression.kind
            else {
                return;
            };
            if matches!(
                sequence,
                Some(SequenceKind::AsyncStream | SequenceKind::RowStream)
            ) || handler_may_suspend(*handler, summaries)
            {
                return;
            }
            if handler_effects(*handler, summaries)
                .known
                .contains("concurrency")
            {
                errors.push(Diagnostic::error(
                    "C005",
                    "a suspending parallel_each handler cannot transitively use parallel_each or concurrency",
                    expression.span,
                ));
            }
        });
    }
}

fn validate_suspending_blocking_boundaries(
    program: &Program,
    summaries: &[Effects],
    errors: &mut Vec<Diagnostic>,
) {
    for (id, function) in program.functions.iter().enumerate() {
        if !call_may_suspend(&summaries[id]) {
            continue;
        }
        if let Some(port) = &function.port {
            for implementation in &port.implementations {
                let effects = &summaries[*implementation];
                if !call_may_suspend(effects) && requires_worker_boundary(effects) {
                    errors.push(Diagnostic::error(
                        "C005",
                        "a suspending Port cannot dispatch a blocking implementation inline; use an explicit worker boundary in the implementation",
                        program.functions[*implementation].span,
                    ));
                }
            }
        }
        crate::check::visit(function, |expression| {
            let blocks_runtime = match &expression.kind {
                ExpressionKind::Call { target, arguments }
                | ExpressionKind::CaptureResult {
                    target, arguments, ..
                } => {
                    let effects = call_effects(*target, arguments, summaries);
                    (!call_may_suspend(&effects) && requires_worker_boundary(&effects))
                        || bound_handler_requires_worker_boundary(*target, arguments, summaries)
                }
                ExpressionKind::Intrinsic {
                    operation, handler, ..
                } => {
                    intrinsic_blocking(*operation)
                        || handler.is_some_and(|handler| {
                            !handler_may_suspend(handler, summaries)
                                && requires_worker_boundary(&handler_effects(handler, summaries))
                        })
                }
                ExpressionKind::Collection {
                    operation,
                    sequence,
                    handler,
                    ..
                } => {
                    if *operation == CollectionOp::ParallelEach {
                        false
                    } else if matches!(
                        sequence,
                        Some(SequenceKind::AsyncStream | SequenceKind::RowStream)
                    ) || handler
                        .is_some_and(|handler| handler_may_suspend(handler, summaries))
                    {
                        handler.is_some_and(|handler| {
                            !handler_may_suspend(handler, summaries)
                                && requires_worker_boundary(&handler_effects(handler, summaries))
                        })
                    } else {
                        let effects = expression_effects(expression, summaries);
                        requires_worker_boundary(&effects)
                    }
                }
                _ => false,
            };
            if blocks_runtime {
                errors.push(Diagnostic::error(
                    "C005",
                    "a blocking or parallel operation cannot run inline inside a suspending function",
                    expression.span,
                ));
            }
        });
    }
}

fn requires_worker_boundary(effects: &Effects) -> bool {
    effects.blocking || effects.known.contains("concurrency") || !effects.handlers.is_empty()
}

fn bound_handler_requires_worker_boundary(
    target: CallTarget,
    arguments: &[CallArgument],
    summaries: &[Effects],
) -> bool {
    let CallTarget::Function(function) = target else {
        return false;
    };
    summaries[function].handlers.iter().any(|parameter| {
        let CallArgument::Handler(handler) = arguments[*parameter] else {
            unreachable!("inferred handler effect belongs to a handler parameter")
        };
        let effects = handler_effects(handler, summaries);
        !call_may_suspend(&effects) && requires_worker_boundary(&effects)
    })
}

fn call_may_suspend(effects: &Effects) -> bool {
    effects.suspends || !effects.handlers.is_empty()
}

fn validate_transactions(program: &Program, summaries: &[Effects], errors: &mut Vec<Diagnostic>) {
    for (id, function) in program.functions.iter().enumerate() {
        // Case-specific checking removes unreachable bodies after production
        // contracts were checked; those signatures are not empty transactions.
        if function.kind != crate::syntax::FunctionKind::Transaction || function.clauses.is_empty()
        {
            continue;
        }
        if summaries[id].databases.is_empty() && summaries[id].handlers.is_empty() {
            errors.push(Diagnostic::error(
                "C014",
                "transaction functions must perform a database operation",
                function.span,
            ));
        }
        let instance = crate::specialize::Specialization {
            function: id,
            handlers: Vec::new(),
        };
        let concurrent = crate::specialize::function_starts_transaction_unsafe_work(program, id)
            || (summaries[id].handlers.is_empty()
                && crate::specialize::starts_transaction_unsafe_work(program, &instance));
        if concurrent {
            errors.push(Diagnostic::error(
                "C014",
                "transaction functions cannot start concurrent work",
                function.span,
            ));
        }
        let password_crypto = summaries[id].handlers.is_empty()
            && (crate::specialize::uses_intrinsic(
                program,
                &instance,
                crate::intrinsic::Intrinsic::CryptoPasswordHash,
            ) || crate::specialize::uses_intrinsic(
                program,
                &instance,
                crate::intrinsic::Intrinsic::CryptoPasswordVerify,
            ));
        if password_crypto {
            errors.push(Diagnostic::error(
                "C014",
                "transaction functions cannot perform password hashing or verification",
                function.span,
            ));
        }
        let explicit = summaries[id]
            .databases
            .iter()
            .filter_map(|owner| match program.database_binding(*owner).0 {
                crate::model::ConnectionSelector::Explicit(name) => Some(name),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        if explicit.len() > 1 {
            errors.push(Diagnostic::error(
                "C014",
                "transaction spans multiple explicit database connections",
                function.span,
            ));
        }
    }
    let Ok(instances) = crate::specialize::concrete(program) else {
        return;
    };
    let mut missing_database = BTreeSet::new();
    let mut concurrent = BTreeSet::new();
    let mut password_crypto = BTreeSet::new();
    for instance in instances {
        let function = &program.functions[instance.function];
        if function.kind != crate::syntax::FunctionKind::Transaction {
            continue;
        }
        if !summaries[instance.function].handlers.is_empty()
            && (crate::specialize::uses_intrinsic(
                program,
                &instance,
                crate::intrinsic::Intrinsic::CryptoPasswordHash,
            ) || crate::specialize::uses_intrinsic(
                program,
                &instance,
                crate::intrinsic::Intrinsic::CryptoPasswordVerify,
            ))
        {
            password_crypto.insert(instance.function);
        }
        if summaries[instance.function].handlers.is_empty() {
            continue;
        }
        if crate::specialize::database_effects(program, &instance).is_empty() {
            missing_database.insert(instance.function);
        }
        if crate::specialize::starts_transaction_unsafe_work(program, &instance) {
            concurrent.insert(instance.function);
        }
    }
    for function in missing_database {
        errors.push(Diagnostic::error(
            "C014",
            "transaction functions must perform a database operation",
            program.functions[function].span,
        ));
    }
    for function in concurrent {
        errors.push(Diagnostic::error(
            "C014",
            "transaction functions cannot start concurrent work",
            program.functions[function].span,
        ));
    }
    for function in password_crypto {
        errors.push(Diagnostic::error(
            "C014",
            "transaction functions cannot perform password hashing or verification",
            program.functions[function].span,
        ));
    }
}

fn handler_may_suspend(handler: HandlerTarget, summaries: &[Effects]) -> bool {
    match handler {
        HandlerTarget::Function(id) => call_may_suspend(&summaries[id]),
        HandlerTarget::Parameter(_) => true,
    }
}

fn intrinsic_blocking(operation: Intrinsic) -> bool {
    use Intrinsic::*;
    matches!(
        operation,
        TimeSleep
            | CryptoPasswordHash
            | CryptoPasswordVerify
            | StdoutWrite
            | LogDebug
            | LogInfo
            | LogWarn
            | LogError
            | FileOpen
            | FileCreate
            | FileRead
            | FileWrite
            | FileClose
            | FileChunks
    )
}

// Exhaustive classification makes every new system primitive choose its effect boundary.
fn intrinsic_effect(operation: Intrinsic) -> Option<&'static str> {
    use Intrinsic::*;
    match operation {
        JobServe | TestJobDrain => Some("database"),
        TestClockAdvance => Some("time"),
        TaskTicks | TaskTimeout | TaskRace | StreamOf => Some("concurrency"),
        ProcessArguments => Some("process"),
        CryptoToken | CryptoPasswordHash => Some("random"),
        TimeNow | TimeUnixMillis | TimeMonotonicNanos | TimeSleep | TaskSleep => Some("time"),
        StdoutWrite => Some("stdout"),
        LogDebug | LogInfo | LogWarn | LogError => Some("log"),
        FileOpen | FileCreate | FileRead | FileWrite | FileClose | FileChunks => Some("file"),
        AuthIssue | AuthIssueCookie | AuthClearCookie | AuthOwnsUser | AuthId | AuthSession
        | AuthUserId | AuthTenantId | AuthPermissions | AuthSaveRole | AuthGrantRole
        | AuthRevokeRole | AuthDisableRole | SiteKey => Some("identity"),
        ApiRequestId
        | ApiMethod
        | ApiPath
        | ApiClientAddress
        | ApiHeader
        | ApiCookie
        | ApiSecretCookie
        | ApiResponseHeader
        | ApiResponseCookie
        | ApiResponseSecretCookie => Some("request"),
        UploadFilename | UploadContentType | UploadSize | UploadClose => Some("request"),
        UploadStore => Some("file"),
        TcpConnect | TcpConnectTimeout | TcpListen | TcpAccept | TcpPort | TcpRead | TcpWrite
        | TcpTimeout | TcpClose | TcpCloseListener | TcpChunks | TcpConnections | HttpServe
        | ApiServe | HttpSend | HttpServeLive | HttpRespond | HttpStart | HttpWrite
        | HttpFinish | SseStart | SseSend | WsAccept | WsConnect | WsSend | WsReceive
        | WsMessages | WsClose | TlsSystem | TlsClient | TlsServer | HttpClientNew
        | HttpRequest | HttpOpen | HttpOpenStream | HttpCloseClient | HttpServeTls
        | HttpServeLiveTls | WsOpen => Some("network"),
        TestAssert | TestAssertEq | TestSecret | CryptoPasswordVerify | CryptoSha256
        | CryptoHmacSha256 | CryptoConstantTimeEq | TextTrim | TextLower | TextUpper
        | TextCodepoint | TextFromCodepoint | TextAt | TextSlice | TextIndexOf | TextContains
        | TextStartsWith | TextEndsWith | TextSplit | TextReplace | IntParse | IntToText
        | DecimalParse | DecimalToText | DecimalFromInt | FloatParse | FloatToText
        | DecimalRound | FloatFromInt | FloatSqrt | FloatSin | FloatCos | FloatLog | FloatPow
        | FloatIsNan | FloatIsFinite | FloatIsInfinite | IdFromText | IdToText | UuidParse
        | UuidToText | BytesFromText | BytesToText | BytesFromInts | BytesLength | BytesAt
        | BytesSlice | BytesConcat | TimeParseDateTime | TimeFormatDateTime | TimeParseDate
        | TimeFormatDate | TimeParseTime | TimeFormatTime | TimeDuration | TimeAdd
        | TimeSubtract | TimeDifference => None,
    }
}
