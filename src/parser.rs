use crate::ast::*;
use crate::error::{Error, Pos, Result};
use crate::lexer::{Tok, Token};

pub struct Parsed {
    pub decls: Vec<Decl>,
    pub imports: Vec<(String, Pos)>,
}

pub struct Parser<'a> {
    toks: Vec<Token>,
    i: usize,
    next_id: &'a mut usize,
    /// Inside an `if` condition or `match` scrutinee, `Name {` starts the
    /// block, not a record literal.
    no_struct: bool,
}

pub fn parse(toks: Vec<Token>, next_id: &mut usize) -> Result<Parsed> {
    let mut p = Parser {
        toks,
        i: 0,
        next_id,
        no_struct: false,
    };
    let mut decls = Vec::new();
    let mut imports = Vec::new();
    loop {
        let t = p.peek().clone();
        match t {
            Tok::Eof => break,
            Tok::Import => {
                p.bump();
                let pos = p.pos();
                match p.bump() {
                    Tok::Str(s) => imports.push((s, pos)),
                    _ => return Err(Error::at(pos, "expected a file name after `import`")),
                }
            }
            Tok::Type => decls.push(Decl::Type(p.type_decl()?)),
            Tok::Fn => decls.push(Decl::Fn(p.fn_decl()?)),
            Tok::Semi => {
                p.bump();
            }
            _ => {
                return Err(Error::at(
                    p.pos(),
                    format!("expected `fn`, `type` or `import`, found {}", t.describe()),
                ))
            }
        }
    }
    Ok(Parsed { decls, imports })
}

impl<'a> Parser<'a> {
    pub fn for_types(toks: Vec<Token>, next_id: &'a mut usize) -> Parser<'a> {
        Parser {
            toks,
            i: 0,
            next_id,
            no_struct: false,
        }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }
    fn peek_at(&self, k: usize) -> &Tok {
        &self.toks[(self.i + k).min(self.toks.len() - 1)].tok
    }
    fn nl(&self) -> bool {
        self.toks[self.i].nl
    }
    fn pos(&self) -> Pos {
        self.toks[self.i].pos
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == t {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, t: &Tok) -> Result<()> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(Error::at(
                self.pos(),
                format!(
                    "expected {}, found {}",
                    t.describe(),
                    self.peek().describe()
                ),
            ))
        }
    }
    fn ident(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            t => Err(Error::at(
                self.pos(),
                format!("expected a name, found {}", t.describe()),
            )),
        }
    }
    fn uident(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::UIdent(s) => {
                self.bump();
                Ok(s)
            }
            t => Err(Error::at(
                self.pos(),
                format!("expected a capitalized name, found {}", t.describe()),
            )),
        }
    }
    fn mk(&mut self, pos: Pos, kind: ExprKind) -> Expr {
        let id = *self.next_id;
        *self.next_id += 1;
        Expr { id, pos, kind }
    }

    // ---------------------------------------------------------------- decls

    fn type_decl(&mut self) -> Result<TypeDecl> {
        let pos = self.pos();
        self.expect(&Tok::Type)?;
        let name = self.uident()?;
        let mut params = Vec::new();
        if self.eat(&Tok::LBracket) {
            loop {
                params.push(self.ident()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RBracket)?;
        }
        self.expect(&Tok::Eq)?;
        let body = if self.eat(&Tok::LBrace) {
            let mut fields = Vec::new();
            while !self.eat(&Tok::RBrace) {
                let f = self.ident()?;
                self.expect(&Tok::Colon)?;
                let t = self.type_expr()?;
                fields.push((f, t));
                self.eat(&Tok::Comma);
            }
            TypeBody::Record(fields)
        } else {
            self.eat(&Tok::Bar);
            let mut ctors = Vec::new();
            loop {
                let cpos = self.pos();
                let cname = self.uident()?;
                let mut fields = Vec::new();
                if self.peek() == &Tok::LParen && !self.nl() {
                    self.bump();
                    if !self.eat(&Tok::RParen) {
                        loop {
                            fields.push(self.type_expr()?);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                        self.expect(&Tok::RParen)?;
                    }
                }
                ctors.push(Ctor {
                    name: cname,
                    fields,
                    pos: cpos,
                });
                if !self.eat(&Tok::Bar) {
                    break;
                }
            }
            TypeBody::Variant(ctors)
        };
        Ok(TypeDecl {
            name,
            params,
            body,
            pos,
        })
    }

    fn fn_decl(&mut self) -> Result<FnDecl> {
        let pos = self.pos();
        self.expect(&Tok::Fn)?;
        let name = match self.peek().clone() {
            Tok::UIdent(ns) => {
                self.bump();
                self.expect(&Tok::Dot)?;
                format!("{}.{}", ns, self.ident()?)
            }
            _ => self.ident()?,
        };
        let params = self.params()?;
        let ret = if self.eat(&Tok::Arrow) {
            Some(self.type_expr()?)
        } else {
            None
        };
        let body = if self.eat(&Tok::Eq) {
            self.expr()?
        } else if self.peek() == &Tok::LBrace {
            self.block()?
        } else {
            return Err(Error::at(
                self.pos(),
                format!("expected `=` or `{{` before the body of `{}`", name),
            ));
        };
        Ok(FnDecl {
            name,
            params,
            ret,
            body,
            pos,
        })
    }

    fn params(&mut self) -> Result<Vec<Param>> {
        self.expect(&Tok::LParen)?;
        let mut ps = Vec::new();
        if !self.eat(&Tok::RParen) {
            loop {
                let pos = self.pos();
                let name = match self.peek() {
                    Tok::Underscore => {
                        self.bump();
                        "_".to_string()
                    }
                    _ => self.ident()?,
                };
                let ty = if self.eat(&Tok::Colon) {
                    Some(self.type_expr()?)
                } else {
                    None
                };
                ps.push(Param { name, ty, pos });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RParen)?;
        }
        Ok(ps)
    }

    pub fn type_expr(&mut self) -> Result<TypeExpr> {
        let pos = self.pos();
        match self.peek().clone() {
            Tok::LParen => {
                self.bump();
                let mut ts = Vec::new();
                if !self.eat(&Tok::RParen) {
                    loop {
                        ts.push(self.type_expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen)?;
                }
                if self.eat(&Tok::Arrow) {
                    let r = self.type_expr()?;
                    return Ok(TypeExpr::Fn(ts, Box::new(r)));
                }
                Ok(match ts.len() {
                    0 => TypeExpr::Con("Unit".into(), vec![], pos),
                    1 => ts.pop().unwrap(),
                    _ => TypeExpr::Tuple(ts),
                })
            }
            Tok::UIdent(n) => {
                self.bump();
                let mut args = Vec::new();
                if self.peek() == &Tok::LBracket && !self.nl() {
                    self.bump();
                    loop {
                        args.push(self.type_expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RBracket)?;
                }
                Ok(TypeExpr::Con(n, args, pos))
            }
            Tok::Ident(n) => {
                self.bump();
                Ok(TypeExpr::Var(n, pos))
            }
            t => Err(Error::at(
                pos,
                format!("expected a type, found {}", t.describe()),
            )),
        }
    }

    // ---------------------------------------------------------- expressions

    pub fn expr(&mut self) -> Result<Expr> {
        self.binary(0)
    }

    fn binop(&self, level: u8) -> Option<(BinOp, bool)> {
        // (operator, may continue on the next line)
        let t = self.peek();
        let r = match (level, t) {
            (1, Tok::OrOr) => (BinOp::Or, true),
            (2, Tok::AndAnd) => (BinOp::And, true),
            (3, Tok::EqEq) => (BinOp::Eq, false),
            (3, Tok::NotEq) => (BinOp::Ne, false),
            (3, Tok::Lt) => (BinOp::Lt, false),
            (3, Tok::Le) => (BinOp::Le, false),
            (3, Tok::Gt) => (BinOp::Gt, false),
            (3, Tok::Ge) => (BinOp::Ge, false),
            (4, Tok::PlusPlus) => (BinOp::Concat, true),
            (5, Tok::Plus) => (BinOp::Add, false),
            (5, Tok::Minus) => (BinOp::Sub, false),
            (6, Tok::Star) => (BinOp::Mul, false),
            (6, Tok::Slash) => (BinOp::Div, false),
            (6, Tok::Percent) => (BinOp::Rem, false),
            _ => return None,
        };
        if self.nl() && !r.1 {
            return None;
        }
        Some(r)
    }

    fn binary(&mut self, level: u8) -> Result<Expr> {
        if level == 0 {
            // `x |> f(a)` is `f(x, a)`; `x |> f` is `f(x)`.
            let mut lhs = self.binary(1)?;
            while self.peek() == &Tok::Pipe {
                let pos = self.pos();
                self.bump();
                let rhs = self.binary(1)?;
                lhs = match rhs.kind {
                    ExprKind::Call(f, mut args) => {
                        args.insert(0, lhs);
                        self.mk(pos, ExprKind::Call(f, args))
                    }
                    ExprKind::Method(r, m, mut args) => {
                        args.insert(0, lhs);
                        self.mk(pos, ExprKind::Method(r, m, args))
                    }
                    _ => self.mk(pos, ExprKind::Call(Box::new(rhs), vec![lhs])),
                };
            }
            return Ok(lhs);
        }
        if level == 7 {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        while let Some((op, _)) = self.binop(level) {
            let pos = self.pos();
            self.bump();
            let rhs = if op == BinOp::Concat {
                self.binary(level)? // right-associative
            } else {
                self.binary(level + 1)?
            };
            lhs = self.mk(pos, ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)));
            if level == 3 {
                if let Some((op2, _)) = self.binop(3) {
                    return Err(Error::at(
                        self.pos(),
                        format!(
                            "comparisons do not chain: parenthesize before `{}`",
                            op2.text()
                        ),
                    ));
                }
            }
            if op == BinOp::Concat {
                break;
            }
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr> {
        let pos = self.pos();
        match self.peek() {
            Tok::Minus => {
                self.bump();
                let e = self.unary()?;
                Ok(match e.kind {
                    ExprKind::Int(n) => self.mk(pos, ExprKind::Int(n.wrapping_neg())),
                    ExprKind::Float(f) => self.mk(pos, ExprKind::Float(-f)),
                    _ => self.mk(pos, ExprKind::Unary(UnOp::Neg, Box::new(e))),
                })
            }
            Tok::Bang => {
                self.bump();
                let e = self.unary()?;
                Ok(self.mk(pos, ExprKind::Unary(UnOp::Not, Box::new(e))))
            }
            _ => self.postfix(),
        }
    }

    fn args(&mut self) -> Result<Vec<Expr>> {
        self.expect(&Tok::LParen)?;
        let save = self.no_struct;
        self.no_struct = false;
        let mut args = Vec::new();
        if !self.eat(&Tok::RParen) {
            loop {
                args.push(self.expr()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
                if self.peek() == &Tok::RParen {
                    break;
                }
            }
            self.expect(&Tok::RParen)?;
        }
        self.no_struct = save;
        Ok(args)
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        loop {
            let pos = self.pos();
            match self.peek().clone() {
                Tok::LParen if !self.nl() => {
                    let args = self.args()?;
                    e = self.mk(pos, ExprKind::Call(Box::new(e), args));
                }
                Tok::LBracket if !self.nl() => {
                    self.bump();
                    let save = self.no_struct;
                    self.no_struct = false;
                    let i = self.expr()?;
                    self.no_struct = save;
                    self.expect(&Tok::RBracket)?;
                    let f = self.mk(pos, ExprKind::Qual("Array".into(), "get".into()));
                    e = self.mk(pos, ExprKind::Call(Box::new(f), vec![e, i]));
                }
                Tok::Dot => {
                    self.bump();
                    match self.peek().clone() {
                        Tok::Ident(name) => {
                            self.bump();
                            if self.peek() == &Tok::LParen && !self.nl() {
                                let args = self.args()?;
                                e = self.mk(pos, ExprKind::Method(Box::new(e), name, args));
                            } else {
                                e = self.mk(pos, ExprKind::Field(Box::new(e), name));
                            }
                        }
                        Tok::Int(n) => {
                            self.bump();
                            e = self.mk(pos, ExprKind::Field(Box::new(e), n.to_string()));
                        }
                        Tok::Float(_) => {
                            // `t.0.1` lexes its fields as the number `0.1`.
                            let tpos = self.pos();
                            let src_text = self.float_text(tpos);
                            self.bump();
                            for part in src_text.split('.') {
                                e = self.mk(pos, ExprKind::Field(Box::new(e), part.to_string()));
                            }
                        }
                        t => {
                            return Err(Error::at(
                                self.pos(),
                                format!("expected a field or method name, found {}", t.describe()),
                            ))
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn float_text(&self, _pos: Pos) -> String {
        match self.peek() {
            Tok::Float(f) => {
                let s = format!("{}", f);
                if s.contains('.') {
                    s
                } else {
                    format!("{}.0", s)
                }
            }
            _ => String::new(),
        }
    }

    fn primary(&mut self) -> Result<Expr> {
        let pos = self.pos();
        let t = self.peek().clone();
        match t {
            Tok::Int(n) => {
                self.bump();
                Ok(self.mk(pos, ExprKind::Int(n)))
            }
            Tok::Float(f) => {
                self.bump();
                Ok(self.mk(pos, ExprKind::Float(f)))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(self.mk(pos, ExprKind::Str(s)))
            }
            Tok::True => {
                self.bump();
                Ok(self.mk(pos, ExprKind::Bool(true)))
            }
            Tok::False => {
                self.bump();
                Ok(self.mk(pos, ExprKind::Bool(false)))
            }
            Tok::Ident(n) => {
                self.bump();
                Ok(self.mk(pos, ExprKind::Var(n)))
            }
            Tok::UIdent(n) => {
                self.bump();
                if self.peek() == &Tok::Dot && matches!(self.peek_at(1), Tok::Ident(_)) {
                    self.bump();
                    let m = self.ident()?;
                    return Ok(self.mk(pos, ExprKind::Qual(n, m)));
                }
                if self.peek() == &Tok::LBrace && !self.no_struct {
                    return self.record(n, pos);
                }
                Ok(self.mk(pos, ExprKind::Var(n)))
            }
            Tok::LParen => {
                self.bump();
                let save = self.no_struct;
                self.no_struct = false;
                if self.eat(&Tok::RParen) {
                    self.no_struct = save;
                    return Ok(self.mk(pos, ExprKind::Unit));
                }
                let first = self.expr()?;
                let r = if self.eat(&Tok::Comma) {
                    let mut es = vec![first];
                    while self.peek() != &Tok::RParen {
                        es.push(self.expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen)?;
                    if es.len() == 1 {
                        es.pop().unwrap()
                    } else {
                        self.mk(pos, ExprKind::Tuple(es))
                    }
                } else {
                    self.expect(&Tok::RParen)?;
                    first
                };
                self.no_struct = save;
                Ok(r)
            }
            Tok::LBracket => {
                self.bump();
                let save = self.no_struct;
                self.no_struct = false;
                let mut es = Vec::new();
                while self.peek() != &Tok::RBracket {
                    es.push(self.expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RBracket)?;
                self.no_struct = save;
                Ok(self.mk(pos, ExprKind::Array(es)))
            }
            Tok::LBrace => self.block(),
            Tok::If => self.if_expr(),
            Tok::Match => {
                self.bump();
                let save = self.no_struct;
                self.no_struct = true;
                let scrut = self.expr()?;
                self.no_struct = save;
                self.expect(&Tok::LBrace)?;
                let mut arms = Vec::new();
                while !self.eat(&Tok::RBrace) {
                    let pat = self.pattern()?;
                    let guard = if self.eat(&Tok::If) {
                        Some(self.expr()?)
                    } else {
                        None
                    };
                    self.expect(&Tok::FatArrow)?;
                    let body = self.expr()?;
                    arms.push(Arm { pat, guard, body });
                    self.eat(&Tok::Comma);
                }
                if arms.is_empty() {
                    return Err(Error::at(pos, "a `match` needs at least one arm"));
                }
                Ok(self.mk(pos, ExprKind::Match(Box::new(scrut), arms)))
            }
            Tok::Fn => {
                self.bump();
                let params = self.params()?;
                let ret = if self.eat(&Tok::Arrow) {
                    Some(self.type_expr()?)
                } else {
                    None
                };
                let save = self.no_struct;
                self.no_struct = false;
                let body = if self.eat(&Tok::FatArrow) {
                    self.expr()?
                } else if self.peek() == &Tok::LBrace {
                    self.block()?
                } else {
                    return Err(Error::at(
                        self.pos(),
                        "expected `=>` or `{` after the parameters",
                    ));
                };
                self.no_struct = save;
                Ok(self.mk(pos, ExprKind::Lambda(params, ret, Box::new(body))))
            }
            t => Err(Error::at(
                pos,
                format!("expected an expression, found {}", t.describe()),
            )),
        }
    }

    fn record(&mut self, name: String, pos: Pos) -> Result<Expr> {
        self.expect(&Tok::LBrace)?;
        let save = self.no_struct;
        self.no_struct = false;
        let mut fields = Vec::new();
        let mut base = None;
        while !self.eat(&Tok::RBrace) {
            if self.eat(&Tok::DotDot) {
                base = Some(Box::new(self.expr()?));
                self.eat(&Tok::Comma);
                self.expect(&Tok::RBrace)?;
                break;
            }
            let fpos = self.pos();
            let f = self.ident()?;
            let v = if self.eat(&Tok::Colon) {
                self.expr()?
            } else {
                // `Point { x, y }` means `Point { x: x, y: y }`.
                self.mk(fpos, ExprKind::Var(f.clone()))
            };
            fields.push((f, v));
            self.eat(&Tok::Comma);
        }
        self.no_struct = save;
        Ok(self.mk(pos, ExprKind::Record(name, fields, base)))
    }

    fn if_expr(&mut self) -> Result<Expr> {
        let pos = self.pos();
        self.expect(&Tok::If)?;
        let save = self.no_struct;
        self.no_struct = true;
        let c = self.expr()?;
        self.no_struct = save;
        let t = self.block()?;
        let e = if self.eat(&Tok::Else) {
            if self.peek() == &Tok::If {
                Some(Box::new(self.if_expr()?))
            } else {
                Some(Box::new(self.block()?))
            }
        } else {
            None
        };
        Ok(self.mk(pos, ExprKind::If(Box::new(c), Box::new(t), e)))
    }

    fn block(&mut self) -> Result<Expr> {
        let pos = self.pos();
        self.expect(&Tok::LBrace)?;
        let save = self.no_struct;
        self.no_struct = false;
        let mut stmts = Vec::new();
        let mut tail: Option<Box<Expr>> = None;
        loop {
            while self.eat(&Tok::Semi) {}
            if self.eat(&Tok::RBrace) {
                break;
            }
            if let Some(t) = tail.take() {
                stmts.push(Stmt::Expr(*t));
            }
            if self.peek() == &Tok::Let {
                let lpos = self.pos();
                self.bump();
                let pat = self.pattern()?;
                if !pat.irrefutable() {
                    return Err(Error::at(
                        lpos,
                        "a `let` pattern must always match; use `match` for this pattern",
                    ));
                }
                let ty = if self.eat(&Tok::Colon) {
                    Some(self.type_expr()?)
                } else {
                    None
                };
                self.expect(&Tok::Eq)?;
                let e = self.expr()?;
                stmts.push(Stmt::Let(pat, ty, e));
            } else {
                let e = self.expr()?;
                if self.eat(&Tok::Semi) {
                    stmts.push(Stmt::Expr(e));
                } else {
                    tail = Some(Box::new(e));
                }
            }
            if !matches!(self.peek(), Tok::RBrace | Tok::Semi) && !self.nl() {
                return Err(Error::at(
                    self.pos(),
                    format!(
                        "expected a new line or `;` before {}",
                        self.peek().describe()
                    ),
                ));
            }
        }
        self.no_struct = save;
        Ok(self.mk(pos, ExprKind::Block(stmts, tail)))
    }

    // ------------------------------------------------------------- patterns

    fn pattern(&mut self) -> Result<Pat> {
        let pos = self.pos();
        let kind = match self.peek().clone() {
            Tok::Underscore => {
                self.bump();
                PatKind::Wild
            }
            Tok::Ident(n) => {
                self.bump();
                PatKind::Var(n)
            }
            Tok::Int(n) => {
                self.bump();
                PatKind::Int(n)
            }
            Tok::Minus => {
                self.bump();
                match self.bump() {
                    Tok::Int(n) => PatKind::Int(n.wrapping_neg()),
                    _ => return Err(Error::at(pos, "expected an integer after `-`")),
                }
            }
            Tok::Str(s) => {
                self.bump();
                PatKind::Str(s)
            }
            Tok::True => {
                self.bump();
                PatKind::Bool(true)
            }
            Tok::False => {
                self.bump();
                PatKind::Bool(false)
            }
            Tok::LParen => {
                self.bump();
                if self.eat(&Tok::RParen) {
                    PatKind::Unit
                } else {
                    let mut ps = vec![self.pattern()?];
                    while self.eat(&Tok::Comma) {
                        if self.peek() == &Tok::RParen {
                            break;
                        }
                        ps.push(self.pattern()?);
                    }
                    self.expect(&Tok::RParen)?;
                    if ps.len() == 1 {
                        return Ok(ps.pop().unwrap());
                    }
                    PatKind::Tuple(ps)
                }
            }
            Tok::UIdent(n) => {
                self.bump();
                if self.peek() == &Tok::LParen && !self.nl() {
                    self.bump();
                    let mut ps = Vec::new();
                    if !self.eat(&Tok::RParen) {
                        loop {
                            ps.push(self.pattern()?);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                        self.expect(&Tok::RParen)?;
                    }
                    PatKind::Ctor(n, ps)
                } else if self.peek() == &Tok::LBrace && !self.nl() {
                    self.bump();
                    let mut fs = Vec::new();
                    while !self.eat(&Tok::RBrace) {
                        if self.eat(&Tok::DotDot) {
                            // The other fields are ignored anyway.
                            self.expect(&Tok::RBrace)?;
                            break;
                        }
                        let fpos = self.pos();
                        let f = self.ident()?;
                        let p = if self.eat(&Tok::Colon) {
                            self.pattern()?
                        } else {
                            Pat {
                                pos: fpos,
                                kind: PatKind::Var(f.clone()),
                            }
                        };
                        fs.push((f, p));
                        self.eat(&Tok::Comma);
                    }
                    PatKind::Record(n, fs)
                } else {
                    PatKind::Ctor(n, vec![])
                }
            }
            t => {
                return Err(Error::at(
                    pos,
                    format!("expected a pattern, found {}", t.describe()),
                ))
            }
        };
        Ok(Pat { pos, kind })
    }
}
