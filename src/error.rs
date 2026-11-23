use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Pos {
    pub file: u32,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug)]
pub struct Error {
    pub pos: Option<Pos>,
    pub msg: String,
    /// The file name of `pos`, filled in by the driver.
    pub file: Option<String>,
}

impl Error {
    pub fn at(pos: Pos, msg: impl Into<String>) -> Error {
        Error {
            pos: Some(pos),
            msg: msg.into(),
            file: None,
        }
    }

    pub fn new(msg: impl Into<String>) -> Error {
        Error {
            pos: None,
            msg: msg.into(),
            file: None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match (&self.file, self.pos) {
            (Some(file), Some(p)) => write!(f, "{}:{}:{}: {}", file, p.line, p.col, self.msg),
            (None, Some(p)) => write!(f, "{}:{}: {}", p.line, p.col, self.msg),
            _ => write!(f, "{}", self.msg),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
