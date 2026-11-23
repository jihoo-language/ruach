# Ruach

*Ruach* (רוּחַ) is the breath that moves over the waters of *tohu wa-bohu*. [Tohu](https://github.com/jihoo-language/Tohu), [Aporia](https://github.com/jihoo-language/aporia) and [ASL](https://github.com/jihoo-language/asl) are hard on purpose. Ruach is for writing ordinary programs:

- **Functional.** Programs are built from variant types, records, pattern matching, closures and `|>` pipelines. Values never change: an update returns a new value.
- **Inferred.** Hindley–Milner type inference, with polymorphism for top-level functions. Annotations are optional, and a type variable in a signature is checked to be general.
- **As fast as C.** The compiler writes C and builds it with `cc -O2`. Generic functions are compiled once for each type they are used at (as in [jihoo](https://github.com/jihoo-language/jihoo)). `Int` and `Float` are never boxed, and a function calling itself in tail position becomes a loop.
- **No garbage collector.** Memory is managed by reference counts, which the compiler inserts by liveness in the style of Perceus. A value is freed right after its last use, and its last use hands the reference on instead of copying it. An array that only one place refers to is therefore updated **in place**, so functional array code runs like imperative C.
- **Safe at run time.** Array indexing is bounds-checked, integer arithmetic wraps, and division by zero stops the program with a message. Every `match` is checked to be exhaustive.

```ruach
type Shape =
  | Circle(Float)
  | Rect(Float, Float)

fn Shape.area(s: Shape) -> Float =
  match s {
    Circle(r) => 3.14159 * r * r
    Rect(w, h) => w * h
  }

fn main() {
  let shapes = [Circle(1.0), Rect(3.0, 4.0)]
  println(shapes.map(fn(s) => s.area()))                 // [3.14159, 12.0]
  let squares = range(1, 11)
    |> Array.filter(fn(n) => n % 2 == 0)
    |> Array.map(fn(n) => n * n)
  println(squares.sum())                                 // 220
  println("12".to_int().map(fn(n) => n + 1))             // Some(13)
}
```

## Run a program

```sh
nix run path:. -- run examples/tour.rh
nix run path:. -- run examples/calc.rh -- "let x = 4 in x * (x + 1)"
nix run path:. -- run examples/words.rh < README.md
```

```text
let x = 4 in x * (x + 1) = 20.0
```

`ruach` has four commands:

```sh
ruach run   prog.rh [-- args...]          # compile, run, and pass on the exit code
ruach build prog.rh -o prog [-O3]         # an executable
ruach check prog.rh                       # type errors only
ruach emit-c prog.rh                      # the C it writes
```

The C compiler is `$RUACH_CC`, or `cc` if that is unset. `$RUACH_CFLAGS` adds flags, for example `-fsanitize=address`. The Nix package sets the compiler for you.

To develop Ruach itself, use the flake's shell. It has Rust and a C compiler.

```sh
nix develop -c cargo test
nix develop -c cargo run -q -- run examples/hello.rh
```

## Types

| Type | Values | In C |
| --- | --- | --- |
| `Int` | 64-bit integers that wrap; `'a'` is the byte 97 | `int64_t` |
| `Float` | 64-bit floating point | `double` |
| `Bool`, `()` | `true`, `false`, `()` | `bool`, a byte |
| `String` | immutable bytes, usually UTF-8 | counted pointer |
| `Array[a]` | `[1, 2, 3]`, indexed by `a[i]` | counted pointer |
| `(a, b)` | tuples, read with `t.0` or a pattern | struct, by value |
| `(a, b) -> c` | functions and closures | counted pointer |
| `type P = { x: Float, y: Float }` | records: `P { x: 1.0, y: 2.0 }`, `P { y: 0.0, ..p }` | struct, by value |
| `type T[a] = A \| B(a, T[a])` | variants | counted pointer to a cell; `A` is a small integer |

A variant type whose constructors have no fields, such as `type Color = Red | Green`, is a plain integer. Type names and constructors are capitalized, and type variables are lowercase. The prelude defines `Option[a]` (`None`, `Some`), `Result[a, e]` (`Ok`, `Err`), `List[a]` (`Nil`, `Cons`) and `Map[k, v]`, a persistent sorted map.

## Expressions

```ruach
fn gcd(a: Int, b: Int) -> Int = if b == 0 { a } else { gcd(b, a % b) }

fn describe(n) =                         // the type, (Int) -> String, is inferred
  match n {
    0 => "zero"
    n if n < 0 => "negative"
    _ => "positive"
  }

fn main() {
  let (q, r) = (17 / 5, 17 % 5)          // patterns in `let`
  let add = fn(x, y) => x + y            // closures capture what they use
  let total = [1, 2, 3].fold(0, add)
  println(describe(total) ++ " " ++ show(gcd(q, r)))
}
```

- A block `{ ... }` is a sequence of `let`s and expressions separated by new lines or `;`. Its value is the last expression, or `()`.
- `if` and `match` are expressions. An `if` without `else` has type `()`.
- Patterns are `_`, names, literals, constructors `Some(x)`, tuples `(a, b)` and records `P { x, y: py, .. }`. Arms can have guards (`n if n < 0`). A `match` that misses a case is an error that names the missing value, and so is an arm that can never match.
- `x |> f(a)` is `f(x, a)`, and `x |> f` is `f(x)`.
- `e.name(args)` is a method call. It finds `Type.name` from the type of `e`, then a field of `e` that holds a function, then a top-level function `name`. Define methods with `fn Type.name(self_value: Type, ...)`, and call them by their full name too: `Array.map(xs, f)`.
- `++` joins strings and arrays. `==` compares any values without functions structurally. `<` compares numbers, strings and booleans.
- `import "other.rh"` loads a file relative to the importing file, once.

Operators are checked for each type a generic function is used at, as templates are. `fn add(x, y) = x + y` works for both `Int` and `Float`, and `add(true, false)` is rejected where it is used. A method call needs the receiver's type to be known at that point: in `fn f(xs) = xs.len()` it is not, so write `Array.len(xs)` or annotate `xs`.

## Memory and speed

Every heap value (a string, an array, a closure or a variant cell) has a reference count. The compiler knows where each variable is last used:

- At the last use, the reference moves into its new owner, with no increment and no decrement.
- A value that nothing will use again is released at once, not at the end of a scope.
- Reading a field or matching a pattern borrows from the value it reads, instead of counting a new reference.
- `match xs { Cons(x, rest) => ... }` on a list that nothing else refers to frees the cell and takes over its fields.
- `a.set(i, x)`, `a.push(x)`, `s ++ t` and `a.swap(i, j)` change their first argument in place when nothing else refers to it. Otherwise they copy it first. So `let b = a.set(0, 1)` never changes `a` while `a` is still used, and a loop that keeps only the new array writes in place every time.

Reference counting cannot free cycles, but Ruach values cannot form cycles, because nothing is ever mutated. Strings and arrays double their capacity when they grow, so building a string with `acc ++ piece` in a loop takes linear time. The program runs on a 1 GiB stack, so ordinary non-tail recursion over long lists works. The tests run every example under AddressSanitizer and LeakSanitizer.

The C versions in [benchmarks/](benchmarks) are written the usual imperative way. Run `python3 benchmarks/run.py`. Results on an i7-12700 with GCC 16, best of three runs:

| Benchmark | Ruach | C `-O2` | Ratio |
| --- | ---: | ---: | ---: |
| `fib`: naive Fibonacci of 42 | 0.30 s | 0.29 s | 1.04 |
| `sieve`: primes below 50 million, 5 times | 1.16 s | 1.14 s | 1.01 |
| `nbody`: 5 bodies, 50 million steps, with records | 2.04 s | 1.78 s | 1.15 |
| `trees`: binary trees of depth 20, allocated and freed | 5.32 s | 4.74 s | 1.12 |
| `sort`: quicksort of 5 million integers with a closure, against `qsort` | 0.55 s | 0.56 s | 0.99 |

## The prelude

The [prelude](src/prelude.rh) is written in Ruach and is a good place to read idiomatic code.

| Type | Methods |
| --- | --- |
| `Array[a]` | `len get set push pop slice swap map filter fold each sum any all find contains reverse zip enumerate flat_map sort sort_by first last is_empty join to_list`, and `Array.make(n, x)`, `Array.init(n, f)`, `Array.empty()`, `range(a, b)` |
| `String` | `len byte slice find contains starts_with ends_with split lines words trim repeat pad_left pad_right to_int to_float bytes is_empty`, and `String.from_byte(n)` |
| `Int`, `Float` | `abs min max to_string to_float to_int sqrt floor ceil round sin cos tan exp log pow atan2 format(digits)`, and `bit_and bit_or bit_xor shl shr` |
| `Option`, `Result` | `map and_then unwrap unwrap_or is_some is_none is_ok` |
| `List[a]` | `len map filter fold each reverse to_array is_empty` |
| `Map[k, v]` | `Map.empty() Map.from_array(pairs) get insert update remove contains len keys values to_array fold is_empty` |
| everything | `show(x)`, `print(x)`, `println(x)`, `eprintln(x)`, `compare(a, b)`, `panic(msg)`, `assert(ok, msg)`, `exit(code)`, `clock()` |
| input | `read_line() -> Option[String]`, `read_file(path) -> Option[String]`, `write_file(path, text) -> Bool`, `args() -> Array[String]` |

`show` gives a string unchanged. Inside other values, strings are quoted: `show(Some("a"))` is `Some("a")`.

## How it compiles

| Stage | File |
| --- | --- |
| Tokens; a newline before a token ends a statement where an expression could end | [lexer.rs](src/lexer.rs) |
| Syntax tree; `a[i]` and `\|>` are rewritten into calls | [parser.rs](src/parser.rs) |
| Inference, method resolution, exhaustiveness | [types.rs](src/types.rs) |
| Monomorphization to an intermediate language in which every value is a variable | [lower.rs](src/lower.rs), [ir.rs](src/ir.rs) |
| Reference counts from liveness | [rc.rs](src/rc.rs) |
| C, with counting, equality and printing functions for each type | [emit.rs](src/emit.rs), [runtime.c](src/runtime.c) |

## Examples

| File | Shows |
| --- | --- |
| [tour.rh](examples/tour.rh) | the language in one page |
| [calc.rh](examples/calc.rh) | a tokenizer, a recursive-descent parser and an evaluator, with errors as `Result` |
| [words.rh](examples/words.rh) | reading standard input and counting words in a `Map` |
| [queens.rh](examples/queens.rh) | backtracking over immutable lists |
| [rejected/](examples/rejected) | programs that do not compile, each with the error it gets |

## Limits

Version 0.1. These features are missing:

- loops other than recursion
- local recursive functions
- mutable variables
- or-patterns
- modules with private names
- threads
- polymorphic local `let`s (top-level functions are polymorphic)

`sort` is not stable. Generated programs need a C compiler and run on Linux or macOS.
