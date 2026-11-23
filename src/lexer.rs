//! Tokens. Newlines are not tokens; each token records whether a newline
//! came before it, and the parser uses that to end statements and arms.

use crate::error::{Error, Pos};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    UIdent(String),
    // keywords
    Fn,
    Let,
    If,
    Else,
    Match,
    Type,
    True,
    False,
    Import,
    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semi,
    Dot,
    DotDot,
    FatArrow,
    Arrow,
    Eq,
    EqEq,
    NotEq,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    PlusPlus,
    Minus,
    Star,
    Slash,
    Percent,
    AndAnd,
    OrOr,
    Bang,
    Pipe,
    Bar,
    Underscore,
    Eof,
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Int(n) => format!("`{}`", n),
            Tok::Float(f) => format!("`{}`", f),
            Tok::Str(_) => "a string".into(),
            Tok::Ident(s) | Tok::UIdent(s) => format!("`{}`", s),
            Tok::Eof => "the end of the file".into(),
            t => format!("`{}`", t.text()),
        }
    }

    fn text(&self) -> &'static str {
        match self {
            Tok::Fn => "fn",
            Tok::Let => "let",
            Tok::If => "if",
            Tok::Else => "else",
            Tok::Match => "match",
            Tok::Type => "type",
            Tok::True => "true",
            Tok::False => "false",
            Tok::Import => "import",
            Tok::LParen => "(",
            Tok::RParen => ")",
            Tok::LBrace => "{",
            Tok::RBrace => "}",
            Tok::LBracket => "[",
            Tok::RBracket => "]",
            Tok::Comma => ",",
            Tok::Colon => ":",
            Tok::Semi => ";",
            Tok::Dot => ".",
            Tok::DotDot => "..",
            Tok::FatArrow => "=>",
            Tok::Arrow => "->",
            Tok::Eq => "=",
            Tok::EqEq => "==",
            Tok::NotEq => "!=",
            Tok::Lt => "<",
            Tok::Le => "<=",
            Tok::Gt => ">",
            Tok::Ge => ">=",
            Tok::Plus => "+",
            Tok::PlusPlus => "++",
            Tok::Minus => "-",
            Tok::Star => "*",
            Tok::Slash => "/",
            Tok::Percent => "%",
            Tok::AndAnd => "&&",
            Tok::OrOr => "||",
            Tok::Bang => "!",
            Tok::Pipe => "|>",
            Tok::Bar => "|",
            Tok::Underscore => "_",
            _ => "?",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub pos: Pos,
    /// A newline separates this token from the previous one.
    pub nl: bool,
}

pub fn lex(src: &str, file: u32) -> Result<Vec<Token>, Error> {
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut line = 1u32;
    let mut col = 1u32;
    let mut nl = true;
    let mut out = Vec::new();
    macro_rules! adv {
        () => {{
            if bytes[i] == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
        }};
    }
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            nl = true;
            adv!();
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            adv!();
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                adv!();
            }
            continue;
        }
        let pos = Pos { file, line, col };
        let start = i;
        let tok = if c.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                adv!();
            }
            let mut is_float = false;
            if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
                is_float = true;
                adv!();
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
                    adv!();
                }
            }
            if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                let save = (i, line, col);
                adv!();
                if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                    adv!();
                }
                if i < bytes.len() && bytes[i].is_ascii_digit() {
                    is_float = true;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        adv!();
                    }
                } else {
                    (i, line, col) = save;
                }
            }
            let text: String = src[start..i].chars().filter(|&c| c != '_').collect();
            if is_float {
                Tok::Float(
                    text.parse()
                        .map_err(|_| Error::at(pos, "malformed number"))?,
                )
            } else {
                Tok::Int(text.parse().map_err(|_| {
                    Error::at(
                        pos,
                        format!("integer literal `{}` does not fit in 64 bits", text),
                    )
                })?)
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                adv!();
            }
            let w = &src[start..i];
            match w {
                "fn" => Tok::Fn,
                "let" => Tok::Let,
                "if" => Tok::If,
                "else" => Tok::Else,
                "match" => Tok::Match,
                "type" => Tok::Type,
                "true" => Tok::True,
                "false" => Tok::False,
                "import" => Tok::Import,
                "_" => Tok::Underscore,
                _ if c.is_ascii_uppercase() => Tok::UIdent(w.to_string()),
                _ => Tok::Ident(w.to_string()),
            }
        } else if c == b'"' {
            adv!();
            let mut s = Vec::new();
            loop {
                if i >= bytes.len() {
                    return Err(Error::at(pos, "unterminated string"));
                }
                let b = bytes[i];
                if b == b'"' {
                    adv!();
                    break;
                }
                if b == b'\\' {
                    adv!();
                    if i >= bytes.len() {
                        return Err(Error::at(pos, "unterminated string"));
                    }
                    s.push(escape(bytes[i], Pos { file, line, col })?);
                    adv!();
                } else {
                    s.push(b);
                    adv!();
                }
            }
            Tok::Str(String::from_utf8(s).map_err(|_| Error::at(pos, "string is not UTF-8"))?)
        } else if c == b'\'' {
            // A character literal is the integer value of one byte.
            adv!();
            if i >= bytes.len() {
                return Err(Error::at(pos, "unterminated character"));
            }
            let v = if bytes[i] == b'\\' {
                adv!();
                if i >= bytes.len() {
                    return Err(Error::at(pos, "unterminated character"));
                }
                let e = escape(bytes[i], Pos { file, line, col })?;
                adv!();
                e
            } else {
                let b = bytes[i];
                if b >= 0x80 {
                    return Err(Error::at(pos, "a character literal is one ASCII byte"));
                }
                adv!();
                b
            };
            if i >= bytes.len() || bytes[i] != b'\'' {
                return Err(Error::at(pos, "a character literal is one ASCII byte"));
            }
            adv!();
            Tok::Int(v as i64)
        } else {
            let two = if i + 1 < bytes.len() {
                &src[i..i + 2]
            } else {
                ""
            };
            let t2 = match two {
                "=>" => Some(Tok::FatArrow),
                "->" => Some(Tok::Arrow),
                "==" => Some(Tok::EqEq),
                "!=" => Some(Tok::NotEq),
                "<=" => Some(Tok::Le),
                ">=" => Some(Tok::Ge),
                "++" => Some(Tok::PlusPlus),
                "&&" => Some(Tok::AndAnd),
                "||" => Some(Tok::OrOr),
                "|>" => Some(Tok::Pipe),
                ".." => Some(Tok::DotDot),
                _ => None,
            };
            if let Some(t) = t2 {
                adv!();
                adv!();
                t
            } else {
                let t = match c {
                    b'(' => Tok::LParen,
                    b')' => Tok::RParen,
                    b'{' => Tok::LBrace,
                    b'}' => Tok::RBrace,
                    b'[' => Tok::LBracket,
                    b']' => Tok::RBracket,
                    b',' => Tok::Comma,
                    b':' => Tok::Colon,
                    b';' => Tok::Semi,
                    b'.' => Tok::Dot,
                    b'=' => Tok::Eq,
                    b'<' => Tok::Lt,
                    b'>' => Tok::Gt,
                    b'+' => Tok::Plus,
                    b'-' => Tok::Minus,
                    b'*' => Tok::Star,
                    b'/' => Tok::Slash,
                    b'%' => Tok::Percent,
                    b'!' => Tok::Bang,
                    b'|' => Tok::Bar,
                    _ => {
                        let ch = src[i..].chars().next().unwrap();
                        return Err(Error::at(pos, format!("unexpected character `{}`", ch)));
                    }
                };
                adv!();
                t
            }
        };
        out.push(Token { tok, pos, nl });
        nl = false;
    }
    out.push(Token {
        tok: Tok::Eof,
        pos: Pos { file, line, col },
        nl: true,
    });
    Ok(out)
}

fn escape(b: u8, pos: Pos) -> Result<u8, Error> {
    Ok(match b {
        b'n' => b'\n',
        b't' => b'\t',
        b'r' => b'\r',
        b'0' => 0,
        b'\\' => b'\\',
        b'"' => b'"',
        b'\'' => b'\'',
        _ => return Err(Error::at(pos, format!("unknown escape `\\{}`", b as char))),
    })
}
