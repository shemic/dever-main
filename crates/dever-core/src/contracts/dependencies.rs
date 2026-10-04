use std::collections::{BTreeSet, VecDeque};

use crate::hir::{CallArgument, CallTarget, HandlerTarget, Program};

pub(crate) struct Dependencies {
    callees: Vec<BTreeSet<usize>>,
    callers: Vec<Vec<usize>>,
    order: Vec<usize>,
}

impl Dependencies {
    pub fn new(program: &Program) -> Self {
        let mut dependencies = vec![BTreeSet::new(); program.functions.len()];
        for (caller, function) in program.functions.iter().enumerate() {
            if let Some(port) = &function.port {
                dependencies[caller].extend(&port.implementations);
            }
            crate::check::visit(function, |expression| {
                if let Some((target, arguments)) = expression.kind.static_call() {
                    if let CallTarget::Function(id)
                    | CallTarget::Handler(HandlerTarget::Function(id)) = target
                    {
                        dependencies[caller].insert(id);
                    }
                    for argument in arguments {
                        if let CallArgument::Handler(HandlerTarget::Function(id)) = argument {
                            dependencies[caller].insert(*id);
                        }
                    }
                    return;
                }
                if let Some(HandlerTarget::Function(id)) = expression.kind.callback() {
                    dependencies[caller].insert(id);
                }
            });
        }
        let mut callers = vec![Vec::new(); program.functions.len()];
        let mut pending: Vec<_> = dependencies.iter().map(BTreeSet::len).collect();
        for (caller, callees) in dependencies.iter().enumerate() {
            for callee in callees {
                callers[*callee].push(caller);
            }
        }
        let mut ready: BTreeSet<_> = pending
            .iter()
            .enumerate()
            .filter_map(|(id, count)| (*count == 0).then_some(id))
            .collect();
        let mut order = Vec::new();
        while let Some(id) = ready.pop_first() {
            order.push(id);
            for caller in &callers[id] {
                pending[*caller] -= 1;
                if pending[*caller] == 0 {
                    ready.insert(*caller);
                }
            }
        }
        // 未使用的 handler 引用可能形成摘要依赖环，但没有运行时递归。
        // Facts 沿用声明保证；单调摘要通过工作队列继续收敛。
        order.extend(
            pending
                .iter()
                .enumerate()
                .filter_map(|(id, count)| (*count != 0).then_some(id)),
        );
        Self {
            callees: dependencies,
            callers,
            order,
        }
    }

    pub fn reachable(&self, roots: impl IntoIterator<Item = usize>) -> BTreeSet<usize> {
        let mut reachable = BTreeSet::new();
        let mut pending: Vec<_> = roots.into_iter().collect();
        while let Some(function) = pending.pop() {
            if reachable.insert(function) {
                pending.extend(&self.callees[function]);
            }
        }
        reachable
    }

    pub(super) fn order(&self) -> &[usize] {
        &self.order
    }

    pub(super) fn work(&self) -> Work<'_> {
        Work {
            dependencies: self,
            ready: self.order.iter().copied().collect(),
            queued: vec![true; self.order.len()],
        }
    }
}

pub(super) struct Work<'a> {
    dependencies: &'a Dependencies,
    ready: VecDeque<usize>,
    queued: Vec<bool>,
}

impl Work<'_> {
    pub fn next(&mut self) -> Option<usize> {
        let id = self.ready.pop_front()?;
        self.queued[id] = false;
        Some(id)
    }

    pub fn changed(&mut self, id: usize) {
        for caller in &self.dependencies.callers[id] {
            if !self.queued[*caller] {
                self.queued[*caller] = true;
                self.ready.push_back(*caller);
            }
        }
    }
}
