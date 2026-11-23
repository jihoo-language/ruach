//! The monomorphic intermediate language. Every value is a variable;
//! every variable of a reference-counted type is either owned or borrowed
//! from the value it was read out of.

use std::collections::HashMap;

use crate::types::{Type, TypeDef, TypeDefKind};

pub type VarId = usize;
pub type TyId = usize;
pub type FnId = usize;

#[derive(Clone, Debug)]
pub enum Ir {
    Let(VarId, Rhs, Box<Ir>),
    If(VarId, Box<Ir>, Box<Ir>),
    /// Try each arm in turn; `Fail` moves on to the next arm.
    Chain(Vec<Ir>),
    Fail,
    /// The value of the innermost `Rhs::Block`.
    Yield(VarId),
    Ret(VarId),
    /// A call of the function itself in tail position: a jump.
    TailCall(Vec<VarId>),
    Inc(VarId, Box<Ir>),
    Dec(VarId, Box<Ir>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prim {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Neg,
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    TagIs(u32),
}

#[derive(Clone, Copy, Debug)]
pub enum Peek {
    Field(usize),
    CtorField(u32, usize),
}

#[derive(Clone, Debug)]
pub enum Rhs {
    Int(i64),
    Float(f64),
    Bool(bool),
    Unit,
    Str(String),
    Prim(Prim, Vec<VarId>),
    Call(FnId, Vec<VarId>),
    CallClo(VarId, Vec<VarId>),
    MakeClo(FnId, Vec<VarId>),
    Ctor(u32, Vec<VarId>),
    Struct(Vec<VarId>),
    Update(VarId, Vec<(usize, VarId)>),
    Peek(VarId, Peek),
    /// A field read that will own its value once `Release` runs.
    Own(VarId, Peek),
    /// Consumes a cell whose fields were read with `Own`: frees the cell
    /// if it is unique, and otherwise counts the new references to the fields.
    Release(VarId, Vec<VarId>),
    EnvLoad(usize),
    Builtin(String, Vec<VarId>),
    Block(Box<Ir>),
}

impl Rhs {
    /// Operands, and whether each one is consumed.
    pub fn operands(&self) -> Vec<(VarId, bool)> {
        match self {
            Rhs::Int(_)
            | Rhs::Float(_)
            | Rhs::Bool(_)
            | Rhs::Unit
            | Rhs::Str(_)
            | Rhs::EnvLoad(_)
            | Rhs::Block(_) => {
                vec![]
            }
            Rhs::Prim(_, vs) => vs.iter().map(|&v| (v, false)).collect(),
            Rhs::Call(_, vs) | Rhs::MakeClo(_, vs) | Rhs::Ctor(_, vs) | Rhs::Struct(vs) => {
                vs.iter().map(|&v| (v, true)).collect()
            }
            Rhs::CallClo(c, vs) => {
                let mut out = vec![(*c, false)];
                out.extend(vs.iter().map(|&v| (v, true)));
                out
            }
            Rhs::Update(b, fs) => {
                let mut out = vec![(*b, true)];
                out.extend(fs.iter().map(|&(_, v)| (v, true)));
                out
            }
            Rhs::Peek(v, _) | Rhs::Own(v, _) => vec![(*v, false)],
            Rhs::Release(c, fs) => {
                let mut out = vec![(*c, true)];
                out.extend(fs.iter().map(|&v| (v, false)));
                out
            }
            Rhs::Builtin(name, vs) => {
                let all = matches!(name.as_str(), "++" | "__array_lit");
                let owns = crate::builtins::owns_args(name);
                vs.iter()
                    .enumerate()
                    .map(|(i, &v)| (v, all || owns.get(i).copied().unwrap_or(false)))
                    .collect()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarKind {
    Owned,
    /// Read out of another value, which stays alive while this one is used.
    Borrowed(VarId),
    /// A closure's own environment, borrowed from the caller.
    Env,
    /// A tuple or record whose fields were moved out by a `let` pattern.
    Moved,
}

#[derive(Clone, Debug)]
pub struct VarInfo {
    pub ty: TyId,
    pub kind: VarKind,
}

#[derive(Clone, Debug)]
pub struct IrFunc {
    pub name: String,
    pub params: Vec<VarId>,
    pub ret: TyId,
    pub vars: Vec<VarInfo>,
    pub body: Ir,
    /// For closures: the environment variable and the captured types.
    pub env: Option<(VarId, Vec<TyId>)>,
}

#[derive(Clone, Debug)]
pub enum Repr {
    Int,
    Float,
    Bool,
    Unit,
    Str,
    Arr(TyId),
    Clo(Vec<TyId>, TyId),
    Tuple(Vec<TyId>),
    Record(String, Vec<(String, TyId)>),
    /// A variant type with fields: a pointer to a cell, or a tagged
    /// integer for constructors without fields.
    Boxed(String, Vec<(String, Vec<TyId>)>),
    /// A variant type whose constructors have no fields.
    Enum(String, Vec<String>),
    Pending,
}

pub struct TyTable {
    pub types: Vec<Type>,
    pub reprs: Vec<Repr>,
    map: HashMap<Type, TyId>,
}

impl TyTable {
    pub fn new() -> TyTable {
        TyTable {
            types: Vec::new(),
            reprs: Vec::new(),
            map: HashMap::new(),
        }
    }

    pub fn intern(&mut self, t: &Type, defs: &HashMap<String, TypeDef>) -> TyId {
        if let Some(&id) = self.map.get(t) {
            return id;
        }
        let id = self.types.len();
        self.types.push(t.clone());
        self.reprs.push(Repr::Pending);
        self.map.insert(t.clone(), id);
        let repr = match t {
            Type::Con(n, args) => match n.as_str() {
                "Int" => Repr::Int,
                "Float" => Repr::Float,
                "Bool" => Repr::Bool,
                "Unit" => Repr::Unit,
                "String" => Repr::Str,
                "Array" => Repr::Arr(self.intern(&args[0], defs)),
                _ => {
                    let td = &defs[n];
                    match &td.kind {
                        TypeDefKind::Record(fs) => {
                            let fs = fs
                                .iter()
                                .map(|(f, ft)| {
                                    (f.clone(), self.intern(&ft.subst_params(args), defs))
                                })
                                .collect();
                            Repr::Record(n.clone(), fs)
                        }
                        TypeDefKind::Variant(cs) => {
                            if cs.iter().all(|c| c.fields.is_empty()) {
                                Repr::Enum(n.clone(), cs.iter().map(|c| c.name.clone()).collect())
                            } else {
                                let cs = cs
                                    .iter()
                                    .map(|c| {
                                        (
                                            c.name.clone(),
                                            c.fields
                                                .iter()
                                                .map(|f| self.intern(&f.subst_params(args), defs))
                                                .collect(),
                                        )
                                    })
                                    .collect();
                                Repr::Boxed(n.clone(), cs)
                            }
                        }
                    }
                }
            },
            Type::Fn(ps, r) => {
                let ps = ps.iter().map(|p| self.intern(p, defs)).collect();
                Repr::Clo(ps, self.intern(r, defs))
            }
            Type::Tuple(ts) => Repr::Tuple(ts.iter().map(|t| self.intern(t, defs)).collect()),
            Type::Var(_) | Type::Param(_) => unreachable!("type variable in a concrete type"),
        };
        self.reprs[id] = repr;
        id
    }

    pub fn id(&self, t: &Type) -> Option<TyId> {
        self.map.get(t).copied()
    }

    /// Whether values of the type hold references that are counted.
    pub fn managed(&self, id: TyId) -> bool {
        let mut seen = Vec::new();
        self.managed_inner(id, &mut seen)
    }

    fn managed_inner(&self, id: TyId, seen: &mut Vec<TyId>) -> bool {
        match &self.reprs[id] {
            Repr::Int | Repr::Float | Repr::Bool | Repr::Unit | Repr::Enum(..) => false,
            Repr::Str | Repr::Arr(_) | Repr::Clo(..) | Repr::Boxed(..) => true,
            Repr::Tuple(ts) => {
                if seen.contains(&id) {
                    return false;
                }
                seen.push(id);
                ts.iter().any(|&t| self.managed_inner(t, seen))
            }
            Repr::Record(_, fs) => {
                if seen.contains(&id) {
                    return false;
                }
                seen.push(id);
                fs.iter().any(|&(_, t)| self.managed_inner(t, seen))
            }
            Repr::Pending => true,
        }
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }
}

impl Default for TyTable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct IrProgram {
    pub funcs: Vec<IrFunc>,
    pub tys: TyTable,
    pub main: FnId,
}
