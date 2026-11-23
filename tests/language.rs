//! End-to-end tests: each program is compiled to C, built with the C
//! compiler and run.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

fn build(path: &Path, src: &str, cflags: &str) -> Result<PathBuf, String> {
    let c = ruach::compile_to_c(&path.display().to_string(), src).map_err(|e| e.to_string())?;
    let dir = ruach::scratch_dir().map_err(|e| e.to_string())?;
    let exe = dir.join("program");
    if cflags.is_empty() {
        ruach::build_c(&c, &exe, "-O2").map_err(|e| e.to_string())?;
    } else {
        // Sanitizer builds pass their flags straight to the C compiler.
        let cfile = dir.join("main.c");
        std::fs::write(&cfile, &c).unwrap();
        let out = Command::new(ruach::c_compiler())
            .args(cflags.split_whitespace())
            .args(["-O1", "-w", "-std=gnu11", "-o"])
            .arg(&exe)
            .arg(&cfile)
            .arg("-lm")
            .output()
            .unwrap();
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).to_string());
        }
    }
    Ok(exe)
}

fn run_with(path: &Path, src: &str, stdin: &str, args: &[&str], cflags: &str) -> Run {
    let exe = build(path, src, cflags).unwrap_or_else(|e| panic!("{}: {}", path.display(), e));
    let mut child = Command::new(&exe)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(exe.parent().unwrap());
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        code: out.status.code(),
    }
}

fn run_file(rel: &str) -> Run {
    let path = root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap();
    run_with(&path, &src, "", &[], "")
}

fn run_src(src: &str) -> Run {
    run_with(&root().join("inline.rh"), src, "", &[], "")
}

fn compile_error(src: &str) -> String {
    match ruach::check("inline.rh", src) {
        Ok(()) => panic!("expected an error for:\n{}", src),
        Err(e) => e.to_string(),
    }
}

/// Every `tests/run/NAME.rh` prints `tests/run/NAME.out`.
#[test]
fn programs_print_their_expected_output() {
    let dir = root().join("tests/run");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "rh"))
        .collect();
    names.sort();
    assert!(names.len() >= 5);
    for p in names {
        let rel = p.strip_prefix(root()).unwrap().display().to_string();
        let expected = std::fs::read_to_string(p.with_extension("out")).unwrap();
        let r = run_file(&rel);
        assert_eq!(r.code, Some(0), "{} failed: {}", rel, r.stderr);
        assert_eq!(r.stdout, expected, "{}", rel);
    }
}

#[test]
fn hello() {
    assert_eq!(run_file("examples/hello.rh").stdout, "Hello, Ruach!\n");
}

#[test]
fn tour() {
    let out = run_file("examples/tour.rh").stdout;
    assert!(out.starts_with("Circle(1.0) has area 3.142\n"));
    assert!(out.contains("Item { name: \"honey\", price: 1080, qty: 1 }\n"));
    assert!(out.contains("\n1540\n63\nabababab\n21\n111\n"));
    assert!(out.ends_with("hi, Ada; hi, Alan\n"));
}

#[test]
fn calculator() {
    let out = run_file("examples/calc.rh").stdout;
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "1 + 2 * 3 = 7.0");
    assert_eq!(lines[2], "let x = 4 in x * (x + 1) = 20.0");
    assert_eq!(lines[5], "1 / (3 - 3): error: division by zero");
    assert_eq!(lines[8], "3 $ 4: error: unexpected `$` at 2");
    let path = root().join("examples/calc.rh");
    let src = std::fs::read_to_string(&path).unwrap();
    let r = run_with(&path, &src, "", &["2 * (3 + 4)"], "");
    assert_eq!(r.stdout, "2 * (3 + 4) = 14.0\n");
}

#[test]
fn queens() {
    let out = run_file("examples/queens.rh").stdout;
    assert!(out.starts_with("4 queens: 2\n5 queens: 10\n6 queens: 4\n7 queens: 40\n8 queens: 92\n"));
    assert!(out.contains("10 queens: 724\nQ . . . . . . .\n"));
}

#[test]
fn word_counts_read_standard_input() {
    let path = root().join("examples/words.rh");
    let src = std::fs::read_to_string(&path).unwrap();
    let r = run_with(
        &path,
        &src,
        "The cat and the hat.\nThe CAT sat; the dog did not.\n",
        &[],
        "",
    );
    assert_eq!(
        r.stdout,
        "12 words, 8 different\n     4  the\n     2  cat\n     1  and\n     1  did\n     1  dog\n     1  hat\n     1  not\n     1  sat\n"
    );
}

/// Each file in `examples/rejected` starts with `// error: <message>`.
#[test]
fn rejected_programs_explain_why() {
    let dir = root().join("examples/rejected");
    let mut n = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        let src = std::fs::read_to_string(&p).unwrap();
        let want = src
            .lines()
            .next()
            .unwrap()
            .strip_prefix("// error: ")
            .expect("first line names the error");
        let err = ruach::check(&p.display().to_string(), &src)
            .expect_err(&p.display().to_string())
            .to_string();
        assert!(
            err.contains(want),
            "{}: got `{}`, want `{}`",
            p.display(),
            err,
            want
        );
        n += 1;
    }
    assert!(n >= 8);
}

#[test]
fn errors_point_at_the_problem() {
    assert_eq!(
        compile_error("fn main() {\n  let x = 1 + \"a\"\n}"),
        "inline.rh:2:15: expected Int, found String"
    );
    assert!(compile_error("fn main() { undefined_thing() }")
        .contains("`undefined_thing` is not defined"));
    assert!(compile_error("fn f(x) = x.len()\nfn main() {}").contains("must be known here"));
    assert!(compile_error("fn main() { println(1 < 2 < 3) }").contains("do not chain"));
    assert!(compile_error("fn main() { let Some(x) = None }").contains("must always match"));
    assert!(compile_error("type T = A | B\ntype U = A\nfn main() {}").contains("already defined"));
    assert!(compile_error("fn f() = 1\nfn f() = 2\nfn main() {}").contains("already defined"));
    assert!(compile_error("fn main(x: Int) {}").contains("takes no arguments"));
    assert!(compile_error("type R = { r: R }\nfn main() {}").contains("contains itself"));
    assert!(compile_error("fn main() { if 1 { 2 } }").contains("expected Bool, found Int"));
    assert!(compile_error("fn main() { let x = if true { 1 } }")
        .contains("an `if` without `else` gives ()"));
    assert!(
        compile_error("fn swap(x: a, y: b) -> (b, a) = (x, x)\nfn main() {}")
            .contains("`a` and `b` must be the same")
    );
}

#[test]
fn runtime_errors_stop_the_program() {
    let r = run_src("fn main() { let a = [1, 2, 3]\n println(a[3]) }");
    assert_eq!(r.code, Some(101));
    assert_eq!(r.stderr, "panic: index 3 is out of bounds for length 3\n");
    let r = run_src("fn main() { println(\"before\")\n let z = 0\n println(10 / z) }");
    assert_eq!(r.code, Some(101));
    assert_eq!(r.stdout, "before\n");
    assert_eq!(r.stderr, "panic: division by zero\n");
    let r = run_src("fn main() { let x: Int = None.unwrap()\n println(x) }");
    assert_eq!(r.stderr, "panic: unwrap of None\n");
    let r = run_src("fn main() { println(\"bye\")\n exit(3) }");
    assert_eq!((r.code, r.stdout.as_str()), (Some(3), "bye\n"));
    let r = run_src("fn main() { assert(1 + 1 == 3, \"arithmetic\") }");
    assert_eq!(r.stderr, "panic: assertion failed: arithmetic\n");
}

#[test]
fn integers_wrap_and_divide_toward_zero() {
    let r = run_src(
        "fn main() {\n println(9223372036854775807 + 1)\n println(-7 / 2)\n println(-7 % 2)\n println((-9223372036854775807 - 1) / -1)\n println(Int.shl(1, 62))\n println(Int.bit_xor(12, 10)) }",
    );
    assert_eq!(
        r.stdout,
        "-9223372036854775808\n-3\n-1\n-9223372036854775808\n4611686018427387904\n6\n"
    );
}

#[test]
fn floats_print_the_shortest_exact_text() {
    let r = run_src("fn main() { println([20.0, 0.1, 1e20, 123456.75, -3.0, 100.0 / 3.0, 1.0 / 0.0, 0.1 + 0.2]) }");
    assert_eq!(
        r.stdout,
        "[20.0, 0.1, 1e+20, 123456.75, -3.0, 33.333333333333336, inf, 0.30000000000000004]\n"
    );
}

#[test]
fn imports_load_other_files_once() {
    let dir = ruach::scratch_dir().unwrap();
    std::fs::write(
        dir.join("geometry.rh"),
        "import \"util.rh\"\nfn area(w: Float, h: Float) -> Float = double(w * h) / 2.0\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("util.rh"),
        "fn double(x: Float) -> Float = x * 2.0\n",
    )
    .unwrap();
    let main = dir.join("main.rh");
    let src = "import \"geometry.rh\"\nimport \"util.rh\"\nfn main() { println(area(3.0, 4.0) + double(1.0)) }\n";
    std::fs::write(&main, src).unwrap();
    let r = run_with(&main, src, "", &[], "");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(r.stdout, "14.0\n");
}

#[test]
fn self_tail_calls_do_not_grow_the_stack() {
    let r = run_src("fn count(n: Int, acc: Int) -> Int = if n == 0 { acc } else { count(n - 1, acc + 1) }\nfn main() { println(count(100_000_000, 0)) }");
    assert_eq!(r.stdout, "100000000\n");
    let c = ruach::compile_to_c("t.rh", "fn count(n: Int, acc: Int) -> Int = if n == 0 { acc } else { count(n - 1, acc + 1) }\nfn main() { println(count(3, 0)) }").unwrap();
    assert!(c.contains("goto top;"));
}

/// The generated C must be free of leaks, double frees and reads of freed
/// memory. Skipped where AddressSanitizer cannot run.
#[test]
fn reference_counting_is_exact() {
    // Sanitizers need support from the compiler and the environment (a
    // build sandbox may forbid what LeakSanitizer does at exit).
    let probe = ruach::scratch_dir().unwrap();
    let ok = std::fs::write(
        probe.join("p.c"),
        "#include <stdlib.h>\nint main(void){free(malloc(8));return 0;}",
    )
    .is_ok()
        && Command::new(ruach::c_compiler())
            .args(["-fsanitize=address,undefined", "-o"])
            .arg(probe.join("p"))
            .arg(probe.join("p.c"))
            .output()
            .is_ok_and(|o| o.status.success())
        && Command::new(probe.join("p"))
            .output()
            .is_ok_and(|o| o.status.success() && o.stderr.is_empty());
    let _ = std::fs::remove_dir_all(&probe);
    if !ok {
        eprintln!("skipped: AddressSanitizer is not available here");
        return;
    }
    for rel in [
        "tests/run/basics.rh",
        "tests/run/features.rh",
        "tests/run/strings.rh",
        "tests/run/closures.rh",
        "tests/run/records.rh",
        "tests/run/arrays.rh",
        "tests/run/maps.rh",
        "examples/tour.rh",
        "examples/calc.rh",
        "examples/queens.rh",
    ] {
        let path = root().join(rel);
        let src = std::fs::read_to_string(&path).unwrap();
        let r = run_with(
            &path,
            &src,
            "",
            &[],
            "-fsanitize=address,undefined -fno-omit-frame-pointer",
        );
        let report: Vec<&str> = r
            .stderr
            .lines()
            .filter(|l| !l.contains("makecontext/swapcontext"))
            .collect();
        assert!(
            report.is_empty() && r.code == Some(0),
            "{}:\n{}",
            rel,
            report.join("\n")
        );
    }
    let path = root().join("examples/words.rh");
    let src = std::fs::read_to_string(&path).unwrap();
    let readme = std::fs::read_to_string(root().join("README.md")).unwrap_or_default();
    let r = run_with(&path, &src, &readme, &[], "-fsanitize=address,undefined");
    let report: Vec<&str> = r
        .stderr
        .lines()
        .filter(|l| !l.contains("makecontext/swapcontext"))
        .collect();
    assert!(report.is_empty(), "words.rh:\n{}", report.join("\n"));
}
