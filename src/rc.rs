//! Inserts reference-count operations by liveness, in the style of
//! Perceus: a value is released right after its last use, and its last use
//! hands the reference on instead of copying it. A uniquely referenced
//! array can then be updated in place.

use std::collections::BTreeSet;

use crate::ir::*;

type Set = BTreeSet<VarId>;

pub fn insert(prog: &mut IrProgram) {
    for f in &mut prog.funcs {
        let managed: Vec<bool> = f.vars.iter().map(|v| prog.tys.managed(v.ty)).collect();
        let kinds: Vec<VarKind> = f.vars.iter().map(|v| v.kind).collect();
        let pass = Pass {
            managed: &managed,
            kinds: &kinds,
        };
        let body = std::mem::replace(&mut f.body, Ir::Fail);
        let (body, live) = pass.go(body, &Set::new(), &Set::new());
        // Parameters that are never used are released on entry.
        let mut body = body;
        for &p in f.params.iter().rev() {
            if pass.owned(p) && !live.contains(&p) {
                body = Ir::Dec(p, Box::new(body));
            }
        }
        f.body = body;
    }
}

struct Pass<'a> {
    managed: &'a [bool],
    kinds: &'a [VarKind],
}

impl<'a> Pass<'a> {
    fn owned(&self, v: VarId) -> bool {
        self.managed[v] && self.kinds[v] == VarKind::Owned
    }

    /// Adds `v` and every value it is borrowed from.
    fn add(&self, set: &mut Set, v: VarId) {
        let mut v = v;
        loop {
            if !self.managed[v] {
                return;
            }
            match self.kinds[v] {
                VarKind::Env | VarKind::Moved => return,
                VarKind::Owned => {
                    set.insert(v);
                    return;
                }
                VarKind::Borrowed(p) => {
                    set.insert(v);
                    v = p;
                }
            }
        }
    }

    fn descends(&self, u: VarId, v: VarId) -> bool {
        let mut u = u;
        loop {
            if u == v {
                return true;
            }
            match self.kinds[u] {
                VarKind::Borrowed(p) => u = p,
                _ => return false,
            }
        }
    }

    /// The operations around one instruction: increments before it, the
    /// live set before it, and releases after it.
    fn instr(&self, ops: &[(VarId, bool)], after: &Set) -> (Vec<VarId>, Set, Vec<VarId>) {
        let mut before = after.clone();
        for &(v, _) in ops {
            self.add(&mut before, v);
        }
        let mut incs = Vec::new();
        let mut moved = Set::new();
        for (k, &(v, own)) in ops.iter().enumerate() {
            if !own || !self.managed[v] {
                continue;
            }
            let can_move = self.kinds[v] == VarKind::Owned
                && !after.contains(&v)
                && !moved.contains(&v)
                && !ops.iter().any(|&(u, o)| !o && self.descends(u, v))
                && !ops[k + 1..].iter().any(|&(u, o)| o && u == v);
            if can_move {
                moved.insert(v);
            } else {
                incs.push(v);
            }
        }
        let decs = before
            .iter()
            .copied()
            .filter(|v| !after.contains(v) && self.owned(*v) && !moved.contains(v))
            .collect();
        (incs, before, decs)
    }

    fn wrap(incs: Vec<VarId>, inner: Ir) -> Ir {
        let mut ir = inner;
        for v in incs.into_iter().rev() {
            ir = Ir::Inc(v, Box::new(ir));
        }
        ir
    }

    fn wrap_dec(decs: Vec<VarId>, inner: Ir) -> Ir {
        let mut ir = inner;
        for v in decs.into_iter().rev() {
            ir = Ir::Dec(v, Box::new(ir));
        }
        ir
    }

    fn terminator(&self, ops: &[(VarId, bool)], after: &Set, term: Ir) -> (Ir, Set) {
        let (incs, before, decs) = self.instr(ops, after);
        (Self::wrap(incs, Self::wrap_dec(decs, term)), before)
    }

    fn go(&self, ir: Ir, ylive: &Set, flive: &Set) -> (Ir, Set) {
        match ir {
            Ir::Let(x, rhs, rest) => {
                let (rest, live_rest) = self.go(*rest, ylive, flive);
                let mut after = live_rest.clone();
                after.remove(&x);
                let rest = if self.owned(x) && !live_rest.contains(&x) {
                    Ir::Dec(x, Box::new(rest))
                } else {
                    rest
                };
                match rhs {
                    Rhs::Block(body) => {
                        let (body, before) = self.go(*body, &after, &Set::new());
                        (
                            Ir::Let(x, Rhs::Block(Box::new(body)), Box::new(rest)),
                            before,
                        )
                    }
                    rhs => {
                        let ops = rhs.operands();
                        let (incs, before, decs) = self.instr(&ops, &after);
                        let ir = Ir::Let(x, rhs, Box::new(Self::wrap_dec(decs, rest)));
                        (Self::wrap(incs, ir), before)
                    }
                }
            }
            Ir::Ret(v) => self.terminator(&[(v, true)], &Set::new(), Ir::Ret(v)),
            Ir::Yield(v) => self.terminator(&[(v, true)], ylive, Ir::Yield(v)),
            Ir::TailCall(vs) => {
                let ops: Vec<(VarId, bool)> = vs.iter().map(|&v| (v, true)).collect();
                self.terminator(&ops, &Set::new(), Ir::TailCall(vs))
            }
            Ir::Fail => (Ir::Fail, flive.clone()),
            Ir::If(c, t, e) => {
                let (t, lt) = self.go(*t, ylive, flive);
                let (e, le) = self.go(*e, ylive, flive);
                let mut before: Set = lt.union(&le).copied().collect();
                self.add(&mut before, c);
                let drop = |l: &Set| -> Vec<VarId> {
                    before
                        .iter()
                        .copied()
                        .filter(|v| !l.contains(v) && self.owned(*v))
                        .collect()
                };
                let t = Self::wrap_dec(drop(&lt), t);
                let e = Self::wrap_dec(drop(&le), e);
                (Ir::If(c, Box::new(t), Box::new(e)), before)
            }
            Ir::Chain(arms) => {
                let mut out = Vec::new();
                let mut next = Set::new();
                for arm in arms.into_iter().rev() {
                    let (a, l) = self.go(arm, ylive, &next);
                    out.push(a);
                    next = l;
                }
                out.reverse();
                (Ir::Chain(out), next)
            }
            Ir::Inc(..) | Ir::Dec(..) => unreachable!("reference counts are inserted once"),
        }
    }
}
