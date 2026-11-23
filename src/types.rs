//! Hindley–Milner inference with let-polymorphism for top-level functions.
//!
//! Operators such as `+` and `<` are checked against concrete types: in a
//! generic function they are checked again for every instance the program
//! uses, as templates are.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::ast::*;
use crate::builtins::BUILTINS;
use crate::error::{Error, Pos, Result};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Var(u32),
    /// A quantified parameter, in schemes and type definitions only.
    Param(u32),
    Con(String, Vec<Type>),
    Fn(Vec<Type>, Box<Type>),
    Tuple(Vec<Type>),
}

impl Type {
    pub fn con(n: &str) -> Type {
        Type::Con(n.to_string(), vec![])
    }
    pub fn int() -> Type {
        Type::con("Int")
    }
    pub fn float() -> Type {
        Type::con("Float")
    }
    pub fn bool() -> Type {
        Type::con("Bool")
    }
    pub fn unit() -> Type {
        Type::con("Unit")
    }
    pub fn string() -> Type {
        Type::con("String")
    }
    pub fn array(t: Type) -> Type {
        Type::Con("Array".into(), vec![t])
    }

    pub fn subst_params(&self, args: &[Type]) -> Type {
        match self {
            Type::Param(i) => args[*i as usize].clone(),
            Type::Var(_) => self.clone(),
            Type::Con(n, ts) => {
                Type::Con(n.clone(), ts.iter().map(|t| t.subst_params(args)).collect())
            }
            Type::Fn(ps, r) => Type::Fn(
                ps.iter().map(|t| t.subst_params(args)).collect(),
                Box::new(r.subst_params(args)),
            ),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| t.subst_params(args)).collect()),
        }
    }

    fn vars(&self, out: &mut Vec<u32>) {
        match self {
            Type::Var(v) => {
                if !out.contains(v) {
                    out.push(*v)
                }
            }
            Type::Param(_) => {}
            Type::Con(_, ts) | Type::Tuple(ts) => ts.iter().for_each(|t| t.vars(out)),
            Type::Fn(ps, r) => {
                ps.iter().for_each(|t| t.vars(out));
                r.vars(out);
            }
        }
    }

    pub fn has_fn(&self) -> bool {
        match self {
            Type::Fn(..) => true,
            Type::Con(_, ts) | Type::Tuple(ts) => ts.iter().any(|t| t.has_fn()),
            _ => false,
        }
    }
}

/// Prints types, naming unknowns `a`, `b`, ... consistently across one message.
pub struct Namer {
    names: HashMap<u32, String>,
}

impl Namer {
    pub fn new() -> Namer {
        Namer {
            names: HashMap::new(),
        }
    }
    pub fn show(&mut self, t: &Type) -> String {
        let mut s = String::new();
        self.write(t, &mut s);
        s
    }
    fn write(&mut self, t: &Type, s: &mut String) {
        match t {
            Type::Var(v) => {
                let n = self.names.len();
                let name = self.names.entry(*v).or_insert_with(|| {
                    let c = (b'a' + (n % 26) as u8) as char;
                    if n < 26 {
                        c.to_string()
                    } else {
                        format!("{}{}", c, n / 26)
                    }
                });
                s.push_str(name);
            }
            Type::Param(i) => {
                let _ = write!(s, "p{}", i);
            }
            Type::Con(n, ts) => {
                if n == "Unit" {
                    s.push_str("()");
                    return;
                }
                s.push_str(n);
                if !ts.is_empty() {
                    s.push('[');
                    for (i, t) in ts.iter().enumerate() {
                        if i > 0 {
                            s.push_str(", ");
                        }
                        self.write(t, s);
                    }
                    s.push(']');
                }
            }
            Type::Fn(ps, r) => {
                s.push('(');
                for (i, t) in ps.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.write(t, s);
                }
                s.push_str(") -> ");
                self.write(r, s);
            }
            Type::Tuple(ts) => {
                s.push('(');
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.write(t, s);
                }
                s.push(')');
            }
        }
    }
}

impl Default for Namer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug)]
pub struct Scheme {
    pub n: u32,
    /// Uses `Param(0..n)`.
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct CtorDef {
    pub name: String,
    pub fields: Vec<Type>,
}

#[derive(Clone, Debug)]
pub enum TypeDefKind {
    Variant(Vec<CtorDef>),
    Record(Vec<(String, Type)>),
}

#[derive(Clone, Debug)]
pub struct TypeDef {
    pub name: String,
    pub params: Vec<String>,
    pub kind: TypeDefKind,
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    pub decl: usize,
    pub scheme: Scheme,
    /// The variables of the body's types that the scheme quantifies, in
    /// parameter order.
    pub quantified: Vec<u32>,
}

#[derive(Clone, Debug)]
pub enum Res {
    /// `r.f(args)` where `f` is a field of `r` that holds a function.
    FieldCall,
    Global(String, Vec<Type>),
    Builtin(String, Vec<Type>),
    Ctor(String, Vec<Type>),
}

pub struct Checked {
    pub types: HashMap<String, TypeDef>,
    pub ctors: HashMap<String, (String, usize)>,
    pub fns: HashMap<String, FnInfo>,
    pub builtins: HashMap<String, Scheme>,
    pub expr_ty: Vec<Type>,
    pub res: HashMap<usize, Res>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Constraint {
    Num,
    Ord,
    Eq,
    Concat,
}

impl Constraint {
    pub fn allows(self, t: &Type) -> Option<bool> {
        let head = match t {
            Type::Var(_) | Type::Param(_) => return None,
            Type::Con(n, _) => n.as_str(),
            Type::Fn(..) => "fn",
            Type::Tuple(_) => "tuple",
        };
        Some(match self {
            Constraint::Num => matches!(head, "Int" | "Float"),
            Constraint::Ord => matches!(head, "Int" | "Float" | "String" | "Bool"),
            Constraint::Concat => matches!(head, "String" | "Array"),
            Constraint::Eq => !t.has_fn(),
        })
    }

    pub fn describe(self, op: &str, t: &str) -> String {
        match self {
            Constraint::Num => format!("`{}` works on Int and Float, not on {}", op, t),
            Constraint::Ord => {
                format!("`{}` compares Int, Float, String and Bool, not {}", op, t)
            }
            Constraint::Concat => format!("`{}` joins strings and arrays, not {}", op, t),
            Constraint::Eq => format!(
                "values of type {} contain functions and cannot be compared",
                t
            ),
        }
    }
}

pub fn parse_sig(sig: &str) -> Scheme {
    let toks = crate::lexer::lex(sig, 0).expect("builtin signature");
    let mut id = 0;
    let mut p = crate::parser::Parser::for_types(toks, &mut id);
    let te = p.type_expr().expect("builtin signature");
    let mut names: Vec<String> = Vec::new();
    fn conv(te: &TypeExpr, names: &mut Vec<String>) -> Type {
        match te {
            TypeExpr::Con(n, args, _) => {
                Type::Con(n.clone(), args.iter().map(|a| conv(a, names)).collect())
            }
            TypeExpr::Var(n, _) => {
                let i = names.iter().position(|m| m == n).unwrap_or_else(|| {
                    names.push(n.clone());
                    names.len() - 1
                });
                Type::Param(i as u32)
            }
            TypeExpr::Fn(ps, r) => Type::Fn(
                ps.iter().map(|a| conv(a, names)).collect(),
                Box::new(conv(r, names)),
            ),
            TypeExpr::Tuple(ts) => Type::Tuple(ts.iter().map(|a| conv(a, names)).collect()),
        }
    }
    let ty = conv(&te, &mut names);
    Scheme {
        n: names.len() as u32,
        ty,
    }
}

struct Checker<'a> {
    decls: &'a [Decl],
    subst: Vec<Option<Type>>,
    types: HashMap<String, TypeDef>,
    ctors: HashMap<String, (String, usize)>,
    fns: HashMap<String, FnInfo>,
    builtins: HashMap<String, Scheme>,
    fn_index: HashMap<String, usize>,
    /// Monomorphic types of the functions of the group being inferred.
    current: HashMap<String, Type>,
    /// Uses of functions of the current group, filled in after generalizing.
    pending_self: Vec<(usize, String)>,
    expr_ty: Vec<Type>,
    res: HashMap<usize, Res>,
    locals: Vec<(String, Type)>,
    /// Type variable names in scope in the current function's annotations.
    tvars: HashMap<String, Type>,
    constraints: Vec<(Type, Constraint, Pos, &'static str)>,
    matches: Vec<(Pos, Vec<(Pat, bool)>)>,
}

pub fn check(prog: &Program) -> Result<Checked> {
    let mut c = Checker {
        decls: &prog.decls,
        subst: Vec::new(),
        types: HashMap::new(),
        ctors: HashMap::new(),
        fns: HashMap::new(),
        builtins: HashMap::new(),
        fn_index: HashMap::new(),
        current: HashMap::new(),
        pending_self: Vec::new(),
        expr_ty: vec![Type::unit(); prog.n_exprs],
        res: HashMap::new(),
        locals: Vec::new(),
        tvars: HashMap::new(),
        constraints: Vec::new(),
        matches: Vec::new(),
    };
    for (name, sig) in BUILTINS {
        c.builtins.insert(name.to_string(), parse_sig(sig));
    }
    c.declare_types()?;
    c.declare_fns()?;
    c.infer_all()?;
    // Resolve every recorded type.
    let n = c.expr_ty.len();
    for i in 0..n {
        let t = c.expr_ty[i].clone();
        c.expr_ty[i] = c.zonk(&t);
    }
    let keys: Vec<usize> = c.res.keys().copied().collect();
    for k in keys {
        let r = c.res[&k].clone();
        let r = match r {
            Res::Global(n, ts) => Res::Global(n, ts.iter().map(|t| c.zonk(t)).collect()),
            Res::Builtin(n, ts) => Res::Builtin(n, ts.iter().map(|t| c.zonk(t)).collect()),
            Res::Ctor(n, ts) => Res::Ctor(n, ts.iter().map(|t| c.zonk(t)).collect()),
            Res::FieldCall => Res::FieldCall,
        };
        c.res.insert(k, r);
    }
    match c.fns.get("main") {
        None => return Err(Error::new("the program has no `fn main()`")),
        Some(info) => {
            let ok =
                matches!(&info.scheme.ty, Type::Fn(ps, r) if ps.is_empty() && **r == Type::unit());
            if !ok {
                let pos = match &prog.decls[info.decl] {
                    Decl::Fn(f) => f.pos,
                    _ => Pos::default(),
                };
                return Err(Error::at(pos, "`main` takes no arguments and returns ()"));
            }
        }
    }
    Ok(Checked {
        types: c.types,
        ctors: c.ctors,
        fns: c.fns,
        builtins: c.builtins,
        expr_ty: c.expr_ty,
        res: c.res,
    })
}

const PRIMITIVE: &[(&str, usize)] = &[
    ("Int", 0),
    ("Float", 0),
    ("Bool", 0),
    ("String", 0),
    ("Unit", 0),
    ("Array", 1),
];

impl<'a> Checker<'a> {
    fn fresh(&mut self) -> Type {
        self.subst.push(None);
        Type::Var(self.subst.len() as u32 - 1)
    }

    fn zonk(&self, t: &Type) -> Type {
        match t {
            Type::Var(v) => match &self.subst[*v as usize] {
                Some(t) => self.zonk(t),
                None => t.clone(),
            },
            Type::Param(_) => t.clone(),
            Type::Con(n, ts) => Type::Con(n.clone(), ts.iter().map(|t| self.zonk(t)).collect()),
            Type::Fn(ps, r) => Type::Fn(
                ps.iter().map(|t| self.zonk(t)).collect(),
                Box::new(self.zonk(r)),
            ),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.zonk(t)).collect()),
        }
    }

    fn shallow(&self, t: &Type) -> Type {
        let mut t = t.clone();
        while let Type::Var(v) = t {
            match &self.subst[v as usize] {
                Some(u) => t = u.clone(),
                None => break,
            }
        }
        t
    }

    fn occurs(&self, v: u32, t: &Type) -> bool {
        match self.shallow(t) {
            Type::Var(w) => v == w,
            Type::Param(_) => false,
            Type::Con(_, ts) | Type::Tuple(ts) => ts.iter().any(|t| self.occurs(v, t)),
            Type::Fn(ps, r) => ps.iter().any(|t| self.occurs(v, t)) || self.occurs(v, &r),
        }
    }

    fn unify_inner(&mut self, a: &Type, b: &Type) -> bool {
        let a = self.shallow(a);
        let b = self.shallow(b);
        match (&a, &b) {
            (Type::Var(x), Type::Var(y)) if x == y => true,
            (Type::Var(x), _) => {
                if self.occurs(*x, &b) {
                    return false;
                }
                self.subst[*x as usize] = Some(b.clone());
                true
            }
            (_, Type::Var(_)) => self.unify_inner(&b, &a),
            (Type::Con(n, ts), Type::Con(m, us)) => {
                n == m
                    && ts.len() == us.len()
                    && ts.iter().zip(us).all(|(t, u)| self.unify_inner(t, u))
            }
            (Type::Tuple(ts), Type::Tuple(us)) => {
                ts.len() == us.len() && ts.iter().zip(us).all(|(t, u)| self.unify_inner(t, u))
            }
            (Type::Fn(ps, r), Type::Fn(qs, s)) => {
                ps.len() == qs.len()
                    && ps.iter().zip(qs).all(|(t, u)| self.unify_inner(t, u))
                    && self.unify_inner(r, s)
            }
            _ => false,
        }
    }

    /// `expected` is what the context needs; `found` is what the code has.
    fn unify(&mut self, expected: &Type, found: &Type, pos: Pos) -> Result<()> {
        let e0 = self.zonk(expected);
        let f0 = self.zonk(found);
        if self.unify_inner(expected, found) {
            return Ok(());
        }
        let mut nm = Namer::new();
        let (e, f) = (nm.show(&e0), nm.show(&f0));
        let mut msg = format!("expected {}, found {}", e, f);
        if self.occurs_any(&e0, &f0) {
            msg.push_str(" (the type would contain itself)");
        }
        Err(Error::at(pos, msg))
    }

    fn occurs_any(&self, a: &Type, b: &Type) -> bool {
        match (a, b) {
            (Type::Var(v), t) | (t, Type::Var(v)) => {
                !matches!(t, Type::Var(_)) && self.occurs(*v, t)
            }
            _ => false,
        }
    }

    fn instantiate(&mut self, s: &Scheme) -> (Type, Vec<Type>) {
        let args: Vec<Type> = (0..s.n).map(|_| self.fresh()).collect();
        (s.ty.subst_params(&args), args)
    }

    // ------------------------------------------------------------ declarations

    fn declare_types(&mut self) -> Result<()> {
        let mut arity: HashMap<String, usize> =
            PRIMITIVE.iter().map(|(n, a)| (n.to_string(), *a)).collect();
        for d in self.decls {
            if let Decl::Type(t) = d {
                if arity.contains_key(&t.name) {
                    return Err(Error::at(
                        t.pos,
                        format!("the type `{}` is already defined", t.name),
                    ));
                }
                arity.insert(t.name.clone(), t.params.len());
            }
        }
        for d in self.decls {
            if let Decl::Type(td) = d {
                let mut names = HashMap::new();
                for (i, p) in td.params.iter().enumerate() {
                    if names.insert(p.clone(), Type::Param(i as u32)).is_some() {
                        return Err(Error::at(
                            td.pos,
                            format!("the parameter `{}` appears twice", p),
                        ));
                    }
                }
                let kind = match &td.body {
                    TypeBody::Variant(cs) => {
                        let mut defs = Vec::new();
                        for (i, c) in cs.iter().enumerate() {
                            if self.ctors.contains_key(&c.name) {
                                return Err(Error::at(
                                    c.pos,
                                    format!("the constructor `{}` is already defined", c.name),
                                ));
                            }
                            self.ctors.insert(c.name.clone(), (td.name.clone(), i));
                            let mut fields = Vec::new();
                            for f in &c.fields {
                                fields.push(conv_type(f, &arity, &mut names, false)?);
                            }
                            defs.push(CtorDef {
                                name: c.name.clone(),
                                fields,
                            });
                        }
                        TypeDefKind::Variant(defs)
                    }
                    TypeBody::Record(fs) => {
                        let mut out: Vec<(String, Type)> = Vec::new();
                        for (n, f) in fs {
                            if out.iter().any(|(m, _)| m == n) {
                                return Err(Error::at(
                                    td.pos,
                                    format!("the field `{}` appears twice", n),
                                ));
                            }
                            out.push((n.clone(), conv_type(f, &arity, &mut names, false)?));
                        }
                        TypeDefKind::Record(out)
                    }
                };
                self.types.insert(
                    td.name.clone(),
                    TypeDef {
                        name: td.name.clone(),
                        params: td.params.clone(),
                        kind,
                    },
                );
            }
        }
        // A record stored inside itself without an indirection has no size.
        for d in self.decls {
            if let Decl::Type(td) = d {
                if let TypeBody::Record(_) = td.body {
                    let mut seen = HashSet::new();
                    if self.record_cycle(&td.name, &td.name, &mut seen) {
                        return Err(Error::at(
                            td.pos,
                            format!(
                                "the record `{}` contains itself; put the recursive part inside a variant type",
                                td.name
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn record_cycle(&self, target: &str, cur: &str, seen: &mut HashSet<String>) -> bool {
        if !seen.insert(cur.to_string()) {
            return false;
        }
        let Some(TypeDef {
            kind: TypeDefKind::Record(fs),
            ..
        }) = self.types.get(cur)
        else {
            return false;
        };
        fn heads(t: &Type, out: &mut Vec<String>) {
            match t {
                Type::Con(n, ts) => {
                    out.push(n.clone());
                    if n != "Array" {
                        ts.iter().for_each(|t| heads(t, out));
                    }
                }
                Type::Tuple(ts) => ts.iter().for_each(|t| heads(t, out)),
                _ => {}
            }
        }
        for (_, t) in fs {
            let mut hs = Vec::new();
            heads(t, &mut hs);
            for h in hs {
                if h == target || self.record_cycle(target, &h, seen) {
                    return true;
                }
            }
        }
        false
    }

    fn arity_map(&self) -> HashMap<String, usize> {
        let mut m: HashMap<String, usize> =
            PRIMITIVE.iter().map(|(n, a)| (n.to_string(), *a)).collect();
        for (n, t) in &self.types {
            m.insert(n.clone(), t.params.len());
        }
        m
    }

    fn declare_fns(&mut self) -> Result<()> {
        for (i, d) in self.decls.iter().enumerate() {
            if let Decl::Fn(f) = d {
                if self.fn_index.contains_key(&f.name) {
                    return Err(Error::at(
                        f.pos,
                        format!("the function `{}` is already defined", f.name),
                    ));
                }
                if self.builtins.contains_key(&f.name) || self.ctors.contains_key(&f.name) {
                    return Err(Error::at(f.pos, format!("`{}` is a builtin name", f.name)));
                }
                if let Some((ns, _)) = f.name.split_once('.') {
                    if !self.types.contains_key(ns)
                        && !PRIMITIVE.iter().any(|(n, _)| *n == ns)
                        && ns != "Fn"
                    {
                        return Err(Error::at(
                            f.pos,
                            format!("there is no type `{}` to put `{}` in", ns, f.name),
                        ));
                    }
                }
                self.fn_index.insert(f.name.clone(), i);
            }
        }
        // Fully annotated functions get their schemes now.
        let arity = self.arity_map();
        for (i, d) in self.decls.iter().enumerate() {
            if let Decl::Fn(f) = d {
                if !f.annotated() {
                    continue;
                }
                let mut names = HashMap::new();
                let mut ps = Vec::new();
                for p in &f.params {
                    ps.push(self.conv_fresh(p.ty.as_ref().unwrap(), &arity, &mut names)?);
                }
                let r = self.conv_fresh(f.ret.as_ref().unwrap(), &arity, &mut names)?;
                let ty = Type::Fn(ps, Box::new(r));
                let mut vars = Vec::new();
                ty.vars(&mut vars);
                let scheme = generalize(&ty, &vars);
                self.fns.insert(
                    f.name.clone(),
                    FnInfo {
                        decl: i,
                        scheme,
                        quantified: vars,
                    },
                );
            }
        }
        Ok(())
    }

    fn conv_fresh(
        &mut self,
        te: &TypeExpr,
        arity: &HashMap<String, usize>,
        names: &mut HashMap<String, Type>,
    ) -> Result<Type> {
        // Fresh variables for each new name.
        let mut vs = Vec::new();
        collect_tvars(te, &mut vs);
        for v in vs {
            names.entry(v).or_insert_with(|| self.fresh());
        }
        conv_type(te, arity, names, true)
    }

    // --------------------------------------------------------------- inference

    fn infer_all(&mut self) -> Result<()> {
        // Unannotated functions, grouped into strongly connected components.
        let names: Vec<String> = self
            .decls
            .iter()
            .filter_map(|d| match d {
                Decl::Fn(f) if !f.annotated() => Some(f.name.clone()),
                _ => None,
            })
            .collect();
        let index: HashMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
        for (i, n) in names.iter().enumerate() {
            let Decl::Fn(f) = &self.decls[self.fn_index[n]] else {
                unreachable!()
            };
            let mut refs = Vec::new();
            refs_of(&f.body, &mut refs);
            for r in refs {
                match r {
                    Ref::Name(r) => {
                        if let Some(&j) = index.get(r.as_str()) {
                            edges[i].push(j);
                        }
                    }
                    Ref::Method(m) => {
                        for (j, n2) in names.iter().enumerate() {
                            if n2 == &m || n2.ends_with(&format!(".{}", m)) {
                                edges[i].push(j);
                            }
                        }
                    }
                }
            }
        }
        for scc in tarjan(&edges) {
            let group: Vec<String> = scc.iter().map(|&i| names[i].clone()).collect();
            self.infer_group(&group)?;
        }
        // Annotated functions, against their declared schemes.
        let annotated: Vec<String> = self
            .decls
            .iter()
            .filter_map(|d| match d {
                Decl::Fn(f) if f.annotated() => Some(f.name.clone()),
                _ => None,
            })
            .collect();
        for n in annotated {
            self.check_annotated(&n)?;
        }
        Ok(())
    }

    fn sig_of(&mut self, f: &FnDecl) -> Result<Type> {
        let arity = self.arity_map();
        let mut names = HashMap::new();
        let mut ps = Vec::new();
        for p in &f.params {
            ps.push(match &p.ty {
                Some(t) => self.conv_fresh(t, &arity, &mut names)?,
                None => self.fresh(),
            });
        }
        let r = match &f.ret {
            Some(t) => self.conv_fresh(t, &arity, &mut names)?,
            None if f.name == "main" => Type::unit(),
            None => self.fresh(),
        };
        self.tvars = names;
        Ok(Type::Fn(ps, Box::new(r)))
    }

    fn infer_body(&mut self, f: &FnDecl, sig: &Type) -> Result<()> {
        let Type::Fn(ps, r) = sig else { unreachable!() };
        self.locals.clear();
        let mut seen = HashSet::new();
        for (p, t) in f.params.iter().zip(ps) {
            if p.name != "_" && !seen.insert(p.name.clone()) {
                return Err(Error::at(
                    p.pos,
                    format!("the parameter `{}` appears twice", p.name),
                ));
            }
            self.locals.push((p.name.clone(), t.clone()));
        }
        let bt = self.infer(&f.body)?;
        self.unify(r, &bt, body_pos(&f.body))?;
        Ok(())
    }

    fn infer_group(&mut self, group: &[String]) -> Result<()> {
        let mut sigs = Vec::new();
        let mut tvars = Vec::new();
        for n in group {
            let Decl::Fn(f) = &self.decls[self.fn_index[n]] else {
                unreachable!()
            };
            let s = self.sig_of(f)?;
            tvars.push(std::mem::take(&mut self.tvars));
            self.current.insert(n.clone(), s.clone());
            sigs.push(s);
        }
        self.constraints.clear();
        self.matches.clear();
        for (k, n) in group.iter().enumerate() {
            let Decl::Fn(f) = &self.decls[self.fn_index[n]] else {
                unreachable!()
            };
            self.tvars = std::mem::take(&mut tvars[k]);
            self.infer_body(f, &sigs[k])?;
            // Named type variables in a partial annotation stay general.
            let names = std::mem::take(&mut self.tvars);
            self.check_rigid(&names, f.pos)?;
        }
        let mut infos = Vec::new();
        for (k, n) in group.iter().enumerate() {
            let t = self.zonk(&sigs[k]);
            let mut vars = Vec::new();
            t.vars(&mut vars);
            let scheme = generalize(&t, &vars);
            infos.push((
                n.clone(),
                FnInfo {
                    decl: self.fn_index[n],
                    scheme,
                    quantified: vars,
                },
            ));
        }
        let quantified: HashSet<u32> = infos
            .iter()
            .flat_map(|(_, i)| i.quantified.iter().copied())
            .collect();
        for (n, info) in infos {
            self.fns.insert(n, info);
        }
        self.current.clear();
        for (id, callee) in std::mem::take(&mut self.pending_self) {
            let inst = self.fns[&callee]
                .quantified
                .iter()
                .map(|&v| Type::Var(v))
                .collect();
            self.res.insert(id, Res::Global(callee, inst));
        }
        self.finish_constraints(&quantified)?;
        self.check_matches()?;
        Ok(())
    }

    fn check_annotated(&mut self, name: &str) -> Result<()> {
        let info = self.fns[name].clone();
        let Decl::Fn(f) = &self.decls[info.decl] else {
            unreachable!()
        };
        let args: Vec<Type> = info.quantified.iter().map(|&v| Type::Var(v)).collect();
        let sig = info.scheme.ty.subst_params(&args);
        // Recover the names of the signature's type variables.
        let arity = self.arity_map();
        let mut names = HashMap::new();
        let mut it = info.quantified.iter();
        let mut vs = Vec::new();
        for p in &f.params {
            collect_tvars(p.ty.as_ref().unwrap(), &mut vs);
        }
        collect_tvars(f.ret.as_ref().unwrap(), &mut vs);
        for v in vs {
            names
                .entry(v)
                .or_insert_with(|| Type::Var(*it.next().unwrap()));
        }
        let _ = arity;
        self.tvars = names.clone();
        self.constraints.clear();
        self.matches.clear();
        self.infer_body(f, &sig)?;
        self.check_rigid(&names, f.pos)?;
        // A signature variable may now be represented by another variable.
        let reps: Vec<u32> = info
            .quantified
            .iter()
            .map(|&v| match self.zonk(&Type::Var(v)) {
                Type::Var(w) => w,
                _ => v,
            })
            .collect();
        self.fns.get_mut(name).unwrap().quantified = reps.clone();
        let q: HashSet<u32> = reps.iter().copied().collect();
        self.finish_constraints(&q)?;
        self.check_matches()?;
        Ok(())
    }

    fn check_rigid(&mut self, names: &HashMap<String, Type>, pos: Pos) -> Result<()> {
        let mut seen: HashMap<u32, String> = HashMap::new();
        let mut sorted: Vec<(&String, &Type)> = names.iter().collect();
        sorted.sort();
        for (n, t) in sorted {
            match self.zonk(t) {
                Type::Var(v) => {
                    if let Some(m) = seen.insert(v, n.clone()) {
                        return Err(Error::at(
                            pos,
                            format!(
                                "the type variables `{}` and `{}` must be the same type here",
                                m, n
                            ),
                        ));
                    }
                }
                other => {
                    let mut nm = Namer::new();
                    return Err(Error::at(
                        pos,
                        format!(
                            "the type variable `{}` stands for any type, but the body needs it to be {}",
                            n,
                            nm.show(&other)
                        ),
                    ));
                }
            }
        }
        Ok(())
    }

    fn finish_constraints(&mut self, quantified: &HashSet<u32>) -> Result<()> {
        for (t, c, pos, op) in std::mem::take(&mut self.constraints) {
            let mut t = self.zonk(&t);
            if let Type::Var(v) = t {
                if quantified.contains(&v) {
                    continue; // checked for each instance
                }
                if matches!(c, Constraint::Num | Constraint::Ord) {
                    // An otherwise unknown number is an Int.
                    self.subst[v as usize] = Some(Type::int());
                    t = Type::int();
                } else {
                    continue;
                }
            }
            if c == Constraint::Eq {
                // Only reject a function type that is known.
                let mut vs = Vec::new();
                t.vars(&mut vs);
                if vs.iter().any(|v| quantified.contains(v)) && !t.has_fn() {
                    continue;
                }
            }
            if let Some(false) = c.allows(&t) {
                let mut nm = Namer::new();
                return Err(Error::at(pos, c.describe(op, &nm.show(&t))));
            }
        }
        Ok(())
    }

    fn lookup_local(&self, n: &str) -> Option<Type> {
        self.locals
            .iter()
            .rev()
            .find(|(m, _)| m == n)
            .map(|(_, t)| t.clone())
    }

    /// The type of a named global, recording what the name resolves to.
    fn global(&mut self, id: usize, name: &str, pos: Pos) -> Result<Type> {
        if let Some(t) = self.current.get(name).cloned() {
            self.pending_self.push((id, name.to_string()));
            return Ok(t);
        }
        if let Some(info) = self.fns.get(name).cloned() {
            let (t, inst) = self.instantiate(&info.scheme);
            self.res.insert(id, Res::Global(name.to_string(), inst));
            return Ok(t);
        }
        if let Some(s) = self.builtins.get(name).cloned() {
            let (t, inst) = self.instantiate(&s);
            self.res.insert(id, Res::Builtin(name.to_string(), inst));
            return Ok(t);
        }
        if let Some((tn, i)) = self.ctors.get(name).cloned() {
            let td = self.types[&tn].clone();
            let args: Vec<Type> = (0..td.params.len()).map(|_| self.fresh()).collect();
            let TypeDefKind::Variant(cs) = &td.kind else {
                unreachable!()
            };
            let res_t = Type::Con(tn.clone(), args.clone());
            let fields: Vec<Type> = cs[i].fields.iter().map(|f| f.subst_params(&args)).collect();
            self.res.insert(id, Res::Ctor(name.to_string(), args));
            return Ok(if fields.is_empty() {
                res_t
            } else {
                Type::Fn(fields, Box::new(res_t))
            });
        }
        if self.fn_index.contains_key(name) {
            // An annotated function's scheme always exists, so this is unreachable.
            return Err(Error::at(
                pos,
                format!("`{}` is used before its type is known", name),
            ));
        }
        Err(Error::at(
            pos,
            unknown_name(name, self.types.contains_key(name)),
        ))
    }

    fn has_global(&self, name: &str) -> bool {
        self.current.contains_key(name)
            || self.fns.contains_key(name)
            || self.builtins.contains_key(name)
    }

    fn set(&mut self, e: &Expr, t: Type) -> Type {
        self.expr_ty[e.id] = t.clone();
        t
    }

    /// Infers `e` where a `p` is expected. A lambda learns its parameter
    /// types from `p` before its body is checked, so methods on them resolve.
    fn infer_expect(&mut self, e: &Expr, p: &Type) -> Result<()> {
        if let ExprKind::Lambda(params, ret, body) = &e.kind {
            if let Type::Fn(eps, er) = self.zonk(p) {
                if eps.len() == params.len() {
                    let depth = self.locals.len();
                    let mut ps = Vec::new();
                    for (prm, ep) in params.iter().zip(&eps) {
                        let t = match &prm.ty {
                            Some(a) => {
                                let ta = self.annotation(a)?;
                                self.unify(ep, &ta, prm.pos)?;
                                ta
                            }
                            None => ep.clone(),
                        };
                        self.locals.push((prm.name.clone(), t.clone()));
                        ps.push(t);
                    }
                    let tb = self.infer(body)?;
                    if let Some(r) = ret {
                        let tr = self.annotation(r)?;
                        self.unify(&tr, &tb, body_pos(body))?;
                    }
                    self.unify(&er, &tb, body_pos(body))?;
                    self.locals.truncate(depth);
                    self.set(e, Type::Fn(ps, Box::new(tb)));
                    return Ok(());
                }
            }
        }
        let ta = self.infer(e)?;
        self.unify(p, &ta, e.pos)
    }

    fn infer(&mut self, e: &Expr) -> Result<Type> {
        let t = self.infer_inner(e)?;
        Ok(self.set(e, t))
    }

    fn infer_inner(&mut self, e: &Expr) -> Result<Type> {
        Ok(match &e.kind {
            ExprKind::Int(_) => Type::int(),
            ExprKind::Float(_) => Type::float(),
            ExprKind::Str(_) => Type::string(),
            ExprKind::Bool(_) => Type::bool(),
            ExprKind::Unit => Type::unit(),
            ExprKind::Var(n) => match self.lookup_local(n) {
                Some(t) => t,
                None => self.global(e.id, n, e.pos)?,
            },
            ExprKind::Qual(ns, n) => {
                let full = format!("{}.{}", ns, n);
                self.global(e.id, &full, e.pos)?
            }
            ExprKind::Call(f, args) => {
                let tf = self.infer(f)?;
                self.call(e.pos, tf, args, None, &callee_name(f))?
            }
            ExprKind::Method(recv, name, args) => {
                let tr = self.infer(recv)?;
                let tr = self.zonk(&tr);
                let head = match &tr {
                    Type::Con(n, _) => Some(n.clone()),
                    Type::Fn(..) => Some("Fn".to_string()),
                    _ => None,
                };
                let qualified = head.as_ref().map(|h| format!("{}.{}", h, name));
                let has_method = matches!(&qualified, Some(q) if self.has_global(q));
                if !has_method && self.record_field(&tr, name).is_some() {
                    // A function stored in a field.
                    let tf = self.field(tr, name, e.pos)?;
                    self.res.insert(e.id, Res::FieldCall);
                    return self.call(e.pos, tf, args, None, name);
                }
                let target = match &qualified {
                    Some(q) if self.has_global(q) => q.clone(),
                    _ if self.has_global(name) && self.lookup_local(name).is_none() => name.clone(),
                    _ => {
                        let mut nm = Namer::new();
                        return Err(Error::at(
                            e.pos,
                            match head {
                                Some(_) => format!("there is no method `{}` for {}", name, nm.show(&tr)),
                                None => format!(
                                    "the type of the value before `.{}` must be known here; add a type annotation",
                                    name
                                ),
                            },
                        ));
                    }
                };
                let tf = self.global(e.id, &target, e.pos)?;
                self.call(e.pos, tf, args, Some((recv, tr)), &target)?
            }
            ExprKind::Field(inner, name) => {
                let t = self.infer(inner)?;
                self.field(t, name, e.pos)?
            }
            ExprKind::Unary(op, inner) => {
                let t = self.infer(inner)?;
                match op {
                    UnOp::Neg => {
                        self.constraints
                            .push((t.clone(), Constraint::Num, e.pos, "-"));
                        t
                    }
                    UnOp::Not => {
                        self.unify(&Type::bool(), &t, inner.pos)?;
                        Type::bool()
                    }
                }
            }
            ExprKind::Binary(op, l, r) => {
                let tl = self.infer(l)?;
                let tr = self.infer(r)?;
                let name = op.text();
                match op {
                    BinOp::And | BinOp::Or => {
                        self.unify(&Type::bool(), &tl, l.pos)?;
                        self.unify(&Type::bool(), &tr, r.pos)?;
                        Type::bool()
                    }
                    BinOp::Rem => {
                        self.unify(&Type::int(), &tl, l.pos)?;
                        self.unify(&Type::int(), &tr, r.pos)?;
                        Type::int()
                    }
                    _ => {
                        self.unify(&tl, &tr, r.pos)?;
                        let (c, res) = match op {
                            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                                (Constraint::Num, tl.clone())
                            }
                            BinOp::Concat => (Constraint::Concat, tl.clone()),
                            BinOp::Eq | BinOp::Ne => (Constraint::Eq, Type::bool()),
                            _ => (Constraint::Ord, Type::bool()),
                        };
                        self.constraints.push((tl, c, e.pos, name));
                        res
                    }
                }
            }
            ExprKind::If(c, t, f) => {
                let tc = self.infer(c)?;
                self.unify(&Type::bool(), &tc, c.pos)?;
                let tt = self.infer(t)?;
                match f {
                    Some(f) => {
                        let tf = self.infer(f)?;
                        self.unify(&tt, &tf, body_pos(f))?;
                        tt
                    }
                    None => {
                        self.unify(&Type::unit(), &tt, body_pos(t))
                            .map_err(|mut err| {
                                err.msg.push_str("; an `if` without `else` gives ()");
                                err
                            })?;
                        Type::unit()
                    }
                }
            }
            ExprKind::Match(s, arms) => {
                let ts = self.infer(s)?;
                let res = self.fresh();
                for arm in arms {
                    let depth = self.locals.len();
                    self.bind_pat(&arm.pat, &ts)?;
                    if let Some(g) = &arm.guard {
                        let tg = self.infer(g)?;
                        self.unify(&Type::bool(), &tg, g.pos)?;
                    }
                    let tb = self.infer(&arm.body)?;
                    self.unify(&res, &tb, body_pos(&arm.body))?;
                    self.locals.truncate(depth);
                }
                self.matches.push((
                    e.pos,
                    arms.iter()
                        .map(|a| (a.pat.clone(), a.guard.is_some()))
                        .collect(),
                ));
                res
            }
            ExprKind::Block(stmts, tail) => {
                let depth = self.locals.len();
                for s in stmts {
                    match s {
                        Stmt::Let(p, ann, v) => {
                            let tv = self.infer(v)?;
                            if let Some(a) = ann {
                                let ta = self.annotation(a)?;
                                self.unify(&ta, &tv, v.pos)?;
                            }
                            self.bind_pat(p, &tv)?;
                        }
                        Stmt::Expr(x) => {
                            self.infer(x)?;
                        }
                    }
                }
                let t = match tail {
                    Some(t) => self.infer(t)?,
                    None => Type::unit(),
                };
                self.locals.truncate(depth);
                t
            }
            ExprKind::Lambda(params, ret, body) => {
                let depth = self.locals.len();
                let mut ps = Vec::new();
                for p in params {
                    let t = match &p.ty {
                        Some(a) => self.annotation(a)?,
                        None => self.fresh(),
                    };
                    self.locals.push((p.name.clone(), t.clone()));
                    ps.push(t);
                }
                let tb = self.infer(body)?;
                if let Some(r) = ret {
                    let tr = self.annotation(r)?;
                    self.unify(&tr, &tb, body_pos(body))?;
                }
                self.locals.truncate(depth);
                Type::Fn(ps, Box::new(tb))
            }
            ExprKind::Tuple(es) => {
                let mut ts = Vec::new();
                for x in es {
                    ts.push(self.infer(x)?);
                }
                Type::Tuple(ts)
            }
            ExprKind::Array(es) => {
                let t = self.fresh();
                for x in es {
                    let tx = self.infer(x)?;
                    self.unify(&t, &tx, x.pos)?;
                }
                Type::array(t)
            }
            ExprKind::Record(name, fields, base) => {
                let Some(td) = self.types.get(name).cloned() else {
                    return Err(Error::at(
                        e.pos,
                        format!("there is no record type `{}`", name),
                    ));
                };
                let TypeDefKind::Record(defs) = &td.kind else {
                    return Err(Error::at(
                        e.pos,
                        format!("`{}` is a variant type, not a record", name),
                    ));
                };
                let args: Vec<Type> = (0..td.params.len()).map(|_| self.fresh()).collect();
                let rt = Type::Con(name.clone(), args.clone());
                let mut given = HashSet::new();
                for (f, v) in fields {
                    let Some((_, ft)) = defs.iter().find(|(n, _)| n == f) else {
                        return Err(Error::at(v.pos, format!("`{}` has no field `{}`", name, f)));
                    };
                    if !given.insert(f.clone()) {
                        return Err(Error::at(
                            v.pos,
                            format!("the field `{}` is given twice", f),
                        ));
                    }
                    let ft = ft.subst_params(&args);
                    let tv = self.infer(v)?;
                    self.unify(&ft, &tv, v.pos)?;
                }
                match base {
                    Some(b) => {
                        let tb = self.infer(b)?;
                        self.unify(&rt, &tb, b.pos)?;
                    }
                    None => {
                        let missing: Vec<&str> = defs
                            .iter()
                            .filter(|(n, _)| !given.contains(n))
                            .map(|(n, _)| n.as_str())
                            .collect();
                        if !missing.is_empty() {
                            return Err(Error::at(
                                e.pos,
                                format!(
                                    "missing field{} {}",
                                    if missing.len() > 1 { "s" } else { "" },
                                    missing.join(", ")
                                ),
                            ));
                        }
                    }
                }
                rt
            }
        })
    }

    fn call(
        &mut self,
        pos: Pos,
        tf: Type,
        args: &[Expr],
        recv: Option<(&Expr, Type)>,
        name: &str,
    ) -> Result<Type> {
        let tf = self.zonk(&tf);
        let n = args.len() + recv.is_some() as usize;
        let (ps, r) = match tf {
            Type::Fn(ps, r) => (ps, *r),
            Type::Var(_) => {
                let ps: Vec<Type> = (0..n).map(|_| self.fresh()).collect();
                let r = self.fresh();
                self.unify(&tf, &Type::Fn(ps.clone(), Box::new(r.clone())), pos)?;
                (ps, r)
            }
            other => {
                let mut nm = Namer::new();
                return Err(Error::at(
                    pos,
                    format!(
                        "{} is not a function; it has type {}",
                        what(name),
                        nm.show(&other)
                    ),
                ));
            }
        };
        if ps.len() != n {
            return Err(Error::at(
                pos,
                format!(
                    "{} takes {} argument{}, but {} {} given",
                    what(name),
                    ps.len(),
                    if ps.len() == 1 { "" } else { "s" },
                    n,
                    if n == 1 { "was" } else { "were" }
                ),
            ));
        }
        let mut k = 0;
        if let Some((re, rt)) = recv {
            self.unify(&ps[0], &rt, re.pos)?;
            k = 1;
        }
        for (a, p) in args.iter().zip(&ps[k..]) {
            self.infer_expect(a, p)?;
        }
        Ok(r)
    }

    fn record_field(&self, t: &Type, name: &str) -> Option<Type> {
        let Type::Con(n, args) = t else { return None };
        let Some(TypeDef {
            kind: TypeDefKind::Record(fs),
            ..
        }) = self.types.get(n)
        else {
            return None;
        };
        fs.iter()
            .find(|(f, _)| f == name)
            .map(|(_, ft)| ft.subst_params(args))
    }

    fn field(&mut self, t: Type, name: &str, pos: Pos) -> Result<Type> {
        let t = self.zonk(&t);
        if let Ok(i) = name.parse::<usize>() {
            return match &t {
                Type::Tuple(ts) if i < ts.len() => Ok(ts[i].clone()),
                Type::Tuple(ts) => Err(Error::at(
                    pos,
                    format!("this tuple has {} fields", ts.len()),
                )),
                Type::Var(_) => Err(Error::at(
                    pos,
                    "the tuple type must be known here; add a type annotation",
                )),
                other => {
                    let mut nm = Namer::new();
                    Err(Error::at(pos, format!("{} is not a tuple", nm.show(other))))
                }
            };
        }
        match &t {
            Type::Con(n, args) => {
                if let Some(TypeDef {
                    kind: TypeDefKind::Record(fs),
                    ..
                }) = self.types.get(n)
                {
                    if let Some((_, ft)) = fs.iter().find(|(f, _)| f == name) {
                        return Ok(ft.subst_params(args));
                    }
                }
                let mut nm = Namer::new();
                Err(Error::at(
                    pos,
                    format!("{} has no field `{}`", nm.show(&t), name),
                ))
            }
            Type::Var(_) => {
                let owners: Vec<TypeDef> = self
                    .types
                    .values()
                    .filter(|td| matches!(&td.kind, TypeDefKind::Record(fs) if fs.iter().any(|(f, _)| f == name)))
                    .cloned()
                    .collect();
                match owners.len() {
                    0 => Err(Error::at(pos, format!("no record has a field `{}`", name))),
                    1 => {
                        let td = &owners[0];
                        let args: Vec<Type> = (0..td.params.len()).map(|_| self.fresh()).collect();
                        self.unify(&Type::Con(td.name.clone(), args.clone()), &t, pos)?;
                        let TypeDefKind::Record(fs) = &td.kind else {
                            unreachable!()
                        };
                        Ok(fs
                            .iter()
                            .find(|(f, _)| f == name)
                            .unwrap()
                            .1
                            .subst_params(&args))
                    }
                    _ => Err(Error::at(
                        pos,
                        format!(
                            "several records have a field `{}`; annotate the type of this value",
                            name
                        ),
                    )),
                }
            }
            other => {
                let mut nm = Namer::new();
                Err(Error::at(
                    pos,
                    format!("{} has no field `{}`", nm.show(other), name),
                ))
            }
        }
    }

    fn annotation(&mut self, te: &TypeExpr) -> Result<Type> {
        let arity = self.arity_map();
        let mut names = std::mem::take(&mut self.tvars);
        let r = self.conv_fresh(te, &arity, &mut names);
        self.tvars = names;
        r
    }

    fn bind_pat(&mut self, p: &Pat, t: &Type) -> Result<()> {
        let mut names = Vec::new();
        p.bindings(&mut names);
        let mut seen = HashSet::new();
        for (n, pos) in &names {
            if !seen.insert(n.clone()) {
                return Err(Error::at(
                    *pos,
                    format!("`{}` is bound twice in this pattern", n),
                ));
            }
        }
        self.pat(p, t)
    }

    fn pat(&mut self, p: &Pat, t: &Type) -> Result<()> {
        match &p.kind {
            PatKind::Wild => Ok(()),
            PatKind::Var(n) => {
                self.locals.push((n.clone(), t.clone()));
                Ok(())
            }
            PatKind::Int(_) => self.unify(t, &Type::int(), p.pos),
            PatKind::Str(_) => self.unify(t, &Type::string(), p.pos),
            PatKind::Bool(_) => self.unify(t, &Type::bool(), p.pos),
            PatKind::Unit => self.unify(t, &Type::unit(), p.pos),
            PatKind::Tuple(ps) => {
                let ts: Vec<Type> = ps.iter().map(|_| self.fresh()).collect();
                self.unify(t, &Type::Tuple(ts.clone()), p.pos)?;
                for (q, u) in ps.iter().zip(&ts) {
                    self.pat(q, u)?;
                }
                Ok(())
            }
            PatKind::Ctor(c, ps) => {
                let Some((tn, i)) = self.ctors.get(c).cloned() else {
                    return Err(Error::at(p.pos, format!("there is no constructor `{}`", c)));
                };
                let td = self.types[&tn].clone();
                let args: Vec<Type> = (0..td.params.len()).map(|_| self.fresh()).collect();
                self.unify(t, &Type::Con(tn.clone(), args.clone()), p.pos)?;
                let TypeDefKind::Variant(cs) = &td.kind else {
                    unreachable!()
                };
                let fields = &cs[i].fields;
                if fields.len() != ps.len() {
                    return Err(Error::at(
                        p.pos,
                        format!(
                            "`{}` has {} field{}, but the pattern has {}",
                            c,
                            fields.len(),
                            if fields.len() == 1 { "" } else { "s" },
                            ps.len()
                        ),
                    ));
                }
                for (q, f) in ps.iter().zip(fields) {
                    let ft = f.subst_params(&args);
                    self.pat(q, &ft)?;
                }
                Ok(())
            }
            PatKind::Record(name, fs) => {
                let Some(td) = self.types.get(name).cloned() else {
                    return Err(Error::at(
                        p.pos,
                        format!("there is no record type `{}`", name),
                    ));
                };
                let TypeDefKind::Record(defs) = &td.kind else {
                    return Err(Error::at(p.pos, format!("`{}` is not a record", name)));
                };
                let args: Vec<Type> = (0..td.params.len()).map(|_| self.fresh()).collect();
                self.unify(t, &Type::Con(name.clone(), args.clone()), p.pos)?;
                for (f, q) in fs {
                    let Some((_, ft)) = defs.iter().find(|(n, _)| n == f) else {
                        return Err(Error::at(q.pos, format!("`{}` has no field `{}`", name, f)));
                    };
                    let ft = ft.subst_params(&args);
                    self.pat(q, &ft)?;
                }
                Ok(())
            }
        }
    }

    // --------------------------------------------------------- exhaustiveness

    fn check_matches(&mut self) -> Result<()> {
        for (pos, arms) in std::mem::take(&mut self.matches) {
            let rows: Vec<Vec<Pat>> = arms
                .iter()
                .filter(|(_, g)| !g)
                .map(|(p, _)| vec![p.clone()])
                .collect();
            // Report arms that can never match.
            for (i, (p, _)) in arms.iter().enumerate() {
                let before: Vec<Vec<Pat>> = arms[..i]
                    .iter()
                    .filter(|(_, g)| !g)
                    .map(|(p, _)| vec![p.clone()])
                    .collect();
                if !self.useful(&before, std::slice::from_ref(p)) {
                    return Err(Error::at(
                        p.pos,
                        "this arm can never match: earlier arms cover it",
                    ));
                }
            }
            let wild = Pat {
                pos,
                kind: PatKind::Wild,
            };
            if let Some(w) = self.witness(&rows, 1) {
                let _ = wild;
                return Err(Error::at(
                    pos,
                    format!("this `match` does not handle {}", w[0]),
                ));
            }
        }
        Ok(())
    }

    /// The constructors of the column, if the column's patterns name a
    /// finite set: (name, arity) pairs.
    fn signature(&self, col: &[&Pat]) -> Option<Vec<(Head, usize)>> {
        for p in col {
            match &p.kind {
                PatKind::Ctor(c, _) => {
                    let (tn, _) = &self.ctors[c];
                    let TypeDefKind::Variant(cs) = &self.types[tn].kind else {
                        unreachable!()
                    };
                    return Some(
                        cs.iter()
                            .map(|c| (Head::Ctor(c.name.clone()), c.fields.len()))
                            .collect(),
                    );
                }
                PatKind::Bool(_) => {
                    return Some(vec![(Head::Bool(false), 0), (Head::Bool(true), 0)])
                }
                PatKind::Unit => return Some(vec![(Head::Unit, 0)]),
                PatKind::Tuple(ps) => return Some(vec![(Head::Tuple, ps.len())]),
                PatKind::Record(n, _) => {
                    let TypeDefKind::Record(fs) = &self.types[n].kind else {
                        unreachable!()
                    };
                    return Some(vec![(Head::Record(n.clone()), fs.len())]);
                }
                _ => {}
            }
        }
        None
    }

    fn specialize(&self, p: &Pat, h: &Head, arity: usize) -> Option<Vec<Pat>> {
        let wild = |pos| Pat {
            pos,
            kind: PatKind::Wild,
        };
        match (&p.kind, h) {
            (PatKind::Wild | PatKind::Var(_), _) => Some((0..arity).map(|_| wild(p.pos)).collect()),
            (PatKind::Ctor(c, ps), Head::Ctor(d)) => (c == d).then(|| ps.clone()),
            (PatKind::Bool(b), Head::Bool(c)) => (b == c).then(Vec::new),
            (PatKind::Unit, Head::Unit) => Some(vec![]),
            (PatKind::Tuple(ps), Head::Tuple) => Some(ps.clone()),
            (PatKind::Record(n, fs), Head::Record(_)) => {
                let TypeDefKind::Record(defs) = &self.types[n].kind else {
                    unreachable!()
                };
                Some(
                    defs.iter()
                        .map(|(f, _)| {
                            fs.iter()
                                .find(|(g, _)| g == f)
                                .map(|(_, q)| q.clone())
                                .unwrap_or(wild(p.pos))
                        })
                        .collect(),
                )
            }
            (PatKind::Int(a), Head::Int(b)) => (a == b).then(Vec::new),
            (PatKind::Str(a), Head::Str(b)) => (a == b).then(Vec::new),
            _ => None,
        }
    }

    fn useful(&self, rows: &[Vec<Pat>], v: &[Pat]) -> bool {
        if v.is_empty() {
            return rows.is_empty();
        }
        let head = &v[0];
        match &head.kind {
            PatKind::Wild | PatKind::Var(_) => {
                let col: Vec<&Pat> = rows.iter().map(|r| &r[0]).collect();
                match self.signature(&col) {
                    Some(sig) => sig.iter().any(|(h, a)| {
                        let rs = self.spec_rows(rows, h, *a);
                        let mut nv = self.specialize(head, h, *a).unwrap();
                        nv.extend_from_slice(&v[1..]);
                        self.useful(&rs, &nv)
                    }),
                    None => {
                        let rs: Vec<Vec<Pat>> = rows
                            .iter()
                            .filter(|r| matches!(r[0].kind, PatKind::Wild | PatKind::Var(_)))
                            .map(|r| r[1..].to_vec())
                            .collect();
                        self.useful(&rs, &v[1..])
                    }
                }
            }
            _ => {
                let (h, a) = head_of(self, head);
                let rs = self.spec_rows(rows, &h, a);
                let mut nv = self.specialize(head, &h, a).unwrap();
                nv.extend_from_slice(&v[1..]);
                self.useful(&rs, &nv)
            }
        }
    }

    fn spec_rows(&self, rows: &[Vec<Pat>], h: &Head, a: usize) -> Vec<Vec<Pat>> {
        rows.iter()
            .filter_map(|r| {
                self.specialize(&r[0], h, a).map(|mut s| {
                    s.extend_from_slice(&r[1..]);
                    s
                })
            })
            .collect()
    }

    /// Values (as text) of `n` columns that no row matches.
    fn witness(&self, rows: &[Vec<Pat>], n: usize) -> Option<Vec<String>> {
        if n == 0 {
            return if rows.is_empty() { Some(vec![]) } else { None };
        }
        let col: Vec<&Pat> = rows.iter().map(|r| &r[0]).collect();
        match self.signature(&col) {
            Some(sig) => {
                for (h, a) in &sig {
                    let rs = self.spec_rows(rows, h, *a);
                    if let Some(mut w) = self.witness(&rs, a + n - 1) {
                        let fields: Vec<String> = w.drain(..*a).collect();
                        let text = match h {
                            Head::Ctor(c) if fields.is_empty() => c.clone(),
                            Head::Ctor(c) => format!("{}({})", c, fields.join(", ")),
                            Head::Bool(b) => b.to_string(),
                            Head::Unit => "()".into(),
                            Head::Tuple => format!("({})", fields.join(", ")),
                            Head::Record(r) => {
                                let TypeDefKind::Record(defs) = &self.types[r].kind else {
                                    unreachable!()
                                };
                                let parts: Vec<String> = defs
                                    .iter()
                                    .zip(&fields)
                                    .filter(|(_, v)| v.as_str() != "_")
                                    .map(|((f, _), v)| format!("{}: {}", f, v))
                                    .collect();
                                if parts.is_empty() {
                                    format!("{} {{ .. }}", r)
                                } else {
                                    format!("{} {{ {}, .. }}", r, parts.join(", "))
                                }
                            }
                            _ => "_".into(),
                        };
                        w.insert(0, text);
                        return Some(w);
                    }
                }
                None
            }
            None => {
                let rs: Vec<Vec<Pat>> = rows
                    .iter()
                    .filter(|r| matches!(r[0].kind, PatKind::Wild | PatKind::Var(_)))
                    .map(|r| r[1..].to_vec())
                    .collect();
                self.witness(&rs, n - 1).map(|mut w| {
                    w.insert(0, "_".into());
                    w
                })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Head {
    Ctor(String),
    Bool(bool),
    Unit,
    Tuple,
    Record(String),
    Int(i64),
    Str(String),
}

fn head_of(c: &Checker, p: &Pat) -> (Head, usize) {
    match &p.kind {
        PatKind::Ctor(n, ps) => (Head::Ctor(n.clone()), ps.len()),
        PatKind::Bool(b) => (Head::Bool(*b), 0),
        PatKind::Unit => (Head::Unit, 0),
        PatKind::Tuple(ps) => (Head::Tuple, ps.len()),
        PatKind::Record(n, _) => {
            let TypeDefKind::Record(fs) = &c.types[n].kind else {
                unreachable!()
            };
            (Head::Record(n.clone()), fs.len())
        }
        PatKind::Int(i) => (Head::Int(*i), 0),
        PatKind::Str(s) => (Head::Str(s.clone()), 0),
        PatKind::Wild | PatKind::Var(_) => unreachable!(),
    }
}

fn generalize(t: &Type, vars: &[u32]) -> Scheme {
    fn go(t: &Type, vars: &[u32]) -> Type {
        match t {
            Type::Var(v) => match vars.iter().position(|w| w == v) {
                Some(i) => Type::Param(i as u32),
                None => t.clone(),
            },
            Type::Param(_) => t.clone(),
            Type::Con(n, ts) => Type::Con(n.clone(), ts.iter().map(|t| go(t, vars)).collect()),
            Type::Fn(ps, r) => Type::Fn(
                ps.iter().map(|t| go(t, vars)).collect(),
                Box::new(go(r, vars)),
            ),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| go(t, vars)).collect()),
        }
    }
    Scheme {
        n: vars.len() as u32,
        ty: go(t, vars),
    }
}

fn collect_tvars(te: &TypeExpr, out: &mut Vec<String>) {
    match te {
        TypeExpr::Var(n, _) => {
            if !out.contains(n) {
                out.push(n.clone())
            }
        }
        TypeExpr::Con(_, ts, _) | TypeExpr::Tuple(ts) => {
            ts.iter().for_each(|t| collect_tvars(t, out))
        }
        TypeExpr::Fn(ps, r) => {
            ps.iter().for_each(|t| collect_tvars(t, out));
            collect_tvars(r, out);
        }
    }
}

/// Converts a written type. In type definitions (`fresh_ok == false`) every
/// variable must be a declared parameter.
fn conv_type(
    te: &TypeExpr,
    arity: &HashMap<String, usize>,
    names: &mut HashMap<String, Type>,
    fresh_ok: bool,
) -> Result<Type> {
    Ok(match te {
        TypeExpr::Con(n, args, pos) => {
            let Some(&a) = arity.get(n) else {
                return Err(Error::at(*pos, format!("there is no type `{}`", n)));
            };
            if a != args.len() {
                return Err(Error::at(
                    *pos,
                    format!(
                        "`{}` takes {} type argument{}, but {} {} given",
                        n,
                        a,
                        if a == 1 { "" } else { "s" },
                        args.len(),
                        if args.len() == 1 { "was" } else { "were" }
                    ),
                ));
            }
            let mut ts = Vec::new();
            for x in args {
                ts.push(conv_type(x, arity, names, fresh_ok)?);
            }
            Type::Con(n.clone(), ts)
        }
        TypeExpr::Var(n, pos) => match names.get(n) {
            Some(t) => t.clone(),
            None => {
                let _ = fresh_ok;
                return Err(Error::at(
                    *pos,
                    format!("the type variable `{}` is not a parameter of this type", n),
                ));
            }
        },
        TypeExpr::Fn(ps, r) => {
            let mut ts = Vec::new();
            for x in ps {
                ts.push(conv_type(x, arity, names, fresh_ok)?);
            }
            Type::Fn(ts, Box::new(conv_type(r, arity, names, fresh_ok)?))
        }
        TypeExpr::Tuple(ts) => {
            let mut out = Vec::new();
            for x in ts {
                out.push(conv_type(x, arity, names, fresh_ok)?);
            }
            Type::Tuple(out)
        }
    })
}

enum Ref {
    Name(String),
    Method(String),
}

fn refs_of(e: &Expr, out: &mut Vec<Ref>) {
    match &e.kind {
        ExprKind::Var(n) => out.push(Ref::Name(n.clone())),
        ExprKind::Qual(a, b) => out.push(Ref::Name(format!("{}.{}", a, b))),
        ExprKind::Call(f, args) => {
            refs_of(f, out);
            args.iter().for_each(|a| refs_of(a, out));
        }
        ExprKind::Method(r, m, args) => {
            out.push(Ref::Method(m.clone()));
            refs_of(r, out);
            args.iter().for_each(|a| refs_of(a, out));
        }
        ExprKind::Field(x, _) | ExprKind::Unary(_, x) => refs_of(x, out),
        ExprKind::Binary(_, a, b) => {
            refs_of(a, out);
            refs_of(b, out);
        }
        ExprKind::If(c, t, f) => {
            refs_of(c, out);
            refs_of(t, out);
            if let Some(f) = f {
                refs_of(f, out);
            }
        }
        ExprKind::Match(s, arms) => {
            refs_of(s, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    refs_of(g, out);
                }
                refs_of(&a.body, out);
            }
        }
        ExprKind::Block(ss, t) => {
            for s in ss {
                match s {
                    Stmt::Let(_, _, x) | Stmt::Expr(x) => refs_of(x, out),
                }
            }
            if let Some(t) = t {
                refs_of(t, out);
            }
        }
        ExprKind::Lambda(_, _, b) => refs_of(b, out),
        ExprKind::Tuple(es) | ExprKind::Array(es) => es.iter().for_each(|x| refs_of(x, out)),
        ExprKind::Record(_, fs, b) => {
            fs.iter().for_each(|(_, x)| refs_of(x, out));
            if let Some(b) = b {
                refs_of(b, out);
            }
        }
        _ => {}
    }
}

/// Strongly connected components, callees before callers.
fn tarjan(edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct S<'a> {
        edges: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Vec<usize>>,
    }
    fn visit(s: &mut S, v: usize) {
        s.index[v] = Some(s.next);
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on[v] = true;
        for k in 0..s.edges[v].len() {
            let w = s.edges[v][k];
            match s.index[w] {
                None => {
                    visit(s, w);
                    s.low[v] = s.low[v].min(s.low[w]);
                }
                Some(iw) if s.on[w] => s.low[v] = s.low[v].min(iw),
                _ => {}
            }
        }
        if Some(s.low[v]) == s.index[v] {
            let mut comp = Vec::new();
            loop {
                let w = s.stack.pop().unwrap();
                s.on[w] = false;
                comp.push(w);
                if w == v {
                    break;
                }
            }
            comp.sort();
            s.out.push(comp);
        }
    }
    let n = edges.len();
    let mut s = S {
        edges,
        index: vec![None; n],
        low: vec![0; n],
        on: vec![false; n],
        stack: vec![],
        next: 0,
        out: vec![],
    };
    for v in 0..n {
        if s.index[v].is_none() {
            visit(&mut s, v);
        }
    }
    s.out
}

fn body_pos(e: &Expr) -> Pos {
    match &e.kind {
        ExprKind::Block(_, Some(t)) => body_pos(t),
        _ => e.pos,
    }
}

fn callee_name(f: &Expr) -> String {
    match &f.kind {
        ExprKind::Var(n) => n.clone(),
        ExprKind::Qual(a, b) => format!("{}.{}", a, b),
        _ => String::new(),
    }
}

fn what(name: &str) -> String {
    if name.is_empty() {
        "this function".to_string()
    } else {
        format!("`{}`", name)
    }
}

fn unknown_name(name: &str, is_type: bool) -> String {
    if is_type {
        return format!(
            "`{}` is a type; a record is built with `{} {{ field: value }}`",
            name, name
        );
    }
    if name == "this" || name == "self" {
        return format!(
            "there is no `{}`; methods take their receiver as the first parameter",
            name
        );
    }
    format!("`{}` is not defined", name)
}
