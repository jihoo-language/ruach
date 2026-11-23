//! Monomorphization and lowering to the intermediate language. Every
//! function is compiled once for each list of types the program uses it at.

use std::collections::HashMap;

use crate::ast::*;
use crate::builtins;
use crate::error::{Error, Pos, Result};
use crate::ir::*;
use crate::types::{Checked, Constraint, Namer, Res, Type, TypeDefKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ret,
    Yield,
}

enum Op {
    Let(VarId, Rhs),
    Check(VarId),
}

struct Frame {
    vars: Vec<VarInfo>,
    scopes: Vec<(String, VarId)>,
    captured: Vec<(String, VarId)>,
    captures: Vec<VarId>,
    prologue: Vec<(VarId, Rhs)>,
    stmts: Vec<(VarId, Rhs)>,
    env: Option<VarId>,
}

impl Frame {
    fn new() -> Frame {
        Frame {
            vars: vec![],
            scopes: vec![],
            captured: vec![],
            captures: vec![],
            prologue: vec![],
            stmts: vec![],
            env: None,
        }
    }
}

struct Lower<'a> {
    ck: &'a Checked,
    decls: &'a [Decl],
    tys: TyTable,
    funcs: Vec<Option<IrFunc>>,
    instances: HashMap<(String, Vec<Type>), FnId>,
    queue: Vec<(FnId, String, Vec<Type>)>,
    wrappers: HashMap<String, FnId>,
    frames: Vec<Frame>,
    subst: HashMap<u32, Type>,
    self_key: Option<(String, Vec<Type>)>,
}

pub fn lower(prog: &Program, ck: &Checked) -> Result<IrProgram> {
    let mut l = Lower {
        ck,
        decls: &prog.decls,
        tys: TyTable::new(),
        funcs: Vec::new(),
        instances: HashMap::new(),
        queue: Vec::new(),
        wrappers: HashMap::new(),
        frames: Vec::new(),
        subst: HashMap::new(),
        self_key: None,
    };
    let main = l.instance("main", vec![]);
    while let Some((id, name, inst)) = l.queue.pop() {
        let f = l.lower_fn(&name, &inst)?;
        l.funcs[id] = Some(f);
    }
    let funcs = l.funcs.into_iter().map(|f| f.unwrap()).collect();
    Ok(IrProgram {
        funcs,
        tys: l.tys,
        main,
    })
}

impl<'a> Lower<'a> {
    fn instance(&mut self, name: &str, inst: Vec<Type>) -> FnId {
        let key = (name.to_string(), inst);
        if let Some(&id) = self.instances.get(&key) {
            return id;
        }
        let id = self.funcs.len();
        self.funcs.push(None);
        self.instances.insert(key.clone(), id);
        self.queue.push((id, key.0, key.1));
        id
    }

    fn intern(&mut self, t: &Type) -> TyId {
        self.tys.intern(t, &self.ck.types)
    }

    fn conc(&self, t: &Type) -> Type {
        match t {
            Type::Var(v) => match self.subst.get(v) {
                Some(t) => t.clone(),
                None => Type::unit(),
            },
            Type::Param(_) => Type::unit(),
            Type::Con(n, ts) => Type::Con(n.clone(), ts.iter().map(|t| self.conc(t)).collect()),
            Type::Fn(ps, r) => Type::Fn(
                ps.iter().map(|t| self.conc(t)).collect(),
                Box::new(self.conc(r)),
            ),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.conc(t)).collect()),
        }
    }

    fn ty(&self, e: &Expr) -> Type {
        self.conc(&self.ck.expr_ty[e.id])
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().unwrap()
    }

    fn new_var(&mut self, t: &Type, kind: VarKind) -> VarId {
        let ty = self.intern(t);
        let kind = if matches!(kind, VarKind::Borrowed(_)) && !self.tys.managed(ty) {
            VarKind::Owned
        } else {
            kind
        };
        let f = self.frame();
        f.vars.push(VarInfo { ty, kind });
        f.vars.len() - 1
    }

    fn var_type(&self, v: VarId) -> Type {
        self.tys.types[self.frames.last().unwrap().vars[v].ty].clone()
    }

    fn push(&mut self, t: &Type, rhs: Rhs) -> VarId {
        let kind = match &rhs {
            Rhs::Peek(p, _) => VarKind::Borrowed(*p),
            Rhs::EnvLoad(_) => VarKind::Borrowed(self.frames.last().unwrap().env.unwrap()),
            _ => VarKind::Owned,
        };
        let v = self.new_var(t, kind);
        self.frame().stmts.push((v, rhs));
        v
    }

    fn finish(&mut self, term: Ir) -> Ir {
        let stmts = std::mem::take(&mut self.frame().stmts);
        let mut ir = term;
        for (v, rhs) in stmts.into_iter().rev() {
            ir = Ir::Let(v, rhs, Box::new(ir));
        }
        ir
    }

    fn sub(&mut self, f: impl FnOnce(&mut Self) -> Result<Ir>) -> Result<Ir> {
        let saved = std::mem::take(&mut self.frame().stmts);
        let r = f(self);
        self.frame().stmts = saved;
        r
    }

    fn lookup(&mut self, name: &str) -> Option<VarId> {
        let n = self.frames.len() - 1;
        self.lookup_in(n, name)
    }

    fn lookup_in(&mut self, fi: usize, name: &str) -> Option<VarId> {
        let f = &self.frames[fi];
        if let Some((_, v)) = f.scopes.iter().rev().find(|(n, _)| n == name) {
            return Some(*v);
        }
        if let Some((_, v)) = f.captured.iter().find(|(n, _)| n == name) {
            return Some(*v);
        }
        if fi == 0 {
            return None;
        }
        let outer = self.lookup_in(fi - 1, name)?;
        let ty = self.frames[fi - 1].vars[outer].ty;
        let t = self.tys.types[ty].clone();
        let env = self.frames[fi].env.unwrap();
        let kind = if self.tys.managed(ty) {
            VarKind::Borrowed(env)
        } else {
            VarKind::Owned
        };
        let f = &mut self.frames[fi];
        let idx = f.captures.len();
        f.captures.push(outer);
        f.vars.push(VarInfo { ty, kind });
        let inner = f.vars.len() - 1;
        f.prologue.push((inner, Rhs::EnvLoad(idx)));
        f.captured.push((name.to_string(), inner));
        let _ = t;
        Some(inner)
    }

    fn bind(&mut self, name: &str, v: VarId) {
        self.frame().scopes.push((name.to_string(), v));
    }

    // ---------------------------------------------------------- functions

    fn lower_fn(&mut self, name: &str, inst: &[Type]) -> Result<IrFunc> {
        let info = &self.ck.fns[name];
        let Decl::Fn(f) = &self.decls[info.decl] else {
            unreachable!()
        };
        self.subst = info
            .quantified
            .iter()
            .copied()
            .zip(inst.iter().cloned())
            .collect();
        let Type::Fn(ps, r) = info.scheme.ty.subst_params(inst) else {
            unreachable!()
        };
        self.self_key = Some((name.to_string(), inst.to_vec()));
        self.frames.push(Frame::new());
        let mut params = Vec::new();
        for (p, t) in f.params.iter().zip(&ps) {
            let t = self.conc(t);
            let v = self.new_var(&t, VarKind::Owned);
            self.bind(&p.name, v);
            params.push(v);
        }
        let body = self.tail(&f.body, Mode::Ret)?;
        let frame = self.frames.pop().unwrap();
        let ret = self.intern(&self.conc(&r));
        Ok(IrFunc {
            name: name.to_string(),
            params,
            ret,
            vars: frame.vars,
            body,
            env: None,
        })
    }

    fn lambda(&mut self, e: &Expr, params: &[Param], body: &Expr) -> Result<VarId> {
        let t = self.ty(e);
        let Type::Fn(ps, r) = &t else { unreachable!() };
        let mut fr = Frame::new();
        let env_ty = self.intern(&t);
        fr.vars.push(VarInfo {
            ty: env_ty,
            kind: VarKind::Env,
        });
        fr.env = Some(0);
        self.frames.push(fr);
        let mut pvars = Vec::new();
        for (p, pt) in params.iter().zip(ps) {
            let v = self.new_var(pt, VarKind::Owned);
            self.bind(&p.name, v);
            pvars.push(v);
        }
        let saved_key = self.self_key.take();
        let body_ir = self.tail(body, Mode::Ret);
        self.self_key = saved_key;
        let body_ir = body_ir?;
        let mut fr = self.frames.pop().unwrap();
        let mut ir = body_ir;
        for (v, rhs) in fr.prologue.drain(..).rev() {
            ir = Ir::Let(v, rhs, Box::new(ir));
        }
        let outer_tys: Vec<TyId> = fr
            .captures
            .iter()
            .map(|&v| self.frames.last().unwrap().vars[v].ty)
            .collect();
        let ret = self.intern(r);
        let id = self.funcs.len();
        self.funcs.push(Some(IrFunc {
            name: "lambda".into(),
            params: pvars,
            ret,
            vars: fr.vars,
            body: ir,
            env: Some((0, outer_tys)),
        }));
        Ok(self.push(&t, Rhs::MakeClo(id, fr.captures)))
    }

    /// A closure that calls a named function, builtin or constructor.
    fn wrapper(
        &mut self,
        key: String,
        t: &Type,
        call: impl FnOnce(&mut Self, Vec<VarId>) -> Result<Rhs>,
    ) -> Result<VarId> {
        let id = match self.wrappers.get(&key) {
            Some(&id) => id,
            None => {
                let Type::Fn(ps, r) = t else { unreachable!() };
                let mut fr = Frame::new();
                let env_ty = self.intern(t);
                fr.vars.push(VarInfo {
                    ty: env_ty,
                    kind: VarKind::Env,
                });
                fr.env = Some(0);
                self.frames.push(fr);
                let params: Vec<VarId> =
                    ps.iter().map(|p| self.new_var(p, VarKind::Owned)).collect();
                let rhs = call(self, params.clone());
                let rhs = match rhs {
                    Ok(r) => r,
                    Err(e) => {
                        self.frames.pop();
                        return Err(e);
                    }
                };
                let rv = self.push(r, rhs);
                let body = self.finish(Ir::Ret(rv));
                let fr = self.frames.pop().unwrap();
                let ret = self.intern(r);
                let id = self.funcs.len();
                self.funcs.push(Some(IrFunc {
                    name: key.clone(),
                    params,
                    ret,
                    vars: fr.vars,
                    body,
                    env: Some((0, vec![])),
                }));
                self.wrappers.insert(key, id);
                id
            }
        };
        Ok(self.push(t, Rhs::MakeClo(id, vec![])))
    }

    // -------------------------------------------------------- expressions

    fn tail(&mut self, e: &Expr, mode: Mode) -> Result<Ir> {
        let term = |v| {
            if mode == Mode::Ret {
                Ir::Ret(v)
            } else {
                Ir::Yield(v)
            }
        };
        match &e.kind {
            ExprKind::If(c, t, f) => {
                let cv = self.val(c)?;
                let ti = self.sub(|s| s.tail(t, mode))?;
                let fi = self.sub(|s| match f {
                    Some(f) => s.tail(f, mode),
                    None => {
                        let u = s.push(&Type::unit(), Rhs::Unit);
                        Ok(s.finish(term(u)))
                    }
                })?;
                Ok(self.finish(Ir::If(cv, Box::new(ti), Box::new(fi))))
            }
            ExprKind::Match(s, arms) => {
                let sv = self.val(s)?;
                let st = self.ty(s);
                let mut irs = Vec::new();
                for arm in arms {
                    let ir = self.sub(|l| l.arm(arm, sv, &st, mode))?;
                    irs.push(ir);
                }
                Ok(self.finish(Ir::Chain(irs)))
            }
            ExprKind::Block(stmts, tail) => {
                let depth = self.frame().scopes.len();
                for s in stmts {
                    self.stmt(s)?;
                }
                let r = match tail {
                    Some(t) => self.tail(t, mode),
                    None => {
                        let u = self.push(&Type::unit(), Rhs::Unit);
                        Ok(self.finish(term(u)))
                    }
                };
                self.frame().scopes.truncate(depth);
                r
            }
            ExprKind::Binary(op @ (BinOp::And | BinOp::Or), a, b) => {
                let av = self.val(a)?;
                let rest = self.sub(|s| s.tail(b, mode))?;
                let short = self.sub(|s| {
                    let k = s.push(&Type::bool(), Rhs::Bool(*op == BinOp::Or));
                    Ok(s.finish(term(k)))
                })?;
                Ok(self.finish(if *op == BinOp::And {
                    Ir::If(av, Box::new(rest), Box::new(short))
                } else {
                    Ir::If(av, Box::new(short), Box::new(rest))
                }))
            }
            ExprKind::Call(f, args) if mode == Mode::Ret && self.frames.len() == 1 => {
                if let Some(Res::Global(n, inst)) = self.ck.res.get(&f.id) {
                    let inst: Vec<Type> = inst.iter().map(|t| self.conc(t)).collect();
                    if self.lookup_name_is_global(f)
                        && self.self_key.as_ref() == Some(&(n.clone(), inst))
                    {
                        let mut vs = Vec::new();
                        for a in args {
                            vs.push(self.val(a)?);
                        }
                        return Ok(self.finish(Ir::TailCall(vs)));
                    }
                }
                let v = self.val(e)?;
                Ok(self.finish(term(v)))
            }
            ExprKind::Method(recv, _, args) if mode == Mode::Ret && self.frames.len() == 1 => {
                if let Some(Res::Global(n, inst)) = self.ck.res.get(&e.id) {
                    let inst: Vec<Type> = inst.iter().map(|t| self.conc(t)).collect();
                    if self.self_key.as_ref() == Some(&(n.clone(), inst)) {
                        let mut vs = vec![self.val(recv)?];
                        for a in args {
                            vs.push(self.val(a)?);
                        }
                        return Ok(self.finish(Ir::TailCall(vs)));
                    }
                }
                let v = self.val(e)?;
                Ok(self.finish(term(v)))
            }
            _ => {
                let v = self.val(e)?;
                Ok(self.finish(term(v)))
            }
        }
    }

    fn lookup_name_is_global(&mut self, f: &Expr) -> bool {
        match &f.kind {
            ExprKind::Var(n) => self.lookup(n).is_none(),
            ExprKind::Qual(..) => true,
            _ => false,
        }
    }

    fn stmt(&mut self, s: &Stmt) -> Result<()> {
        match s {
            Stmt::Let(p, _, e) => {
                let v = self.val(e)?;
                let t = self.ty(e);
                if self.take(p, v, &t) {
                    return Ok(());
                }
                let mut ops = Vec::new();
                self.pat(p, v, &t, &mut ops)?;
                for op in ops {
                    match op {
                        Op::Let(v, rhs) => self.frame().stmts.push((v, rhs)),
                        Op::Check(_) => unreachable!("refutable let"),
                    }
                }
            }
            Stmt::Expr(e) => {
                self.val(e)?;
            }
        }
        Ok(())
    }

    /// `let (a, b) = e` where `e` was just computed moves the fields out
    /// instead of borrowing them, so a unique array inside stays unique.
    fn take(&mut self, p: &Pat, v: VarId, t: &Type) -> bool {
        let fresh = matches!(self.frame().stmts.last(), Some((w, _)) if *w == v);
        let fields: Vec<(Type, Option<&Pat>)> = match (&p.kind, t) {
            (PatKind::Tuple(ps), Type::Tuple(ts)) => {
                ts.iter().cloned().zip(ps.iter().map(Some)).collect()
            }
            (PatKind::Record(name, fs), Type::Con(_, args)) => {
                let TypeDefKind::Record(defs) = &self.ck.types[name].kind else {
                    return false;
                };
                defs.iter()
                    .map(|(f, ft)| {
                        (
                            ft.subst_params(args),
                            fs.iter().find(|(g, _)| g == f).map(|(_, q)| q),
                        )
                    })
                    .collect()
            }
            _ => return false,
        };
        let ty = self.frame().vars[v].ty;
        if !fresh || self.frame().vars[v].kind != VarKind::Owned || !self.tys.managed(ty) {
            return false;
        }
        self.frame().vars[v].kind = VarKind::Moved;
        for (i, (ft, q)) in fields.into_iter().enumerate() {
            let fv = self.new_var(&ft, VarKind::Owned);
            self.frame().stmts.push((fv, Rhs::Peek(v, Peek::Field(i))));
            if let Some(q) = q {
                if !self.take(q, fv, &ft) {
                    let mut ops = Vec::new();
                    // Irrefutable: only bindings and borrows.
                    let _ = self.pat(q, fv, &ft, &mut ops);
                    for op in ops {
                        if let Op::Let(w, rhs) = op {
                            self.frame().stmts.push((w, rhs));
                        }
                    }
                }
            }
        }
        true
    }

    fn arm(&mut self, arm: &Arm, sv: VarId, st: &Type, mode: Mode) -> Result<Ir> {
        let depth = self.frame().scopes.len();
        let mut ops = Vec::new();
        if !self.unpack(arm, sv, st, &mut ops) {
            self.pat(&arm.pat, sv, st, &mut ops)?;
        }
        let body = match &arm.guard {
            Some(g) => {
                let gv = self.val(g)?;
                let b = self.sub(|s| s.tail(&arm.body, mode))?;
                self.finish(Ir::If(gv, Box::new(b), Box::new(Ir::Fail)))
            }
            None => self.tail(&arm.body, mode)?,
        };
        self.frame().scopes.truncate(depth);
        let mut ir = body;
        for op in ops.into_iter().rev() {
            ir = match op {
                Op::Let(v, rhs) => Ir::Let(v, rhs, Box::new(ir)),
                Op::Check(b) => Ir::If(b, Box::new(ir), Box::new(Ir::Fail)),
            };
        }
        Ok(ir)
    }

    /// `Cons(x, rest) => ...` on a value the match owns takes the fields
    /// out of the cell, which is freed at once if nothing else refers to it.
    fn unpack(&mut self, arm: &Arm, sv: VarId, st: &Type, ops: &mut Vec<Op>) -> bool {
        let PatKind::Ctor(c, ps) = &arm.pat.kind else {
            return false;
        };
        if arm.guard.is_some()
            || ps.is_empty()
            || !ps
                .iter()
                .all(|p| matches!(p.kind, PatKind::Var(_) | PatKind::Wild))
            || self.frame().vars[sv].kind != VarKind::Owned
        {
            return false;
        }
        let (tn, tag) = self.ck.ctors[c].clone();
        let Type::Con(_, args) = st else { return false };
        let TypeDefKind::Variant(cs) = &self.ck.types[&tn].kind else {
            return false;
        };
        if cs.len() > 1 {
            let b = self.op_var(
                &Type::bool(),
                Rhs::Prim(Prim::TagIs(tag as u32), vec![sv]),
                ops,
            );
            ops.push(Op::Check(b));
        }
        let fields: Vec<Type> = cs[tag]
            .fields
            .iter()
            .map(|f| f.subst_params(args))
            .collect();
        let mut owned = Vec::new();
        for (i, (p, ft)) in ps.iter().zip(&fields).enumerate() {
            let v = self.op_var(ft, Rhs::Own(sv, Peek::CtorField(tag as u32, i)), ops);
            if let PatKind::Var(n) = &p.kind {
                self.bind(n, v);
            }
            owned.push(v);
        }
        self.op_var(&Type::unit(), Rhs::Release(sv, owned), ops);
        true
    }

    fn op_var(&mut self, t: &Type, rhs: Rhs, ops: &mut Vec<Op>) -> VarId {
        let kind = match &rhs {
            Rhs::Peek(p, _) => VarKind::Borrowed(*p),
            _ => VarKind::Owned,
        };
        let v = self.new_var(t, kind);
        ops.push(Op::Let(v, rhs));
        v
    }

    fn pat(&mut self, p: &Pat, v: VarId, t: &Type, ops: &mut Vec<Op>) -> Result<()> {
        match &p.kind {
            PatKind::Wild | PatKind::Unit => {}
            PatKind::Var(n) => self.bind(n, v),
            PatKind::Int(k) => {
                let c = self.op_var(&Type::int(), Rhs::Int(*k), ops);
                let b = self.op_var(&Type::bool(), Rhs::Prim(Prim::Eq, vec![v, c]), ops);
                ops.push(Op::Check(b));
            }
            PatKind::Str(s) => {
                let c = self.op_var(&Type::string(), Rhs::Str(s.clone()), ops);
                let b = self.op_var(&Type::bool(), Rhs::Prim(Prim::Eq, vec![v, c]), ops);
                ops.push(Op::Check(b));
            }
            PatKind::Bool(true) => ops.push(Op::Check(v)),
            PatKind::Bool(false) => {
                let b = self.op_var(&Type::bool(), Rhs::Prim(Prim::Not, vec![v]), ops);
                ops.push(Op::Check(b));
            }
            PatKind::Tuple(ps) => {
                let Type::Tuple(ts) = t else { unreachable!() };
                for (i, (q, qt)) in ps.iter().zip(ts).enumerate() {
                    if matches!(q.kind, PatKind::Wild) {
                        continue;
                    }
                    let f = self.op_var(qt, Rhs::Peek(v, Peek::Field(i)), ops);
                    self.pat(q, f, qt, ops)?;
                }
            }
            PatKind::Record(name, fs) => {
                let Type::Con(_, args) = t else {
                    unreachable!()
                };
                let TypeDefKind::Record(defs) = &self.ck.types[name].kind else {
                    unreachable!()
                };
                for (f, q) in fs {
                    if matches!(q.kind, PatKind::Wild) {
                        continue;
                    }
                    let i = defs.iter().position(|(n, _)| n == f).unwrap();
                    let ft = defs[i].1.subst_params(args);
                    let fv = self.op_var(&ft, Rhs::Peek(v, Peek::Field(i)), ops);
                    self.pat(q, fv, &ft, ops)?;
                }
            }
            PatKind::Ctor(c, ps) => {
                let (tn, tag) = self.ck.ctors[c].clone();
                let Type::Con(_, args) = t else {
                    unreachable!()
                };
                let TypeDefKind::Variant(cs) = &self.ck.types[&tn].kind else {
                    unreachable!()
                };
                if cs.len() > 1 {
                    let b = self.op_var(
                        &Type::bool(),
                        Rhs::Prim(Prim::TagIs(tag as u32), vec![v]),
                        ops,
                    );
                    ops.push(Op::Check(b));
                }
                let fields: Vec<Type> = cs[tag]
                    .fields
                    .iter()
                    .map(|f| f.subst_params(args))
                    .collect();
                for (i, (q, ft)) in ps.iter().zip(&fields).enumerate() {
                    if matches!(q.kind, PatKind::Wild) {
                        continue;
                    }
                    let fv = self.op_var(ft, Rhs::Peek(v, Peek::CtorField(tag as u32, i)), ops);
                    self.pat(q, fv, ft, ops)?;
                }
            }
        }
        Ok(())
    }

    fn check_constraint(&self, c: Constraint, t: &Type, op: &str, pos: Pos) -> Result<()> {
        if let Some(false) = c.allows(t) {
            let mut nm = Namer::new();
            return Err(Error::at(
                pos,
                format!("{} (in this instance)", c.describe(op, &nm.show(t))),
            ));
        }
        Ok(())
    }

    fn vals(&mut self, es: &[Expr]) -> Result<Vec<VarId>> {
        let mut out = Vec::new();
        for e in es {
            out.push(self.val(e)?);
        }
        Ok(out)
    }

    fn val(&mut self, e: &Expr) -> Result<VarId> {
        let t = self.ty(e);
        Ok(match &e.kind {
            ExprKind::Int(n) => self.push(&t, Rhs::Int(*n)),
            ExprKind::Float(f) => self.push(&t, Rhs::Float(*f)),
            ExprKind::Str(s) => self.push(&t, Rhs::Str(s.clone())),
            ExprKind::Bool(b) => self.push(&t, Rhs::Bool(*b)),
            ExprKind::Unit => self.push(&t, Rhs::Unit),
            ExprKind::Var(n) => match self.lookup(n) {
                Some(v) => v,
                None => self.global_value(e, &t)?,
            },
            ExprKind::Qual(..) => self.global_value(e, &t)?,
            ExprKind::Call(f, args) => {
                let is_global = match &f.kind {
                    ExprKind::Var(n) => self.lookup(n).is_none(),
                    ExprKind::Qual(..) => true,
                    _ => false,
                };
                if is_global {
                    let res = self.ck.res[&f.id].clone();
                    let vs = self.vals(args)?;
                    self.apply(&res, vs, &t, e.pos)?
                } else {
                    let fv = self.val(f)?;
                    let vs = self.vals(args)?;
                    self.push(&t, Rhs::CallClo(fv, vs))
                }
            }
            ExprKind::Method(recv, name, args) => {
                let res = self.ck.res[&e.id].clone();
                if let Res::FieldCall = res {
                    let rv = self.val(recv)?;
                    let rt = self.ty(recv);
                    let Type::Con(n, targs) = &rt else {
                        unreachable!()
                    };
                    let TypeDefKind::Record(fs) = &self.ck.types[n].kind else {
                        unreachable!()
                    };
                    let i = fs.iter().position(|(f, _)| f == name).unwrap();
                    let ft = fs[i].1.subst_params(targs);
                    let fv = self.push(&ft, Rhs::Peek(rv, Peek::Field(i)));
                    let vs = self.vals(args)?;
                    return Ok(self.push(&t, Rhs::CallClo(fv, vs)));
                }
                let mut vs = vec![self.val(recv)?];
                vs.extend(self.vals(args)?);
                self.apply(&res, vs, &t, e.pos)?
            }
            ExprKind::Field(inner, name) => {
                let v = self.val(inner)?;
                let it = self.ty(inner);
                let idx = match &it {
                    Type::Tuple(_) => name.parse::<usize>().unwrap(),
                    Type::Con(n, _) => {
                        let TypeDefKind::Record(fs) = &self.ck.types[n].kind else {
                            unreachable!()
                        };
                        fs.iter().position(|(f, _)| f == name).unwrap()
                    }
                    _ => unreachable!(),
                };
                self.push(&t, Rhs::Peek(v, Peek::Field(idx)))
            }
            ExprKind::Unary(op, inner) => {
                let v = self.val(inner)?;
                match op {
                    UnOp::Neg => {
                        self.check_constraint(Constraint::Num, &t, "-", e.pos)?;
                        self.push(&t, Rhs::Prim(Prim::Neg, vec![v]))
                    }
                    UnOp::Not => self.push(&t, Rhs::Prim(Prim::Not, vec![v])),
                }
            }
            ExprKind::Binary(op, a, b) => match op {
                BinOp::And | BinOp::Or => self.block_value(e, &t)?,
                _ => {
                    let at = self.ty(a);
                    let av = self.val(a)?;
                    let bv = self.val(b)?;
                    let (c, prim) = match op {
                        BinOp::Add => (Constraint::Num, Some(Prim::Add)),
                        BinOp::Sub => (Constraint::Num, Some(Prim::Sub)),
                        BinOp::Mul => (Constraint::Num, Some(Prim::Mul)),
                        BinOp::Div => (Constraint::Num, Some(Prim::Div)),
                        BinOp::Rem => (Constraint::Num, Some(Prim::Rem)),
                        BinOp::Eq => (Constraint::Eq, Some(Prim::Eq)),
                        BinOp::Ne => (Constraint::Eq, Some(Prim::Ne)),
                        BinOp::Lt => (Constraint::Ord, Some(Prim::Lt)),
                        BinOp::Le => (Constraint::Ord, Some(Prim::Le)),
                        BinOp::Gt => (Constraint::Ord, Some(Prim::Gt)),
                        BinOp::Ge => (Constraint::Ord, Some(Prim::Ge)),
                        BinOp::Concat => (Constraint::Concat, None),
                        BinOp::And | BinOp::Or => unreachable!(),
                    };
                    self.check_constraint(c, &at, op.text(), e.pos)?;
                    match prim {
                        Some(p) => self.push(&t, Rhs::Prim(p, vec![av, bv])),
                        None => self.push(&t, Rhs::Builtin("++".into(), vec![av, bv])),
                    }
                }
            },
            ExprKind::If(..) | ExprKind::Match(..) => self.block_value(e, &t)?,
            ExprKind::Block(stmts, tail) => {
                if stmts.is_empty() {
                    match tail {
                        Some(x) => {
                            let depth = self.frame().scopes.len();
                            let r = self.val(x);
                            self.frame().scopes.truncate(depth);
                            r?
                        }
                        None => self.push(&t, Rhs::Unit),
                    }
                } else {
                    // A block's bindings end with it, so it is lowered inline.
                    let depth = self.frame().scopes.len();
                    for s in stmts {
                        if let Err(err) = self.stmt(s) {
                            self.frame().scopes.truncate(depth);
                            return Err(err);
                        }
                    }
                    let r = match tail {
                        Some(x) => self.val(x),
                        None => Ok(self.push(&t, Rhs::Unit)),
                    };
                    self.frame().scopes.truncate(depth);
                    r?
                }
            }
            ExprKind::Lambda(params, _, body) => self.lambda(e, params, body)?,
            ExprKind::Tuple(es) => {
                let vs = self.vals(es)?;
                self.push(&t, Rhs::Struct(vs))
            }
            ExprKind::Array(es) => {
                let vs = self.vals(es)?;
                if vs.is_empty() {
                    self.push(&t, Rhs::Builtin("Array.empty".into(), vec![]))
                } else {
                    self.push(&t, Rhs::Builtin("__array_lit".into(), vs))
                }
            }
            ExprKind::Record(name, fields, base) => {
                let TypeDefKind::Record(defs) = &self.ck.types[name].kind else {
                    unreachable!()
                };
                let defs = defs.clone();
                let mut given = Vec::new();
                for (f, x) in fields {
                    let v = self.val(x)?;
                    let i = defs.iter().position(|(n, _)| n == f).unwrap();
                    given.push((i, v));
                }
                match base {
                    Some(b) => {
                        let bv = self.val(b)?;
                        self.push(&t, Rhs::Update(bv, given))
                    }
                    None => {
                        given.sort_by_key(|(i, _)| *i);
                        self.push(&t, Rhs::Struct(given.into_iter().map(|(_, v)| v).collect()))
                    }
                }
            }
        })
    }

    fn block_value(&mut self, e: &Expr, t: &Type) -> Result<VarId> {
        let body = self.sub(|s| s.tail(e, Mode::Yield))?;
        Ok(self.push(t, Rhs::Block(Box::new(body))))
    }

    fn global_value(&mut self, e: &Expr, t: &Type) -> Result<VarId> {
        let res = self.ck.res[&e.id].clone();
        match &res {
            Res::Ctor(c, _) => {
                let (_, tag) = self.ck.ctors[c];
                if let Type::Fn(..) = t {
                    let key = format!("ctor {} {:?}", c, t);
                    self.wrapper(key, t, move |_, ps| Ok(Rhs::Ctor(tag as u32, ps)))
                } else {
                    Ok(self.push(t, Rhs::Ctor(tag as u32, vec![])))
                }
            }
            Res::Global(n, inst) => {
                let inst: Vec<Type> = inst.iter().map(|x| self.conc(x)).collect();
                let id = self.instance(n, inst);
                self.wrapper(format!("fn {}", id), t, move |_, ps| Ok(Rhs::Call(id, ps)))
            }
            Res::FieldCall => unreachable!(),
            Res::Builtin(n, _) => {
                let n = n.clone();
                let pos = e.pos;
                let Type::Fn(ps, _) = t else { unreachable!() };
                let pts = ps.clone();
                self.check_builtin(&n, &pts, pos)?;
                self.wrapper(format!("builtin {} {:?}", n, t), t, move |_, ps| {
                    Ok(Rhs::Builtin(n, ps))
                })
            }
        }
    }

    fn check_builtin(&self, name: &str, args: &[Type], pos: Pos) -> Result<()> {
        if name == "compare" {
            self.check_constraint(Constraint::Ord, &args[0], "compare", pos)?;
        }
        Ok(())
    }

    fn apply(&mut self, res: &Res, vs: Vec<VarId>, t: &Type, pos: Pos) -> Result<VarId> {
        Ok(match res {
            Res::Global(n, inst) => {
                let inst: Vec<Type> = inst.iter().map(|x| self.conc(x)).collect();
                let id = self.instance(n, inst);
                self.push(t, Rhs::Call(id, vs))
            }
            Res::Ctor(c, _) => {
                let (_, tag) = self.ck.ctors[c];
                self.push(t, Rhs::Ctor(tag as u32, vs))
            }
            Res::FieldCall => unreachable!(),
            Res::Builtin(n, _) => {
                let ats: Vec<Type> = vs.iter().map(|&v| self.var_type(v)).collect();
                self.check_builtin(n, &ats, pos)?;
                let v = self.push(t, Rhs::Builtin(n.clone(), vs));
                if builtins::diverges(n) {
                    // Nothing after a call that never returns runs.
                }
                v
            }
        })
    }
}
