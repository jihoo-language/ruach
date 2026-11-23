use crate::error::Pos;

#[derive(Clone, Debug)]
pub enum TypeExpr {
    /// A capitalized name with arguments: `Int`, `Array[a]`, `Option[Int]`.
    Con(String, Vec<TypeExpr>, Pos),
    /// A lowercase type variable.
    Var(String, Pos),
    Fn(Vec<TypeExpr>, Box<TypeExpr>),
    Tuple(Vec<TypeExpr>),
}

#[derive(Clone, Debug)]
pub struct Ctor {
    pub name: String,
    pub fields: Vec<TypeExpr>,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub enum TypeBody {
    Variant(Vec<Ctor>),
    Record(Vec<(String, TypeExpr)>),
}

#[derive(Clone, Debug)]
pub struct TypeDecl {
    pub name: String,
    pub params: Vec<String>,
    pub body: TypeBody,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    /// `name` or `Type.name`.
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Expr,
    pub pos: Pos,
}

impl FnDecl {
    pub fn annotated(&self) -> bool {
        self.ret.is_some() && self.params.iter().all(|p| p.ty.is_some())
    }
}

#[derive(Clone, Debug)]
pub enum Decl {
    Type(TypeDecl),
    Fn(FnDecl),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Concat => "++",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: usize,
    pub pos: Pos,
    pub kind: ExprKind,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Unit,
    /// A local, a global function, a builtin or a constructor.
    Var(String),
    /// `Type.name`.
    Qual(String, String),
    Call(Box<Expr>, Vec<Expr>),
    /// `recv.name(args)`: resolved by the type of `recv`.
    Method(Box<Expr>, String, Vec<Expr>),
    /// `e.name`, or `e.0` for tuples.
    Field(Box<Expr>, String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Match(Box<Expr>, Vec<Arm>),
    Block(Vec<Stmt>, Option<Box<Expr>>),
    Lambda(Vec<Param>, Option<TypeExpr>, Box<Expr>),
    Tuple(Vec<Expr>),
    Array(Vec<Expr>),
    /// `Point { x: 1, y: 2 }` or `Point { x: 1, ..p }`.
    Record(String, Vec<(String, Expr)>, Option<Box<Expr>>),
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let(Pat, Option<TypeExpr>, Expr),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Clone, Debug)]
pub struct Pat {
    pub pos: Pos,
    pub kind: PatKind,
}

#[derive(Clone, Debug)]
pub enum PatKind {
    Wild,
    Var(String),
    Int(i64),
    Str(String),
    Bool(bool),
    Unit,
    Ctor(String, Vec<Pat>),
    Tuple(Vec<Pat>),
    Record(String, Vec<(String, Pat)>),
}

impl Pat {
    pub fn irrefutable(&self) -> bool {
        match &self.kind {
            PatKind::Wild | PatKind::Var(_) | PatKind::Unit => true,
            PatKind::Tuple(ps) => ps.iter().all(|p| p.irrefutable()),
            PatKind::Record(_, fs) => fs.iter().all(|(_, p)| p.irrefutable()),
            _ => false,
        }
    }

    pub fn bindings(&self, out: &mut Vec<(String, Pos)>) {
        match &self.kind {
            PatKind::Var(n) => out.push((n.clone(), self.pos)),
            PatKind::Ctor(_, ps) | PatKind::Tuple(ps) => ps.iter().for_each(|p| p.bindings(out)),
            PatKind::Record(_, fs) => fs.iter().for_each(|(_, p)| p.bindings(out)),
            _ => {}
        }
    }
}

pub struct Program {
    pub decls: Vec<Decl>,
    /// The number of expression ids handed out.
    pub n_exprs: usize,
}
