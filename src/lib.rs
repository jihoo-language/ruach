//! Ruach: a functional, general-purpose language compiled to C.

pub mod ast;
pub mod builtins;
pub mod emit;
pub mod error;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod rc;
pub mod types;

use std::path::{Path, PathBuf};
use std::process::Command;

pub use error::Error;

pub const PRELUDE: &str = include_str!("prelude.rh");

/// Reads a program and the files it imports.
pub fn load(path: &str, src: &str) -> Result<ast::Program, Error> {
    let mut files: Vec<String> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut decls = Vec::new();
    let mut next_id = 0usize;
    let mut todo: Vec<(String, String, Option<PathBuf>)> = vec![
        ("<prelude>".into(), PRELUDE.to_string(), None),
        (
            path.to_string(),
            src.to_string(),
            Path::new(path).canonicalize().ok(),
        ),
    ];
    if let Some(p) = &todo[1].2 {
        seen.push(p.clone());
    }
    let mut i = 0;
    while i < todo.len() {
        let (name, text, canon) = todo[i].clone();
        let file = files.len() as u32;
        files.push(name.clone());
        let result = (|| {
            let toks = lexer::lex(&text, file)?;
            parser::parse(toks, &mut next_id)
        })();
        let parsed = result.map_err(|e| named(e, &files))?;
        decls.extend(parsed.decls);
        for (imp, pos) in parsed.imports {
            let base = canon
                .as_ref()
                .and_then(|c| c.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."));
            let target = base.join(&imp);
            let canon_t = target.canonicalize().map_err(|e| {
                named(
                    Error::at(pos, format!("cannot import \"{}\": {}", imp, e)),
                    &files,
                )
            })?;
            if seen.contains(&canon_t) {
                continue;
            }
            seen.push(canon_t.clone());
            let text = std::fs::read_to_string(&canon_t).map_err(|e| {
                named(
                    Error::at(pos, format!("cannot import \"{}\": {}", imp, e)),
                    &files,
                )
            })?;
            todo.push((target.display().to_string(), text, Some(canon_t)));
        }
        i += 1;
    }
    FILES.with(|f| *f.borrow_mut() = files);
    ctor_receivers(&mut decls, &mut next_id);
    Ok(ast::Program {
        decls,
        n_exprs: next_id,
    })
}

/// `None.unwrap()` parses as the qualified name `None.unwrap`; when the
/// name before the dot is a constructor and no such function exists, it is
/// a method call on the constructor's value.
fn ctor_receivers(decls: &mut [ast::Decl], next_id: &mut usize) {
    use ast::*;
    let mut ctors = std::collections::HashSet::new();
    let mut fns: std::collections::HashSet<String> = builtins::BUILTINS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    for d in decls.iter() {
        match d {
            Decl::Type(TypeDecl {
                body: TypeBody::Variant(cs),
                ..
            }) => {
                ctors.extend(cs.iter().map(|c| c.name.clone()));
            }
            Decl::Fn(f) => {
                fns.insert(f.name.clone());
            }
            _ => {}
        }
    }
    fn walk(
        e: &mut Expr,
        ctors: &std::collections::HashSet<String>,
        fns: &std::collections::HashSet<String>,
        id: &mut usize,
    ) {
        let is_ctor_qual = |f: &Expr| matches!(&f.kind, ExprKind::Qual(c, m) if ctors.contains(c) && !fns.contains(&format!("{}.{}", c, m)));
        if let ExprKind::Call(f, _) = &e.kind {
            if is_ctor_qual(f) {
                let ExprKind::Call(f, args) = std::mem::replace(&mut e.kind, ExprKind::Unit) else {
                    unreachable!()
                };
                let ExprKind::Qual(c, m) = f.kind else {
                    unreachable!()
                };
                let recv = Expr {
                    id: *id,
                    pos: f.pos,
                    kind: ExprKind::Var(c),
                };
                *id += 1;
                e.kind = ExprKind::Method(Box::new(recv), m, args);
            }
        } else if is_ctor_qual(e) {
            let ExprKind::Qual(c, m) = std::mem::replace(&mut e.kind, ExprKind::Unit) else {
                unreachable!()
            };
            let recv = Expr {
                id: *id,
                pos: e.pos,
                kind: ExprKind::Var(c),
            };
            *id += 1;
            e.kind = ExprKind::Field(Box::new(recv), m);
        }
        match &mut e.kind {
            ExprKind::Call(f, args) => {
                walk(f, ctors, fns, id);
                args.iter_mut().for_each(|a| walk(a, ctors, fns, id));
            }
            ExprKind::Method(r, _, args) => {
                walk(r, ctors, fns, id);
                args.iter_mut().for_each(|a| walk(a, ctors, fns, id));
            }
            ExprKind::Field(x, _) | ExprKind::Unary(_, x) => walk(x, ctors, fns, id),
            ExprKind::Binary(_, a, b) => {
                walk(a, ctors, fns, id);
                walk(b, ctors, fns, id);
            }
            ExprKind::If(c, t, f) => {
                walk(c, ctors, fns, id);
                walk(t, ctors, fns, id);
                if let Some(f) = f {
                    walk(f, ctors, fns, id);
                }
            }
            ExprKind::Match(s, arms) => {
                walk(s, ctors, fns, id);
                for a in arms {
                    if let Some(g) = &mut a.guard {
                        walk(g, ctors, fns, id);
                    }
                    walk(&mut a.body, ctors, fns, id);
                }
            }
            ExprKind::Block(ss, t) => {
                for s in ss {
                    match s {
                        Stmt::Let(_, _, x) | Stmt::Expr(x) => walk(x, ctors, fns, id),
                    }
                }
                if let Some(t) = t {
                    walk(t, ctors, fns, id);
                }
            }
            ExprKind::Lambda(_, _, b) => walk(b, ctors, fns, id),
            ExprKind::Tuple(es) | ExprKind::Array(es) => {
                es.iter_mut().for_each(|x| walk(x, ctors, fns, id))
            }
            ExprKind::Record(_, fs, b) => {
                fs.iter_mut().for_each(|(_, x)| walk(x, ctors, fns, id));
                if let Some(b) = b {
                    walk(b, ctors, fns, id);
                }
            }
            _ => {}
        }
    }
    for d in decls.iter_mut() {
        if let Decl::Fn(f) = d {
            walk(&mut f.body, &ctors, &fns, next_id);
        }
    }
}

thread_local! {
    static FILES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn named(mut e: Error, files: &[String]) -> Error {
    if let Some(p) = e.pos {
        e.file = files.get(p.file as usize).cloned();
    }
    e
}

fn with_file(e: Error) -> Error {
    FILES.with(|f| named(e, &f.borrow()))
}

/// Type-checks a program.
pub fn check(path: &str, src: &str) -> Result<(), Error> {
    let prog = load(path, src)?;
    let ck = types::check(&prog).map_err(with_file)?;
    // Generic code is checked for each instance the program uses.
    lower::lower(&prog, &ck).map_err(with_file)?;
    Ok(())
}

/// Compiles a program to a C translation unit.
pub fn compile_to_c(path: &str, src: &str) -> Result<String, Error> {
    let prog = load(path, src)?;
    let ck = types::check(&prog).map_err(with_file)?;
    let mut ir = lower::lower(&prog, &ck).map_err(with_file)?;
    rc::insert(&mut ir);
    Ok(emit::emit(&ir))
}

/// The C compiler: `$RUACH_CC`, or `cc`.
pub fn c_compiler() -> String {
    std::env::var("RUACH_CC").unwrap_or_else(|_| "cc".to_string())
}

/// Compiles C code to an executable.
pub fn build_c(c: &str, out: &Path, opt: &str) -> Result<(), Error> {
    let dir = scratch_dir()?;
    let cfile = dir.join("main.c");
    std::fs::write(&cfile, c)
        .map_err(|e| Error::new(format!("cannot write {}: {}", cfile.display(), e)))?;
    let cc = c_compiler();
    let extra: Vec<String> = std::env::var("RUACH_CFLAGS")
        .map(|v| v.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    let output = Command::new(&cc)
        .arg(opt)
        .args(&extra)
        .args(["-std=gnu11", "-w", "-fno-strict-aliasing", "-o"])
        .arg(out)
        .arg(&cfile)
        .arg("-lm")
        .output()
        .map_err(|e| {
            Error::new(format!(
                "cannot run the C compiler `{}`: {} (set RUACH_CC)",
                cc, e
            ))
        })?;
    let _ = std::fs::remove_dir_all(&dir);
    if !output.status.success() {
        return Err(Error::new(format!(
            "the C compiler failed (this is a bug in Ruach):\n{}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

pub fn scratch_dir() -> Result<PathBuf, Error> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let k = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ruach-{}-{}-{}", std::process::id(), nanos, k));
    std::fs::create_dir_all(&dir)
        .map_err(|e| Error::new(format!("cannot create {}: {}", dir.display(), e)))?;
    Ok(dir)
}
