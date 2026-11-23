//! C code generation. Types become C types, and each concrete type gets
//! its own reference-counting, equality and printing functions.

use std::collections::HashMap;
use std::fmt::Write;

use crate::ir::*;

const RUNTIME: &str = include_str!("runtime.c");

struct Emitter<'a> {
    p: &'a IrProgram,
    out: String,
    lits: String,
    lit_ids: HashMap<String, usize>,
    label: usize,
    statics: String,
}

pub fn emit(p: &IrProgram) -> String {
    let mut e = Emitter {
        p,
        out: String::new(),
        lits: String::new(),
        lit_ids: HashMap::new(),
        label: 0,
        statics: String::new(),
    };
    e.program();
    e.out
}

fn lit_bytes(s: &str) -> String {
    let mut o = String::from("\"");
    for b in s.bytes() {
        match b {
            b'"' => o.push_str("\\\""),
            b'\\' => o.push_str("\\\\"),
            b'?' => o.push_str("\\?"),
            0x20..=0x7e => o.push(b as char),
            _ => {
                let _ = write!(o, "\\{:03o}", b);
            }
        }
    }
    o.push('"');
    o
}

impl<'a> Emitter<'a> {
    fn repr(&self, t: TyId) -> &Repr {
        &self.p.tys.reprs[t]
    }

    /// For a variant type with exactly one constructor that has fields:
    /// that constructor. Its cells need no tag.
    fn tagless(&self, t: TyId) -> Option<usize> {
        match self.repr(t) {
            Repr::Boxed(_, cs) => {
                let with: Vec<usize> = cs
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| !c.1.is_empty())
                    .map(|(i, _)| i)
                    .collect();
                if with.len() == 1 {
                    Some(with[0])
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn tag_of(&self, t: TyId, x: &str) -> String {
        match self.tagless(t) {
            Some(c) => format!(
                "(rh_is_cell({0}) ? {1} : (int64_t)((uintptr_t){0} >> 1))",
                x, c
            ),
            None => format!("rh_tag({})", x),
        }
    }

    fn managed(&self, t: TyId) -> bool {
        self.p.tys.managed(t)
    }

    fn cty(&self, t: TyId) -> String {
        match self.repr(t) {
            Repr::Int | Repr::Enum(..) => "int64_t".into(),
            Repr::Float => "double".into(),
            Repr::Bool => "bool".into(),
            Repr::Unit => "uint8_t".into(),
            Repr::Str => "Str*".into(),
            Repr::Arr(_) => format!("A{}", t),
            Repr::Clo(..) => "Clo*".into(),
            Repr::Tuple(_) | Repr::Record(..) => format!("S{}", t),
            Repr::Boxed(..) => "void*".into(),
            Repr::Pending => unreachable!(),
        }
    }

    fn inc(&self, t: TyId, x: &str) -> String {
        if self.managed(t) {
            format!("inc{}({});", t, x)
        } else {
            String::new()
        }
    }

    fn dec(&self, t: TyId, x: &str) -> String {
        if self.managed(t) {
            format!("dec{}({});", t, x)
        } else {
            String::new()
        }
    }

    fn program(&mut self) {
        self.out.push_str(RUNTIME);
        self.out.push_str("\n/* ---- types ---- */\n");
        let n = self.p.tys.len();
        // Forward declarations.
        for t in 0..n {
            match self.repr(t) {
                Repr::Tuple(_) | Repr::Record(..) => {
                    let _ = writeln!(self.out, "typedef struct S{0} S{0};", t);
                }
                Repr::Arr(_) => {
                    let _ = writeln!(self.out, "typedef struct arr{0} *A{0};", t);
                }
                _ => {}
            }
        }
        // Structs, each after the structs it contains.
        let mut done = vec![false; n];
        for t in 0..n {
            self.struct_def(t, &mut done);
        }
        for t in 0..n {
            match self.repr(t).clone() {
                Repr::Arr(e) => {
                    let _ = writeln!(
                        self.out,
                        "struct arr{} {{ int64_t rc, len, cap; {} data[]; }};",
                        t,
                        self.cty(e)
                    );
                }
                Repr::Boxed(_, cs) => {
                    for (k, (_, fs)) in cs.iter().enumerate() {
                        if fs.is_empty() {
                            continue;
                        }
                        let mut s = if self.tagless(t).is_some() {
                            "typedef struct { int64_t rc;".to_string()
                        } else {
                            "typedef struct { int64_t rc; int64_t tag;".to_string()
                        };
                        for (i, f) in fs.iter().enumerate() {
                            let _ = write!(s, " {} f{};", self.cty(*f), i);
                        }
                        let _ = writeln!(s, " }} C{}_{};", t, k);
                        self.out.push_str(&s);
                    }
                }
                _ => {}
            }
        }
        for (id, f) in self.p.funcs.iter().enumerate() {
            if let Some((_, caps)) = &f.env {
                if !caps.is_empty() {
                    let mut s = String::from("typedef struct { Clo h;");
                    for (i, c) in caps.iter().enumerate() {
                        let _ = write!(s, " {} c{};", self.cty(*c), i);
                    }
                    let _ = writeln!(s, " }} E{};", id);
                    self.out.push_str(&s);
                }
            }
        }
        self.out.push_str("\n/* ---- helpers ---- */\n");
        for t in 0..n {
            self.helper_protos(t);
        }
        for t in 0..n {
            self.helpers(t);
        }
        self.out.push_str("\n/* ---- functions ---- */\n");
        for (id, f) in self.p.funcs.iter().enumerate() {
            let sig = self.signature(id, f);
            let _ = writeln!(self.out, "{};", sig);
        }
        for (id, f) in self.p.funcs.iter().enumerate() {
            if let Some((_, caps)) = &f.env {
                if caps.is_empty() {
                    let _ = writeln!(
                        self.out,
                        "static Clo sclo{0} = {{ RH_IMMORTAL, (void *)fn{0}, 0 }};",
                        id
                    );
                } else {
                    let mut s = format!("static void edrop{0}(Clo *c) {{ E{0} *e = (E{0} *)c;", id);
                    for (i, c) in caps.iter().enumerate() {
                        s.push_str(&self.dec(*c, &format!("e->c{}", i)));
                    }
                    s.push_str(" free(c); }\n");
                    self.out.push_str(&s);
                }
            }
        }
        let mut bodies = String::new();
        for (id, f) in self.p.funcs.iter().enumerate() {
            bodies.push_str(&self.function(id, f));
        }
        self.out.push_str(&self.lits.clone());
        self.out.push_str(&self.statics.clone());
        self.out.push_str(&bodies);
        let _ = write!(
            self.out,
            r#"
static ucontext_t rh_uc_main, rh_uc_prog;

static void rh_entry(void) {{
    fn{0}();
}}

int main(int argc, char **argv) {{
    rh_argc = argc;
    rh_argv = argv;
    /* Run on a large stack of our own, so that deep recursion works,
       without starting a thread (which would slow down malloc). */
    size_t size = (size_t)1 << 30;
    char *stack = mmap(0, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    if (stack != MAP_FAILED && getcontext(&rh_uc_prog) == 0) {{
        mprotect(stack, 4096, PROT_NONE);
        rh_uc_prog.uc_stack.ss_sp = stack;
        rh_uc_prog.uc_stack.ss_size = size;
        rh_uc_prog.uc_link = &rh_uc_main;
        makecontext(&rh_uc_prog, rh_entry, 0);
        swapcontext(&rh_uc_main, &rh_uc_prog);
    }} else {{
        rh_entry();
    }}
    fflush(stdout);
    return 0;
}}
"#,
            self.p.main
        );
    }

    fn struct_def(&mut self, t: TyId, done: &mut Vec<bool>) {
        if done[t] {
            return;
        }
        let fields: Vec<TyId> = match self.repr(t) {
            Repr::Tuple(ts) => ts.clone(),
            Repr::Record(_, fs) => fs.iter().map(|(_, t)| *t).collect(),
            _ => {
                done[t] = true;
                return;
            }
        };
        done[t] = true;
        for &f in &fields {
            self.struct_def(f, done);
        }
        let mut s = format!("struct S{} {{", t);
        for (i, f) in fields.iter().enumerate() {
            let _ = write!(s, " {} f{};", self.cty(*f), i);
        }
        if fields.is_empty() {
            s.push_str(" char unused;");
        }
        s.push_str(" };\n");
        self.out.push_str(&s);
    }

    fn helper_protos(&mut self, t: TyId) {
        let c = self.cty(t);
        let mut s = String::new();
        if self.managed(t) {
            let _ = writeln!(
                s,
                "static inline void inc{0}({1} x);\nstatic inline void dec{0}({1} x);",
                t, c
            );
        }
        if matches!(self.repr(t), Repr::Arr(_) | Repr::Boxed(..)) {
            let _ = writeln!(s, "static void drop{0}({1} x);", t, c);
        }
        let _ = writeln!(
            s,
            "static bool eq{0}({1} a, {1} b);\nstatic void show{0}(Str **s, {1} x);",
            t, c
        );
        let _ = writeln!(s, "static Str *tostr{0}({1} x);", t, c);
        self.out.push_str(&s);
    }

    fn helpers(&mut self, t: TyId) {
        let c = self.cty(t);
        let repr = self.repr(t).clone();
        let mut s = String::new();
        // inc / dec / drop
        if self.managed(t) {
            match &repr {
                Repr::Str => {
                    let _ = writeln!(s, "static inline void inc{0}(Str *x) {{ rh_str_inc(x); }}\nstatic inline void dec{0}(Str *x) {{ rh_str_dec(x); }}", t);
                }
                Repr::Clo(..) => {
                    let _ = writeln!(s, "static inline void inc{0}(Clo *x) {{ rh_clo_inc(x); }}\nstatic inline void dec{0}(Clo *x) {{ rh_clo_dec(x); }}", t);
                }
                Repr::Arr(_) => {
                    let _ = writeln!(s, "static inline void inc{0}({1} x) {{ x->rc++; }}\nstatic inline void dec{0}({1} x) {{ if (--x->rc == 0) drop{0}(x); }}", t, c);
                }
                Repr::Boxed(..) => {
                    let _ = writeln!(s, "static inline void inc{0}(void *x) {{ rh_cell_inc(x); }}\nstatic inline void dec{0}(void *x) {{ if (rh_cell_release(x)) drop{0}(x); }}", t);
                }
                Repr::Tuple(fs) => {
                    let fs: Vec<TyId> = fs.clone();
                    self.struct_rc(&mut s, t, &c, &fs);
                }
                Repr::Record(_, fs) => {
                    let fs: Vec<TyId> = fs.iter().map(|(_, t)| *t).collect();
                    self.struct_rc(&mut s, t, &c, &fs);
                }
                _ => {}
            }
        }
        match &repr {
            Repr::Arr(e) => {
                let e = *e;
                let ec = self.cty(e);
                let mut body = String::new();
                if self.managed(e) {
                    let _ = write!(
                        body,
                        "for (int64_t i = 0; i < x->len; i++) {} ",
                        self.dec(e, "x->data[i]")
                    );
                }
                let _ = writeln!(
                    s,
                    "static void drop{0}({1} x) {{ {2}free(x); }}",
                    t, c, body
                );
                self.array_ops(&mut s, t, &c, e, &ec);
            }
            Repr::Boxed(_, cs) => {
                let mut body = format!("switch ({}) {{", self.tag_of(t, "x"));
                for (k, (_, fs)) in cs.iter().enumerate() {
                    if fs.is_empty() {
                        continue;
                    }
                    let _ = write!(body, " case {}: {{ C{}_{} *c = x; (void)c;", k, t, k);
                    for (i, f) in fs.iter().enumerate() {
                        body.push_str(&self.dec(*f, &format!("c->f{}", i)));
                    }
                    body.push_str(" break; }");
                }
                body.push_str(" }");
                let _ = writeln!(
                    s,
                    "static void drop{0}(void *x) {{ {1} free(x); }}",
                    t, body
                );
            }
            _ => {}
        }
        // equality
        let eq = match &repr {
            Repr::Int | Repr::Float | Repr::Bool | Repr::Enum(..) => "return a == b;".to_string(),
            Repr::Unit => "(void)a; (void)b; return true;".to_string(),
            Repr::Str => "return rh_str_eq(a, b);".to_string(),
            Repr::Clo(..) => "return a == b;".to_string(),
            Repr::Arr(e) => format!(
                "if (a == b) return true; if (a->len != b->len) return false; for (int64_t i = 0; i < a->len; i++) if (!eq{}(a->data[i], b->data[i])) return false; return true;",
                e
            ),
            Repr::Tuple(fs) => {
                let parts: Vec<String> = fs.iter().enumerate().map(|(i, f)| format!("eq{}(a.f{i}, b.f{i})", f, i = i)).collect();
                format!("(void)a; (void)b; return {};", if parts.is_empty() { "true".into() } else { parts.join(" && ") })
            }
            Repr::Record(_, fs) => {
                let parts: Vec<String> = fs.iter().enumerate().map(|(i, (_, f))| format!("eq{}(a.f{i}, b.f{i})", f, i = i)).collect();
                format!("(void)a; (void)b; return {};", if parts.is_empty() { "true".into() } else { parts.join(" && ") })
            }
            Repr::Boxed(_, cs) => {
                let mut b = format!(
                    "if (a == b) return true; if (!rh_is_cell(a) || !rh_is_cell(b)) return false; if ({0} != {1}) return false; switch ({0}) {{",
                    self.tag_of(t, "a"),
                    self.tag_of(t, "b")
                );
                for (k, (_, fs)) in cs.iter().enumerate() {
                    if fs.is_empty() {
                        continue;
                    }
                    let _ = write!(b, " case {k}: {{ C{t}_{k} *x = a, *y = b; return true", k = k, t = t);
                    for (i, f) in fs.iter().enumerate() {
                        let _ = write!(b, " && eq{}(x->f{i}, y->f{i})", f, i = i);
                    }
                    b.push_str("; }");
                }
                b.push_str(" } return false;");
                b
            }
            Repr::Pending => unreachable!(),
        };
        let _ = writeln!(s, "static bool eq{0}({1} a, {1} b) {{ {2} }}", t, c, eq);
        // printing
        let show = match &repr {
            Repr::Int => "rh_show_int(s, x);".to_string(),
            Repr::Float => "rh_show_float(s, x);".to_string(),
            Repr::Bool => "rh_puts(s, x ? \"true\" : \"false\");".to_string(),
            Repr::Unit => "(void)x; rh_puts(s, \"()\");".to_string(),
            Repr::Str => "rh_show_str(s, x);".to_string(),
            Repr::Clo(..) => "(void)x; rh_puts(s, \"<fn>\");".to_string(),
            Repr::Enum(_, names) => {
                let mut b = String::from("switch (x) {");
                for (k, n) in names.iter().enumerate() {
                    let _ = write!(b, " case {}: rh_puts(s, {}); break;", k, lit_bytes(n));
                }
                b.push_str(" }");
                b
            }
            Repr::Arr(e) => format!(
                "rh_puts(s, \"[\"); for (int64_t i = 0; i < x->len; i++) {{ if (i) rh_puts(s, \", \"); show{}(s, x->data[i]); }} rh_puts(s, \"]\");",
                e
            ),
            Repr::Tuple(fs) => {
                let mut b = String::from("rh_puts(s, \"(\");");
                for (i, f) in fs.iter().enumerate() {
                    if i > 0 {
                        b.push_str(" rh_puts(s, \", \");");
                    }
                    let _ = write!(b, " show{}(s, x.f{});", f, i);
                }
                b.push_str(" rh_puts(s, \")\");");
                b
            }
            Repr::Record(name, fs) => {
                let mut b = format!("rh_puts(s, {});", lit_bytes(&format!("{} {{ ", name)));
                for (i, (fname, f)) in fs.iter().enumerate() {
                    let sep = if i > 0 { ", " } else { "" };
                    let _ = write!(b, " rh_puts(s, {}); show{}(s, x.f{});", lit_bytes(&format!("{}{}: ", sep, fname)), f, i);
                }
                b.push_str(" rh_puts(s, \" }\");");
                b
            }
            Repr::Boxed(_, cs) => {
                let mut b = format!("switch ({}) {{", self.tag_of(t, "x"));
                for (k, (n, fs)) in cs.iter().enumerate() {
                    let _ = write!(b, " case {}: rh_puts(s, {});", k, lit_bytes(n));
                    if !fs.is_empty() {
                        let _ = write!(b, " {{ C{}_{} *c = x; rh_puts(s, \"(\");", t, k);
                        for (i, f) in fs.iter().enumerate() {
                            if i > 0 {
                                b.push_str(" rh_puts(s, \", \");");
                            }
                            let _ = write!(b, " show{}(s, c->f{});", f, i);
                        }
                        b.push_str(" rh_puts(s, \")\"); }");
                    }
                    b.push_str(" break;");
                }
                b.push_str(" }");
                b
            }
            Repr::Pending => unreachable!(),
        };
        let _ = writeln!(
            s,
            "static void show{0}(Str **s, {1} x) {{ {2} }}",
            t, c, show
        );
        let _ = writeln!(
            s,
            "static Str *tostr{0}({1} x) {{ Str *s = rh_str_new(15); show{0}(&s, x); return s; }}",
            t, c
        );
        self.out.push_str(&s);
    }

    fn struct_rc(&self, s: &mut String, t: TyId, c: &str, fs: &[TyId]) {
        let mut inc = String::new();
        let mut dec = String::new();
        for (i, f) in fs.iter().enumerate() {
            inc.push_str(&self.inc(*f, &format!("x.f{}", i)));
            dec.push_str(&self.dec(*f, &format!("x.f{}", i)));
        }
        let _ = writeln!(s, "static inline void inc{0}({1} x) {{ {2} }}\nstatic inline void dec{0}({1} x) {{ {3} }}", t, c, inc, dec);
    }

    fn array_ops(&self, s: &mut String, t: TyId, c: &str, e: TyId, ec: &str) {
        let inc_i = |x: &str| self.inc(e, x);
        let dec_i = |x: &str| self.dec(e, x);
        let _ = writeln!(
            s,
            "static {c} anew{t}(int64_t cap) {{ {c} a = rh_alloc(sizeof(struct arr{t}) + sizeof({ec}) * (size_t)cap); a->rc = 1; a->len = 0; a->cap = cap; return a; }}",
            c = c, t = t, ec = ec
        );
        let _ = writeln!(
            s,
            "static {c} auniq{t}({c} a, int64_t need) {{ if (RH_LIKELY(a->rc == 1)) {{ if (RH_LIKELY(a->cap >= need)) return a; int64_t cap = a->cap * 2; if (cap < need) cap = need; if (cap < 4) cap = 4; a = rh_realloc(a, sizeof(struct arr{t}) + sizeof({ec}) * (size_t)cap); a->cap = cap; return a; }} {c} b = anew{t}(need > a->len ? need : a->len); b->len = a->len; for (int64_t i = 0; i < a->len; i++) {{ b->data[i] = a->data[i]; {inc} }} a->rc--; return b; }}",
            c = c, t = t, ec = ec, inc = inc_i("b->data[i]")
        );
        let _ = writeln!(
            s,
            "static inline {ec} aget{t}({c} a, int64_t i) {{ if (RH_UNLIKELY((uint64_t)i >= (uint64_t)a->len)) rh_oob(i, a->len); {ec} x = a->data[i]; {inc} return x; }}",
            c = c, t = t, ec = ec, inc = inc_i("x")
        );
        let _ = writeln!(
            s,
            "static inline {c} aset{t}({c} a, int64_t i, {ec} x) {{ if (RH_UNLIKELY((uint64_t)i >= (uint64_t)a->len)) rh_oob(i, a->len); a = auniq{t}(a, a->len); {dec} a->data[i] = x; return a; }}",
            c = c, t = t, ec = ec, dec = dec_i("a->data[i]")
        );
        let _ = writeln!(
            s,
            "static inline {c} apush{t}({c} a, {ec} x) {{ a = auniq{t}(a, a->len + 1); a->data[a->len++] = x; return a; }}",
            c = c, t = t, ec = ec
        );
        let _ = writeln!(
            s,
            "static inline {c} apop{t}({c} a) {{ if (a->len == 0) rh_die(\"pop from an empty array\"); a = auniq{t}(a, a->len); a->len--; {dec} return a; }}",
            c = c, t = t, dec = dec_i("a->data[a->len]")
        );
        let _ = writeln!(
            s,
            "static {c} aslice{t}({c} a, int64_t i, int64_t j) {{ if (i < 0) i = 0; if (j > a->len) j = a->len; if (j < i) j = i; {c} b = anew{t}(j - i); for (int64_t k = i; k < j; k++) {{ b->data[k - i] = a->data[k]; {inc} }} b->len = j - i; return b; }}",
            c = c, t = t, inc = inc_i("b->data[k - i]")
        );
        let _ = writeln!(
            s,
            "static inline {c} aswap{t}({c} a, int64_t i, int64_t j) {{ if (RH_UNLIKELY((uint64_t)i >= (uint64_t)a->len)) rh_oob(i, a->len); if (RH_UNLIKELY((uint64_t)j >= (uint64_t)a->len)) rh_oob(j, a->len); a = auniq{t}(a, a->len); {ec} x = a->data[i]; a->data[i] = a->data[j]; a->data[j] = x; return a; }}",
            c = c, t = t, ec = ec
        );
        let _ = writeln!(
            s,
            "static {c} aconcat{t}({c} a, {c} b) {{ a = auniq{t}(a, a->len + b->len); for (int64_t k = 0; k < b->len; k++) {{ a->data[a->len + k] = b->data[k]; {inc} }} a->len += b->len; dec{t}(b); return a; }}",
            c = c, t = t, inc = inc_i("a->data[a->len + k]")
        );
        let _ = writeln!(
            s,
            "static {c} amake{t}(int64_t n, {ec} x) {{ if (n < 0) n = 0; {c} a = anew{t}(n); for (int64_t i = 0; i < n; i++) {{ a->data[i] = x; {inc_rest} }} {dec_empty} a->len = n; return a; }}",
            c = c, t = t, ec = ec,
            inc_rest = if self.managed(e) { format!("if (i) {}", inc_i("x")) } else { String::new() },
            dec_empty = if self.managed(e) { format!("if (n == 0) {{ {} }}", dec_i("x")) } else { String::new() }
        );
    }

    // ----------------------------------------------------------- functions

    fn signature(&self, id: FnId, f: &IrFunc) -> String {
        let mut ps = Vec::new();
        if f.env.is_some() {
            ps.push("Clo *env".to_string());
        }
        for &p in &f.params {
            ps.push(format!("{} v{}", self.cty(f.vars[p].ty), p));
        }
        if ps.is_empty() {
            ps.push("void".into());
        }
        format!("static {} fn{}({})", self.cty(f.ret), id, ps.join(", "))
    }

    fn var(&self, f: &IrFunc, v: VarId) -> String {
        if f.vars[v].kind == VarKind::Env {
            "env".into()
        } else {
            format!("v{}", v)
        }
    }

    fn function(&mut self, id: FnId, f: &IrFunc) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} {{ /* {} */",
            self.signature(id, f),
            f.name.replace("*/", "* /")
        );
        for (v, info) in f.vars.iter().enumerate() {
            if f.params.contains(&v) || info.kind == VarKind::Env {
                continue;
            }
            let _ = writeln!(s, "    {} v{};", self.cty(info.ty), v);
        }
        if has_tail(&f.body) {
            s.push_str("top:;\n");
        }
        let mut ctx = Ctx {
            blocks: vec![],
            fails: vec![],
            fid: id,
        };
        self.ir(&mut s, f, &f.body, &mut ctx, 1);
        s.push_str("}\n\n");
        s
    }

    fn new_label(&mut self) -> String {
        self.label += 1;
        format!("L{}", self.label)
    }

    fn ir(&mut self, s: &mut String, f: &IrFunc, ir: &Ir, ctx: &mut Ctx, d: usize) {
        let ind = "    ".repeat(d);
        match ir {
            Ir::Let(x, rhs, rest) => {
                self.rhs(s, f, *x, rhs, ctx, d);
                self.ir(s, f, rest, ctx, d);
            }
            Ir::If(c, t, e) => {
                let _ = writeln!(s, "{}if ({}) {{", ind, self.var(f, *c));
                self.ir(s, f, t, ctx, d + 1);
                let _ = writeln!(s, "{}}} else {{", ind);
                self.ir(s, f, e, ctx, d + 1);
                let _ = writeln!(s, "{}}}", ind);
            }
            Ir::Chain(arms) => {
                for arm in arms {
                    let l = self.new_label();
                    ctx.fails.push(l.clone());
                    let _ = writeln!(s, "{}{{", ind);
                    self.ir(s, f, arm, ctx, d + 1);
                    let _ = writeln!(s, "{}}}", ind);
                    ctx.fails.pop();
                    let _ = writeln!(s, "{}:;", l);
                }
                let _ = writeln!(s, "{}rh_die(\"no arm of a match applies\");", ind);
            }
            Ir::Fail => {
                let _ = writeln!(s, "{}goto {};", ind, ctx.fails.last().unwrap());
            }
            Ir::Yield(v) => {
                let (dest, label) = ctx.blocks.last().unwrap().clone();
                let _ = writeln!(
                    s,
                    "{}{} = {}; goto {};",
                    ind,
                    self.var(f, dest),
                    self.var(f, *v),
                    label
                );
            }
            Ir::Ret(v) => {
                let _ = writeln!(s, "{}return {};", ind, self.var(f, *v));
            }
            Ir::TailCall(args) => {
                let _ = write!(s, "{}{{", ind);
                for (i, a) in args.iter().enumerate() {
                    let _ = write!(
                        s,
                        " {} t{} = {};",
                        self.cty(f.vars[*a].ty),
                        i,
                        self.var(f, *a)
                    );
                }
                for (i, p) in f.params.iter().enumerate() {
                    let _ = write!(s, " v{} = t{};", p, i);
                }
                let _ = writeln!(s, " }} goto top;");
            }
            Ir::Inc(v, rest) => {
                let _ = writeln!(s, "{}{}", ind, self.inc(f.vars[*v].ty, &self.var(f, *v)));
                self.ir(s, f, rest, ctx, d);
            }
            Ir::Dec(v, rest) => {
                let _ = writeln!(s, "{}{}", ind, self.dec(f.vars[*v].ty, &self.var(f, *v)));
                self.ir(s, f, rest, ctx, d);
            }
        }
    }

    fn lit(&mut self, text: &str) -> String {
        if let Some(&k) = self.lit_ids.get(text) {
            return format!("(Str *)&lit{}", k);
        }
        let k = self.lit_ids.len();
        self.lit_ids.insert(text.to_string(), k);
        let n = text.len();
        let _ = writeln!(
            self.lits,
            "static struct {{ int64_t rc, len, cap; char data[{}]; }} lit{} = {{ RH_IMMORTAL, {}, {}, {} }};",
            n + 1,
            k,
            n,
            n,
            lit_bytes(text)
        );
        format!("(Str *)&lit{}", k)
    }

    fn rhs(&mut self, s: &mut String, f: &IrFunc, x: VarId, rhs: &Rhs, ctx: &mut Ctx, d: usize) {
        let ind = "    ".repeat(d);
        let xs = self.var(f, x);
        let xt = f.vars[x].ty;
        let v = |v: VarId| self.var(f, v);
        let line = match rhs {
            Rhs::Int(n) => {
                if *n == i64::MIN {
                    format!("{} = INT64_MIN;", xs)
                } else {
                    format!("{} = INT64_C({});", xs, n)
                }
            }
            Rhs::Float(fl) => {
                let mut t = format!("{:?}", fl);
                if t == "inf" {
                    t = "INFINITY".into();
                } else if t == "-inf" {
                    t = "(-INFINITY)".into();
                } else if t == "NaN" {
                    t = "NAN".into();
                }
                format!("{} = {};", xs, t)
            }
            Rhs::Bool(b) => format!("{} = {};", xs, b),
            Rhs::Unit => format!("{} = 0;", xs),
            Rhs::Str(text) => {
                let l = self.lit(text);
                format!("{} = {};", xs, l)
            }
            Rhs::Prim(op, args) => {
                let t = f.vars[args[0]].ty;
                let a = v(args[0]);
                let b = args.get(1).map(|&y| v(y)).unwrap_or_default();
                let is_int = matches!(self.repr(t), Repr::Int);
                let e = match op {
                    Prim::Add if is_int => format!("rh_add({}, {})", a, b),
                    Prim::Sub if is_int => format!("rh_sub({}, {})", a, b),
                    Prim::Mul if is_int => format!("rh_mul({}, {})", a, b),
                    Prim::Div if is_int => format!("rh_div({}, {})", a, b),
                    Prim::Rem => format!("rh_rem({}, {})", a, b),
                    Prim::Neg if is_int => format!("rh_neg({})", a),
                    Prim::Add => format!("{} + {}", a, b),
                    Prim::Sub => format!("{} - {}", a, b),
                    Prim::Mul => format!("{} * {}", a, b),
                    Prim::Div => format!("{} / {}", a, b),
                    Prim::Neg => format!("-{}", a),
                    Prim::Not => format!("!{}", a),
                    Prim::Eq | Prim::Ne => {
                        let eq = match self.repr(t) {
                            Repr::Int | Repr::Float | Repr::Bool | Repr::Enum(..) => {
                                format!("({} == {})", a, b)
                            }
                            Repr::Unit => "true".into(),
                            _ => format!("eq{}({}, {})", t, a, b),
                        };
                        if *op == Prim::Eq {
                            eq
                        } else {
                            format!("!{}", eq)
                        }
                    }
                    Prim::Lt | Prim::Le | Prim::Gt | Prim::Ge => {
                        let o = match op {
                            Prim::Lt => "<",
                            Prim::Le => "<=",
                            Prim::Gt => ">",
                            _ => ">=",
                        };
                        if matches!(self.repr(t), Repr::Str) {
                            format!("rh_str_cmp({}, {}) {} 0", a, b, o)
                        } else {
                            format!("{} {} {}", a, o, b)
                        }
                    }
                    Prim::TagIs(k) => match self.repr(t) {
                        Repr::Enum(..) => format!("{} == {}", a, k),
                        _ => match self.tagless(t) {
                            Some(c) if c == *k as usize => format!("rh_is_cell({})", a),
                            Some(_) => format!("{} == (void *)(uintptr_t)({} * 2 + 1)", a, k),
                            None => format!("rh_tag({}) == {}", a, k),
                        },
                    },
                };
                format!("{} = {};", xs, e)
            }
            Rhs::Call(fid, args) => {
                let a: Vec<String> = args.iter().map(|&y| v(y)).collect();
                format!("{} = fn{}({});", xs, fid, a.join(", "))
            }
            Rhs::CallClo(c, args) => {
                let Repr::Clo(ps, r) = self.repr(f.vars[*c].ty).clone() else {
                    unreachable!()
                };
                let mut tys = vec!["Clo *".to_string()];
                tys.extend(ps.iter().map(|p| self.cty(*p)));
                let mut a = vec![v(*c)];
                a.extend(args.iter().map(|&y| v(y)));
                format!(
                    "{} = (({} (*)({})){}->fn)({});",
                    xs,
                    self.cty(r),
                    tys.join(", "),
                    v(*c),
                    a.join(", ")
                )
            }
            Rhs::MakeClo(fid, caps) => {
                if caps.is_empty() {
                    format!("{} = &sclo{};", xs, fid)
                } else {
                    let mut l = format!("{{ E{0} *e = rh_alloc(sizeof(E{0})); e->h.rc = 1; e->h.fn = (void *)fn{0}; e->h.drop = edrop{0};", fid);
                    for (i, c) in caps.iter().enumerate() {
                        let _ = write!(l, " e->c{} = {};", i, v(*c));
                    }
                    let _ = write!(l, " {} = (Clo *)e; }}", xs);
                    l
                }
            }
            Rhs::Ctor(tag, args) => match self.repr(xt) {
                Repr::Enum(..) => format!("{} = {};", xs, tag),
                Repr::Boxed(..) if args.is_empty() => {
                    format!("{} = (void *)(uintptr_t)({} * 2 + 1);", xs, tag)
                }
                Repr::Boxed(..) => {
                    let mut l = format!(
                        "{{ C{0}_{1} *c = rh_alloc(sizeof(C{0}_{1})); c->rc = 1;",
                        xt, tag
                    );
                    if self.tagless(xt).is_none() {
                        let _ = write!(l, " c->tag = {};", tag);
                    }
                    for (i, a) in args.iter().enumerate() {
                        let _ = write!(l, " c->f{} = {};", i, v(*a));
                    }
                    let _ = write!(l, " {} = c; }}", xs);
                    l
                }
                _ => unreachable!(),
            },
            Rhs::Struct(args) => {
                let a: Vec<String> = args.iter().map(|&y| v(y)).collect();
                if a.is_empty() {
                    format!("{} = (S{}){{0}};", xs, xt)
                } else {
                    format!("{} = (S{}){{ {} }};", xs, xt, a.join(", "))
                }
            }
            Rhs::Update(b, fs) => {
                let fts: Vec<TyId> = match self.repr(xt) {
                    Repr::Record(_, fs) => fs.iter().map(|(_, t)| *t).collect(),
                    Repr::Tuple(ts) => ts.clone(),
                    _ => unreachable!(),
                };
                let mut l = format!("{} = {};", xs, v(*b));
                for (i, y) in fs {
                    let _ = write!(
                        l,
                        " {} {}.f{} = {};",
                        self.dec(fts[*i], &format!("{}.f{}", xs, i)),
                        xs,
                        i,
                        v(*y)
                    );
                }
                l
            }
            Rhs::Peek(p, Peek::Field(i)) => format!("{} = {}.f{};", xs, v(*p), i),
            Rhs::Peek(p, Peek::CtorField(tag, i)) => {
                format!(
                    "{} = ((C{}_{} *){})->f{};",
                    xs,
                    f.vars[*p].ty,
                    tag,
                    v(*p),
                    i
                )
            }
            Rhs::Own(p, Peek::CtorField(tag, i)) => {
                format!(
                    "{} = ((C{}_{} *){})->f{};",
                    xs,
                    f.vars[*p].ty,
                    tag,
                    v(*p),
                    i
                )
            }
            Rhs::Own(p, Peek::Field(i)) => format!("{} = {}.f{};", xs, v(*p), i),
            Rhs::Release(c, fields) => {
                let mut incs = String::new();
                for &y in fields {
                    incs.push_str(&self.inc(f.vars[y].ty, &v(y)));
                    incs.push(' ');
                }
                format!(
                    "if (RH_LIKELY(((Cell *){0})->rc == 1)) free({0}); else {{ {1}((Cell *){0})->rc--; }} {2} = 0;",
                    v(*c),
                    incs,
                    xs
                )
            }
            Rhs::EnvLoad(i) => format!("{} = ((E{} *)env)->c{};", xs, ctx.fid, i),
            Rhs::Builtin(name, args) => self.builtin(f, x, name, args),
            Rhs::Block(body) => {
                let l = self.new_label();
                ctx.blocks.push((x, l.clone()));
                let _ = writeln!(s, "{}{{", ind);
                self.ir(s, f, body, ctx, d + 1);
                let _ = writeln!(s, "{}}}", ind);
                ctx.blocks.pop();
                let _ = writeln!(s, "{}:;", l);
                return;
            }
        };
        let _ = writeln!(s, "{}{}", ind, line);
    }

    fn builtin(&mut self, f: &IrFunc, x: VarId, name: &str, args: &[VarId]) -> String {
        let xs = self.var(f, x);
        let xt = f.vars[x].ty;
        let a: Vec<String> = args.iter().map(|&y| self.var(f, y)).collect();
        let at: Vec<TyId> = args.iter().map(|&y| f.vars[y].ty).collect();
        let is_str = |t: &TyId| matches!(self.repr(*t), Repr::Str);
        let set = |e: String| format!("{} = {};", xs, e);
        match name {
            "show" => {
                if is_str(&at[0]) {
                    format!("{} = {}; rh_str_inc({});", xs, a[0], a[0])
                } else {
                    set(format!("tostr{}({})", at[0], a[0]))
                }
            }
            "print" | "println" | "eprintln" => {
                let nl = name != "print";
                let stream = if name == "eprintln" {
                    "stderr"
                } else {
                    "stdout"
                };
                if is_str(&at[0]) {
                    format!("rh_print({}, {}, {}); {} = 0;", a[0], nl, stream, xs)
                } else {
                    format!(
                        "{{ Str *s = tostr{}({}); rh_print(s, {}, {}); rh_str_dec(s); }} {} = 0;",
                        at[0], a[0], nl, stream, xs
                    )
                }
            }
            "panic" => format!("rh_panic({});", a[0]),
            "exit" => format!("rh_exit({});", a[0]),
            "compare" => {
                if is_str(&at[0]) {
                    set(format!("rh_str_cmp({}, {})", a[0], a[1]))
                } else {
                    set(format!("({0} > {1}) - ({0} < {1})", a[0], a[1]))
                }
            }
            "clock" => set("rh_clock()".into()),
            "Int.to_float" => set(format!("(double){}", a[0])),
            "Int.bit_and" => set(format!("{} & {}", a[0], a[1])),
            "Int.bit_or" => set(format!("{} | {}", a[0], a[1])),
            "Int.bit_xor" => set(format!("{} ^ {}", a[0], a[1])),
            "Int.shl" => set(format!("rh_shl({}, {})", a[0], a[1])),
            "Int.shr" => set(format!("rh_shr({}, {})", a[0], a[1])),
            "Float.to_int" => set(format!("rh_f2i({})", a[0])),
            "Float.sqrt" | "Float.floor" | "Float.ceil" | "Float.round" | "Float.sin"
            | "Float.cos" | "Float.tan" | "Float.exp" | "Float.log" => {
                set(format!("{}({})", &name[6..], a[0]))
            }
            "Float.abs" => set(format!("fabs({})", a[0])),
            "Float.pow" => set(format!("pow({}, {})", a[0], a[1])),
            "Float.atan2" => set(format!("atan2({}, {})", a[0], a[1])),
            "Float.format" => set(format!("rh_float_format({}, {})", a[0], a[1])),
            "String.len" => set(format!("{}->len", a[0])),
            "String.byte" => set(format!("rh_str_byte({}, {})", a[0], a[1])),
            "String.slice" => set(format!("rh_str_slice({}, {}, {})", a[0], a[1], a[2])),
            "String.from_byte" => set(format!("rh_str_from_byte({})", a[0])),
            "String.trim" => set(format!("rh_str_trim({})", a[0])),
            "String.__find" => set(format!("rh_str_find({}, {}, {})", a[0], a[1], a[2])),
            "String.__is_int" => set(format!("rh_str_is_int({})", a[0])),
            "String.__to_int" => set(format!("rh_str_to_int({})", a[0])),
            "String.__is_float" => set(format!("rh_str_is_float({})", a[0])),
            "String.__to_float" => set(format!("rh_str_to_float({})", a[0])),
            "Array.make" => set(format!("amake{}({}, {})", xt, a[0], a[1])),
            "Array.empty" => set(format!("anew{}(0)", xt)),
            "Array.len" => set(format!("{}->len", a[0])),
            "Array.get" => set(format!("aget{}({}, {})", at[0], a[0], a[1])),
            "Array.set" => set(format!("aset{}({}, {}, {})", at[0], a[0], a[1], a[2])),
            "Array.push" => set(format!("apush{}({}, {})", at[0], a[0], a[1])),
            "Array.pop" => set(format!("apop{}({})", at[0], a[0])),
            "Array.slice" => set(format!("aslice{}({}, {}, {})", at[0], a[0], a[1], a[2])),
            "Array.swap" => set(format!("aswap{}({}, {}, {})", at[0], a[0], a[1], a[2])),
            "__array_lit" => {
                let mut l = format!("{0} = anew{1}({2}); {0}->len = {2};", xs, xt, a.len());
                for (i, y) in a.iter().enumerate() {
                    let _ = write!(l, " {}->data[{}] = {};", xs, i, y);
                }
                l
            }
            "++" => {
                if is_str(&at[0]) {
                    set(format!("rh_str_concat({}, {})", a[0], a[1]))
                } else {
                    set(format!("aconcat{}({}, {})", at[0], a[0], a[1]))
                }
            }
            "__argc" => set("rh_argc".into()),
            "__argv" => set(format!("rh_argv_at({})", a[0])),
            "__read_line" => set("rh_read_line()".into()),
            "__eof" => set("rh_at_eof".into()),
            "__read_file" => set(format!("rh_read_file({})", a[0])),
            "__io_ok" => set("rh_last_io_ok".into()),
            "write_file" => set(format!("rh_write_file({}, {})", a[0], a[1])),
            other => unreachable!("builtin {}", other),
        }
    }
}

struct Ctx {
    blocks: Vec<(VarId, String)>,
    fails: Vec<String>,
    fid: FnId,
}

fn has_tail(ir: &Ir) -> bool {
    match ir {
        Ir::TailCall(_) => true,
        Ir::Let(_, rhs, rest) => has_tail(rest) || matches!(rhs, Rhs::Block(b) if has_tail(b)),
        Ir::If(_, a, b) => has_tail(a) || has_tail(b),
        Ir::Chain(arms) => arms.iter().any(has_tail),
        Ir::Inc(_, r) | Ir::Dec(_, r) => has_tail(r),
        _ => false,
    }
}
