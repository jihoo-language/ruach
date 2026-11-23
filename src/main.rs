use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const USAGE: &str = "usage:
  ruach run <file.rh | -> [-- args...]   compile and run
  ruach build <file.rh> [-o out] [-O0|-O1|-O2|-O3]
  ruach check <file.rh>                  type-check only
  ruach emit-c <file.rh>                 print the generated C";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match real_main(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn read(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| e.to_string())?;
        Ok(s)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))
    }
}

fn real_main(args: Vec<String>) -> Result<ExitCode, String> {
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r.to_vec()),
        None => {
            eprintln!("{}", USAGE);
            return Ok(ExitCode::from(2));
        }
    };
    let (cmd, rest) = if cmd.ends_with(".rh") || cmd == "-" {
        ("run", args.clone())
    } else {
        (cmd, rest)
    };
    match cmd {
        "run" => {
            let (opts, prog_args) = match rest.iter().position(|a| a == "--") {
                Some(i) => (rest[..i].to_vec(), rest[i + 1..].to_vec()),
                None => (rest.clone(), vec![]),
            };
            let file = opts
                .iter()
                .find(|a| !a.starts_with("-O"))
                .ok_or(USAGE)?
                .clone();
            let opt = opts
                .iter()
                .find(|a| a.starts_with("-O"))
                .cloned()
                .unwrap_or("-O2".into());
            let src = read(&file)?;
            let c = ruach::compile_to_c(&file, &src).map_err(|e| e.to_string())?;
            let dir = ruach::scratch_dir().map_err(|e| e.to_string())?;
            let exe = dir.join("program");
            ruach::build_c(&c, &exe, &opt).map_err(|e| e.to_string())?;
            let status = Command::new(&exe)
                .args(&prog_args)
                .status()
                .map_err(|e| e.to_string());
            let _ = std::fs::remove_dir_all(&dir);
            let status = status?;
            Ok(match status.code() {
                Some(c) => ExitCode::from(c as u8),
                None => ExitCode::FAILURE,
            })
        }
        "build" => {
            let mut file = None;
            let mut out = None;
            let mut opt = "-O2".to_string();
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                if a == "-o" {
                    out = Some(it.next().ok_or(USAGE)?.clone());
                } else if a.starts_with("-O") {
                    opt = a.clone();
                } else {
                    file = Some(a.clone());
                }
            }
            let file = file.ok_or(USAGE)?;
            let out = out.unwrap_or_else(|| {
                PathBuf::from(&file)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or("a.out".into())
            });
            let src = read(&file)?;
            let c = ruach::compile_to_c(&file, &src).map_err(|e| e.to_string())?;
            ruach::build_c(&c, &PathBuf::from(out), &opt).map_err(|e| e.to_string())?;
            Ok(ExitCode::SUCCESS)
        }
        "check" => {
            let file = rest.first().ok_or(USAGE)?;
            let src = read(file)?;
            ruach::check(file, &src).map_err(|e| e.to_string())?;
            Ok(ExitCode::SUCCESS)
        }
        "emit-c" => {
            let file = rest.first().ok_or(USAGE)?;
            let src = read(file)?;
            let c = ruach::compile_to_c(file, &src).map_err(|e| e.to_string())?;
            print!("{}", c);
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => {
            println!("{}", USAGE);
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(format!("unknown command `{}`\n{}", cmd, USAGE)),
    }
}
