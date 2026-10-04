//! Sequential calls borrow the current transaction; detached descriptors pass NULL.
use super::*;

#[derive(Clone)]
pub(super) struct TransactionScope {
    outer: String,
    finishing: String,
    transaction: String,
    output_guard: Option<usize>,
    span: Span,
}

impl FunctionEmitter<'_, '_> {
    pub(super) fn begin_transaction(&mut self, function: &crate::hir::Function) {
        if function.kind != FunctionKind::Transaction
            || !self.module.database_function(self.instance)
        {
            return;
        }
        self.begin_transaction_scope(
            self.instance,
            &output_type(&function.outputs),
            function.span,
        );
    }

    pub(super) fn begin_command_transaction(
        &mut self,
        instance: &Specialization,
        span: Span,
    ) -> bool {
        if !specialize::writes_database(self.module.program, instance) {
            return false;
        }
        self.begin_transaction_scope(instance, &Type::Unit, span);
        true
    }

    pub(super) fn begin_transaction_scope(
        &mut self,
        instance: &Specialization,
        output: &Type,
        span: Span,
    ) {
        let owners = specialize::database_effects(self.module.program, instance);
        let owner_database = *owners.first().expect("checked database transaction");
        let outer = self.entry_slot_ir("i1");
        let finishing = self.entry_slot_ir("i1");
        writeln!(
            self.prologue,
            "  store i1 0, ptr {outer}\n  store i1 0, ptr {finishing}"
        )
        .unwrap();
        let transaction = self.entry_slot_ir("ptr");
        let owner = self.register_owned(OwnedKind::Protocol("db_transaction"), transaction.clone());
        let context = self.entry_slot_ir("ptr");
        self.line(format!(
            "store ptr {}, ptr {context}",
            self.database_context
        ));
        let output_guard = (*output != Type::Unit)
            .then(|| self.register_owned(OwnedKind::Output(output.clone()), "%out".into()));
        self.transactions.push(TransactionScope {
            outer: outer.clone(),
            finishing,
            transaction: transaction.clone(),
            output_guard,
            span,
        });
        let fresh = self.temp();
        self.line(format!(
            "{fresh} = icmp eq ptr {}, null",
            self.database_context
        ));
        let begin = self.label("transaction_begin");
        let ready = self.label("transaction_ready");
        self.line(format!("br i1 {fresh}, label %{begin}, label %{ready}"));
        self.start(&begin);
        let database = self.owner_database(owner_database, span);
        let operation = self.database_operation("db_begin", vec![format!("ptr {database}")], span);
        let handle = self.await_database_handle(&operation, span);
        self.line(format!("store ptr {handle}, ptr {transaction}"));
        self.mark_live(owner);
        self.line(format!("store i1 1, ptr {outer}"));
        self.line(format!("store ptr {handle}, ptr {context}"));
        self.line(format!("br label %{ready}"));
        self.start(&ready);
        let handle = self.temp();
        self.line(format!("{handle} = load ptr, ptr {context}"));
        self.database_context = handle;
    }

    pub(super) fn finish_transaction(&mut self) {
        for scope in self.transactions.clone().into_iter().rev() {
            self.finish_transaction_scope(&scope);
        }
    }

    pub(super) fn commit_command_transaction(&mut self) {
        let scope = self
            .transactions
            .last()
            .expect("writing CMD transaction")
            .clone();
        self.line(format!("store i1 1, ptr {}", scope.finishing));
        let transaction = self.temp();
        self.line(format!(
            "{transaction} = load ptr, ptr {}",
            scope.transaction
        ));
        let operation = self.database_operation(
            "db_finish",
            vec![format!("ptr {transaction}"), "i8 1".into()],
            scope.span,
        );
        self.await_operation(&operation, &Type::Unit, scope.span, false);
        self.database_context = "null".into();
    }

    fn finish_transaction_scope(&mut self, scope: &TransactionScope) {
        let outer = self.temp();
        let finishing = self.temp();
        let cancelling = self.temp();
        self.line(format!("{outer} = load i1, ptr {}", scope.outer));
        self.line(format!("{finishing} = load i1, ptr {}", scope.finishing));
        self.line(format!("{cancelling} = load i1, ptr %cancelling"));
        let skip = self.temp();
        let fresh = self.temp();
        let needed = self.temp();
        self.line(format!("{skip} = or i1 {finishing}, {cancelling}"));
        self.line(format!("{fresh} = xor i1 {skip}, 1"));
        self.line(format!("{needed} = and i1 {outer}, {fresh}"));
        let finish = self.label("transaction_finish");
        let done = self.label("transaction_cleaned");
        self.line(format!("br i1 {needed}, label %{finish}, label %{done}"));
        self.start(&finish);
        self.line(format!("store i1 1, ptr {}", scope.finishing));
        let transaction = self.temp();
        self.line(format!(
            "{transaction} = load ptr, ptr {}",
            scope.transaction
        ));
        let status = self.temp();
        self.line(format!("{status} = load i32, ptr %exit_status"));
        let success = self.temp();
        self.line(format!("{success} = icmp eq i32 {status}, 0"));
        let commit = self.label("transaction_commit");
        let rollback = self.label("transaction_rollback");
        self.line(format!(
            "br i1 {success}, label %{commit}, label %{rollback}"
        ));
        self.start(&commit);
        if let Some(guard) = scope.output_guard {
            self.mark_live(guard);
        }
        let operation = self.database_operation(
            "db_finish",
            vec![format!("ptr {transaction}"), "i8 1".into()],
            scope.span,
        );
        self.await_operation(&operation, &Type::Unit, scope.span, false);
        if let Some(guard) = scope.output_guard {
            self.disarm(guard);
        }
        self.line(format!("br label %{done}"));
        self.start(&rollback);
        let descriptor = format!("@dever_fault_owned_{}", self.module.names[self.instance]);
        let primary = self.entry_slot_ir("%dever.fault");
        self.line(format!(
            "call void @dever_fault_move(ptr %fault, ptr {primary})"
        ));
        let primary_owner = self.register_owned(OwnedKind::Fault, primary.clone());
        self.mark_live(primary_owner);
        let operation = self.database_operation(
            "db_rollback_take",
            vec![
                format!("ptr {transaction}"),
                format!("ptr {primary}"),
                format!("ptr {descriptor}"),
            ],
            scope.span,
        );
        self.disarm(primary_owner);
        let (status, _, _, error) = self.poll_raw(&operation, &Type::Unit);
        let status = self.poll_status(&status, &error, scope.span);
        self.line(format!("store i32 {status}, ptr %exit_status"));
        self.line(format!("br label %{done}"));
        self.start(&done);
    }
}
