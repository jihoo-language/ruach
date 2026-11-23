//! The functions the compiler implements itself, with their types.
//! Lowercase names in a signature are type parameters.

pub const BUILTINS: &[(&str, &str)] = &[
    ("show", "(a) -> String"),
    ("print", "(a) -> ()"),
    ("println", "(a) -> ()"),
    ("eprintln", "(a) -> ()"),
    ("panic", "(String) -> a"),
    ("exit", "(Int) -> a"),
    ("compare", "(a, a) -> Int"),
    ("clock", "() -> Float"),
    ("Int.to_float", "(Int) -> Float"),
    ("Int.bit_and", "(Int, Int) -> Int"),
    ("Int.bit_or", "(Int, Int) -> Int"),
    ("Int.bit_xor", "(Int, Int) -> Int"),
    ("Int.shl", "(Int, Int) -> Int"),
    ("Int.shr", "(Int, Int) -> Int"),
    ("Float.to_int", "(Float) -> Int"),
    ("Float.sqrt", "(Float) -> Float"),
    ("Float.floor", "(Float) -> Float"),
    ("Float.ceil", "(Float) -> Float"),
    ("Float.round", "(Float) -> Float"),
    ("Float.abs", "(Float) -> Float"),
    ("Float.sin", "(Float) -> Float"),
    ("Float.cos", "(Float) -> Float"),
    ("Float.tan", "(Float) -> Float"),
    ("Float.exp", "(Float) -> Float"),
    ("Float.log", "(Float) -> Float"),
    ("Float.pow", "(Float, Float) -> Float"),
    ("Float.atan2", "(Float, Float) -> Float"),
    ("Float.format", "(Float, Int) -> String"),
    ("String.len", "(String) -> Int"),
    ("String.byte", "(String, Int) -> Int"),
    ("String.slice", "(String, Int, Int) -> String"),
    ("String.from_byte", "(Int) -> String"),
    ("String.trim", "(String) -> String"),
    ("String.__find", "(String, String, Int) -> Int"),
    ("String.__is_int", "(String) -> Bool"),
    ("String.__to_int", "(String) -> Int"),
    ("String.__is_float", "(String) -> Bool"),
    ("String.__to_float", "(String) -> Float"),
    ("Array.make", "(Int, a) -> Array[a]"),
    ("Array.empty", "() -> Array[a]"),
    ("Array.len", "(Array[a]) -> Int"),
    ("Array.get", "(Array[a], Int) -> a"),
    ("Array.set", "(Array[a], Int, a) -> Array[a]"),
    ("Array.push", "(Array[a], a) -> Array[a]"),
    ("Array.pop", "(Array[a]) -> Array[a]"),
    ("Array.slice", "(Array[a], Int, Int) -> Array[a]"),
    ("Array.swap", "(Array[a], Int, Int) -> Array[a]"),
    ("__argc", "() -> Int"),
    ("__argv", "(Int) -> String"),
    ("__read_line", "() -> String"),
    ("__eof", "() -> Bool"),
    ("__read_file", "(String) -> String"),
    ("__io_ok", "() -> Bool"),
    ("write_file", "(String, String) -> Bool"),
];

/// How a builtin treats each argument: `true` takes ownership.
pub fn owns_args(name: &str) -> &'static [bool] {
    match name {
        "Array.make" => &[false, true],
        "Array.set" => &[true, false, true],
        "Array.push" => &[true, true],
        "Array.pop" => &[true],
        "Array.swap" => &[true, false, false],
        _ => &[],
    }
}

/// Builtins that never return.
pub fn diverges(name: &str) -> bool {
    matches!(name, "panic" | "exit")
}
