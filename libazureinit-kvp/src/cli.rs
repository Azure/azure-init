// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::{KvpError, KvpPool, KvpPoolStore, PoolMode};

const EXIT_OK: u8 = 0;
const EXIT_NOT_FOUND: u8 = 1;
const EXIT_USAGE_OR_VALIDATION: u8 = 2;
const EXIT_IO: u8 = 3;

/// Run the azure-init-kvp command-line interface.
pub fn run() -> ExitCode {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut stdout = stdout.lock();
    let mut stderr = stderr.lock();
    ExitCode::from(run_with_args(
        env::args_os().skip(1),
        &mut stdout,
        &mut stderr,
    ))
}

fn run_with_args<I, W, E>(args: I, stdout: &mut W, stderr: &mut E) -> u8
where
    I: IntoIterator<Item = OsString>,
    W: Write,
    E: Write,
{
    match try_run(args, stdout) {
        Ok(code) => code,
        Err(err) => {
            let _ = writeln!(stderr, "{err}");
            err.exit_code()
        }
    }
}

fn try_run<I, W>(args: I, stdout: &mut W) -> Result<u8, CliError>
where
    I: IntoIterator<Item = OsString>,
    W: Write,
{
    let invocation = Invocation::parse(args)?;
    if invocation.command == Command::Help {
        print_help(stdout)?;
        return Ok(EXIT_OK);
    }

    let store = match invocation.dir {
        Some(dir) => {
            KvpPoolStore::new_in(invocation.pool, dir, invocation.mode)
        }
        None => KvpPoolStore::new(invocation.pool, invocation.mode),
    }?;

    match invocation.command {
        Command::Info => info(&store, stdout),
        Command::Dump => dump(&store, stdout),
        Command::Entries => entries(&store, stdout),
        Command::Read { key } => read(&store, stdout, &key),
        Command::Write { key, value, append } => {
            if append {
                store.append(&key, &value)?;
            } else {
                store.insert(&key, &value)?;
            }
            Ok(EXIT_OK)
        }
        Command::Load { file } => load(&store, file),
        Command::Delete { key } => {
            writeln!(stdout, "{}", store.delete(&key)?)?;
            Ok(EXIT_OK)
        }
        Command::Clear { if_stale } => {
            if if_stale {
                store.clear_if_stale()?;
            } else {
                store.clear()?;
            }
            Ok(EXIT_OK)
        }
        Command::IsStale => {
            let stale = store.is_stale()?;
            writeln!(stdout, "{stale}")?;
            Ok(if stale { EXIT_OK } else { EXIT_NOT_FOUND })
        }
        Command::Help => unreachable!(),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Invocation {
    pool: KvpPool,
    dir: Option<PathBuf>,
    mode: PoolMode,
    command: Command,
}

impl Invocation {
    fn parse<I, S>(args: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let args: Vec<String> = args
            .into_iter()
            .map(|arg| {
                arg.into().into_string().map_err(|_| {
                    CliError::Usage("arguments must be valid UTF-8".to_string())
                })
            })
            .collect::<Result<_, _>>()?;

        let mut pool = KvpPool::Guest;
        let mut dir = None;
        let mut mode = PoolMode::Safe;
        let mut index = 0;

        while let Some(arg) = args.get(index) {
            match arg.as_str() {
                "-h" | "--help" => {
                    return Ok(Self {
                        pool,
                        dir,
                        mode,
                        command: Command::Help,
                    });
                }
                "--pool" => {
                    index += 1;
                    let value = args.get(index).ok_or_else(|| {
                        CliError::Usage("--pool requires a value".to_string())
                    })?;
                    pool = parse_pool(value)?;
                }
                "--dir" => {
                    index += 1;
                    let value = args.get(index).ok_or_else(|| {
                        CliError::Usage("--dir requires a value".to_string())
                    })?;
                    dir = Some(PathBuf::from(value));
                }
                "--unsafe" => mode = PoolMode::Unsafe,
                _ if arg.starts_with("--pool=") => {
                    pool = parse_pool(&arg[7..])?;
                }
                _ if arg.starts_with("--dir=") => {
                    dir = Some(PathBuf::from(&arg[6..]));
                }
                _ => break,
            }
            index += 1;
        }

        let command = Command::parse(&args[index..])?;
        Ok(Self {
            pool,
            dir,
            mode,
            command,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Info,
    Dump,
    Entries,
    Read {
        key: String,
    },
    Write {
        key: String,
        value: String,
        append: bool,
    },
    Load {
        file: Option<PathBuf>,
    },
    Delete {
        key: String,
    },
    Clear {
        if_stale: bool,
    },
    IsStale,
    Help,
}

impl Command {
    fn parse(args: &[String]) -> Result<Self, CliError> {
        let Some((command, rest)) = args.split_first() else {
            return Err(CliError::Usage("missing command".to_string()));
        };

        match command.as_str() {
            "info" => require_no_args(command, rest).map(|()| Self::Info),
            "dump" => require_no_args(command, rest).map(|()| Self::Dump),
            "entries" => require_no_args(command, rest).map(|()| Self::Entries),
            "read" => one_arg(command, rest).map(|key| Self::Read { key }),
            "write" => parse_write(rest),
            "load" => parse_load(rest),
            "delete" => one_arg(command, rest).map(|key| Self::Delete { key }),
            "clear" => parse_clear(rest),
            "is-stale" => {
                require_no_args(command, rest).map(|()| Self::IsStale)
            }
            "help" => require_no_args(command, rest).map(|()| Self::Help),
            _ => Err(CliError::Usage(format!("unknown command: {command}"))),
        }
    }
}

fn info<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
) -> Result<u8, CliError> {
    writeln!(stdout, "pool={}", pool_name(store.pool()))?;
    writeln!(stdout, "path={}", store.path().display())?;
    writeln!(stdout, "mode={}", mode_name(store.mode()))?;
    writeln!(stdout, "records={}", store.len()?)?;
    writeln!(stdout, "empty={}", store.is_empty()?)?;
    writeln!(stdout, "stale={}", store.is_stale()?)?;
    writeln!(stdout, "max_key_size={}", store.max_key_size())?;
    writeln!(stdout, "max_value_size={}", store.max_value_size())?;
    Ok(EXIT_OK)
}

fn dump<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
) -> Result<u8, CliError> {
    for (key, value) in store.dump()? {
        writeln!(stdout, "{key}={value}")?;
    }
    Ok(EXIT_OK)
}

fn entries<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
) -> Result<u8, CliError> {
    let mut entries: Vec<_> = store.entries()?.into_iter().collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (key, value) in entries {
        writeln!(stdout, "{key}={value}")?;
    }
    Ok(EXIT_OK)
}

fn read<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    key: &str,
) -> Result<u8, CliError> {
    match store.read(key)? {
        Some(value) => {
            writeln!(stdout, "{value}")?;
            Ok(EXIT_OK)
        }
        None => Ok(EXIT_NOT_FOUND),
    }
}

fn load(store: &KvpPoolStore, file: Option<PathBuf>) -> Result<u8, CliError> {
    let input = match file {
        Some(path) => fs::read_to_string(path)?,
        None => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            input
        }
    };
    store.populate(parse_key_value_lines(&input)?)?;
    Ok(EXIT_OK)
}

fn parse_key_value_lines(
    input: &str,
) -> Result<Vec<(String, String)>, CliError> {
    let mut records = Vec::new();
    for (line_number, line) in input.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            CliError::Usage(format!(
                "line {} must be in key=value format",
                line_number + 1
            ))
        })?;
        records.push((key.to_string(), value.to_string()));
    }
    Ok(records)
}

fn parse_write(args: &[String]) -> Result<Command, CliError> {
    let mut append = false;
    let mut values = Vec::new();

    for arg in args {
        if arg == "--append" {
            append = true;
        } else {
            values.push(arg.clone());
        }
    }

    match values.as_slice() {
        [key, value] => Ok(Command::Write {
            key: key.clone(),
            value: value.clone(),
            append,
        }),
        _ => Err(CliError::Usage(
            "usage: azure-init-kvp write [--append] <KEY> <VALUE>".to_string(),
        )),
    }
}

fn parse_load(args: &[String]) -> Result<Command, CliError> {
    let mut file = None;
    let mut index = 0;

    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--file" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    CliError::Usage("--file requires a value".to_string())
                })?;
                file = Some(PathBuf::from(value));
            }
            _ if arg.starts_with("--file=") => {
                file = Some(PathBuf::from(&arg[7..]));
            }
            _ => {
                return Err(CliError::Usage(
                    "usage: azure-init-kvp load [--file PATH]".to_string(),
                ));
            }
        }
        index += 1;
    }

    Ok(Command::Load { file })
}

fn parse_clear(args: &[String]) -> Result<Command, CliError> {
    let mut if_stale = false;
    let mut yes = false;

    for arg in args {
        match arg.as_str() {
            "--if-stale" => if_stale = true,
            "--yes" => yes = true,
            _ => {
                return Err(CliError::Usage(
                    "usage: azure-init-kvp clear [--if-stale] [--yes]"
                        .to_string(),
                ));
            }
        }
    }

    if !if_stale && !yes {
        return Err(CliError::Usage(
            "clear requires --yes unless --if-stale is set".to_string(),
        ));
    }

    Ok(Command::Clear { if_stale })
}

fn one_arg(command: &str, args: &[String]) -> Result<String, CliError> {
    match args {
        [value] => Ok(value.clone()),
        _ => Err(CliError::Usage(format!(
            "usage: azure-init-kvp {command} <KEY>"
        ))),
    }
}

fn require_no_args(command: &str, args: &[String]) -> Result<(), CliError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(CliError::Usage(format!("usage: azure-init-kvp {command}")))
    }
}

fn parse_pool(value: &str) -> Result<KvpPool, CliError> {
    match value {
        "external" | "0" => Ok(KvpPool::External),
        "guest" | "1" => Ok(KvpPool::Guest),
        "auto" | "2" => Ok(KvpPool::Auto),
        "auto-external" | "3" => Ok(KvpPool::AutoExternal),
        "auto-internal" | "4" => Ok(KvpPool::AutoInternal),
        _ => Err(CliError::Usage(format!(
            "unknown pool {value}; expected external, guest, auto, auto-external, or auto-internal"
        ))),
    }
}

fn pool_name(pool: KvpPool) -> &'static str {
    match pool {
        KvpPool::External => "external",
        KvpPool::Guest => "guest",
        KvpPool::Auto => "auto",
        KvpPool::AutoExternal => "auto-external",
        KvpPool::AutoInternal => "auto-internal",
    }
}

fn mode_name(mode: PoolMode) -> &'static str {
    match mode {
        PoolMode::Safe => "safe",
        PoolMode::Unsafe => "unsafe",
    }
}

fn print_help<W: Write>(stdout: &mut W) -> Result<(), CliError> {
    writeln!(stdout, "usage: azure-init-kvp [OPTIONS] <COMMAND>")?;
    writeln!(stdout)?;
    writeln!(stdout, "options:")?;
    writeln!(
        stdout,
        "  --pool <POOL>       external|guest|auto|auto-external|auto-internal"
    )?;
    writeln!(stdout, "  --dir <PATH>        KVP pool directory")?;
    writeln!(
        stdout,
        "  --unsafe           use full wire-format key/value limits"
    )?;
    writeln!(stdout, "  -h, --help          print help")?;
    writeln!(stdout)?;
    writeln!(stdout, "commands:")?;
    writeln!(stdout, "  info")?;
    writeln!(stdout, "  dump")?;
    writeln!(stdout, "  entries")?;
    writeln!(stdout, "  read <KEY>")?;
    writeln!(stdout, "  write [--append] <KEY> <VALUE>")?;
    writeln!(stdout, "  load [--file PATH]")?;
    writeln!(stdout, "  delete <KEY>")?;
    writeln!(stdout, "  clear [--if-stale] [--yes]")?;
    writeln!(stdout, "  is-stale")?;
    Ok(())
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Kvp(KvpError),
    Io(io::Error),
}

impl CliError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => EXIT_USAGE_OR_VALIDATION,
            Self::Kvp(KvpError::Io(_)) | Self::Io(_) => EXIT_IO,
            Self::Kvp(_) => EXIT_USAGE_OR_VALIDATION,
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) => write!(f, "{message}"),
            Self::Kvp(err) => write!(f, "{err}"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl From<KvpError> for CliError {
    fn from(err: KvpError) -> Self {
        Self::Kvp(err)
    }
}

impl From<io::Error> for CliError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_defaults_to_guest_safe() {
        let invocation = Invocation::parse(["info"]).unwrap();
        assert_eq!(invocation.pool, KvpPool::Guest);
        assert_eq!(invocation.dir, None);
        assert_eq!(invocation.mode, PoolMode::Safe);
        assert_eq!(invocation.command, Command::Info);
    }

    #[test]
    fn parse_globals() {
        let invocation = Invocation::parse([
            "--pool=auto-external",
            "--dir",
            "/tmp/kvp",
            "--unsafe",
            "dump",
        ])
        .unwrap();
        assert_eq!(invocation.pool, KvpPool::AutoExternal);
        assert_eq!(invocation.dir, Some(PathBuf::from("/tmp/kvp")));
        assert_eq!(invocation.mode, PoolMode::Unsafe);
        assert_eq!(invocation.command, Command::Dump);
    }

    #[test]
    fn parse_write_append() {
        let invocation =
            Invocation::parse(["write", "--append", "k", "v"]).unwrap();
        assert_eq!(
            invocation.command,
            Command::Write {
                key: "k".to_string(),
                value: "v".to_string(),
                append: true,
            }
        );
    }

    #[test]
    fn parse_key_value_lines_rejects_missing_separator() {
        let err = parse_key_value_lines("a=1\ninvalid\n").unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE_OR_VALIDATION);
        assert_eq!(err.to_string(), "line 2 must be in key=value format");
    }
}
