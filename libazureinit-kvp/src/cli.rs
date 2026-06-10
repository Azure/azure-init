// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use crate::{KvpError, KvpPool, KvpPoolStore, PoolMode};

const EXIT_OK: u8 = 0;
const EXIT_NOT_FOUND: u8 = 1;
const EXIT_USAGE_OR_VALIDATION: u8 = 2;
const EXIT_IO: u8 = 3;

/// Entry point for the `libazureinit-kvp` binary.
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut stdout = stdout.lock();
    let mut stderr = stderr.lock();

    let code = match dispatch(cli, &mut stdout) {
        Ok(code) => code,
        Err(err) => {
            let _ = writeln!(stderr, "{err}");
            err.exit_code()
        }
    };
    ExitCode::from(code)
}

#[derive(Parser, Debug)]
#[command(
    name = "libazureinit-kvp",
    about = "Inspect and manipulate Hyper-V KVP pool files."
)]
struct Cli {
    /// KVP pool to operate on.
    #[arg(long, value_enum, default_value_t = PoolArg::Guest, global = true)]
    pool: PoolArg,

    /// KVP pool directory (defaults to /var/lib/hyperv).
    #[arg(long, global = true)]
    dir: Option<PathBuf>,

    /// Use full wire-format key/value limits instead of the safe profile.
    #[arg(long = "unsafe", global = true)]
    unsafe_mode: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print store metadata.
    Info,
    /// Print every record in insertion order as KEY=VALUE lines.
    Dump,
    /// Print key=last_value entries sorted by key.
    Entries,
    /// Print the last value for KEY (exit 1 if missing).
    Read { key: String },
    /// Write a record. Use --append to keep prior values for KEY.
    Write {
        /// Append a new value instead of replacing prior records for KEY.
        #[arg(long)]
        append: bool,
        key: String,
        value: String,
    },
    /// Replace the pool from KEY=VALUE lines read from --file or stdin.
    Load {
        /// Read records from PATH instead of stdin.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Delete every record with KEY (prints `true` if any were removed).
    Delete { key: String },
    /// Clear the pool. Requires --yes unless --if-stale is set.
    Clear {
        /// Only clear if the store is currently stale.
        #[arg(long = "if-stale")]
        if_stale: bool,
        /// Confirm unconditional clear.
        #[arg(long)]
        yes: bool,
    },
    /// Print whether the pool is stale (exit 0 if stale, 1 otherwise).
    IsStale,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
enum PoolArg {
    #[value(alias = "0")]
    External,
    #[value(alias = "1")]
    Guest,
    #[value(alias = "2")]
    Auto,
    #[value(alias = "3")]
    AutoExternal,
    #[value(alias = "4")]
    AutoInternal,
}

impl From<PoolArg> for KvpPool {
    fn from(value: PoolArg) -> Self {
        match value {
            PoolArg::External => KvpPool::External,
            PoolArg::Guest => KvpPool::Guest,
            PoolArg::Auto => KvpPool::Auto,
            PoolArg::AutoExternal => KvpPool::AutoExternal,
            PoolArg::AutoInternal => KvpPool::AutoInternal,
        }
    }
}

fn dispatch<W: Write>(cli: Cli, stdout: &mut W) -> Result<u8, CliError> {
    let pool: KvpPool = cli.pool.into();
    let mode = if cli.unsafe_mode {
        PoolMode::Unsafe
    } else {
        PoolMode::Safe
    };

    let store = match cli.dir {
        Some(dir) => KvpPoolStore::new_in(pool, dir, mode),
        None => KvpPoolStore::new(pool, mode),
    }
    .expect("KvpPoolStore construction is currently infallible");

    match cli.command {
        Command::Info => info(&store, stdout),
        Command::Dump => dump(&store, stdout),
        Command::Entries => entries(&store, stdout),
        Command::Read { key } => read(&store, stdout, &key),
        Command::Write { append, key, value } => {
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
        Command::Clear { if_stale, yes } => {
            if if_stale {
                store.clear_if_stale()?;
            } else if yes {
                store.clear()?;
            } else {
                return Err(CliError::Usage(
                    "clear requires --yes unless --if-stale is set".to_string(),
                ));
            }
            Ok(EXIT_OK)
        }
        Command::IsStale => {
            let stale = store.is_stale()?;
            writeln!(stdout, "{stale}")?;
            Ok(if stale { EXIT_OK } else { EXIT_NOT_FOUND })
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
    match file {
        Some(path) => load_from_reader(store, fs::File::open(path)?),
        None => load_from_reader(store, io::stdin().lock()),
    }
}

fn load_from_reader<R: Read>(
    store: &KvpPoolStore,
    mut reader: R,
) -> Result<u8, CliError> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    store.populate(parse_key_value_lines(&buf)?)?;
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
    use std::ffi::CString;
    use std::io::Cursor;
    use std::path::Path;

    use clap::CommandFactory;
    use tempfile::TempDir;

    fn cli(dir: &TempDir, command: Command) -> Cli {
        Cli {
            pool: PoolArg::Guest,
            dir: Some(dir.path().to_path_buf()),
            unsafe_mode: false,
            command,
        }
    }

    fn store_at(dir: &TempDir) -> KvpPoolStore {
        KvpPoolStore::new_in(
            KvpPool::Guest,
            dir.path().to_path_buf(),
            PoolMode::Safe,
        )
        .unwrap()
    }

    fn run_dispatch(cli: Cli) -> (u8, String) {
        let mut out = Vec::new();
        let code = dispatch(cli, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    fn set_mtime_to_epoch(path: &Path) {
        let c_path = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let times = [libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        }; 2];
        assert_eq!(unsafe { libc::utimes(c_path.as_ptr(), times.as_ptr()) }, 0);
    }

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_defaults_to_guest_safe() {
        let cli = Cli::parse_from(["libazureinit-kvp", "info"]);
        assert!(matches!(cli.pool, PoolArg::Guest));
        assert_eq!(cli.dir, None);
        assert!(!cli.unsafe_mode);
        assert!(matches!(cli.command, Command::Info));
    }

    #[test]
    fn parse_globals_and_subcommand() {
        let cli = Cli::parse_from([
            "libazureinit-kvp",
            "--pool=auto-external",
            "--dir",
            "/tmp/kvp",
            "--unsafe",
            "dump",
        ]);
        assert!(matches!(cli.pool, PoolArg::AutoExternal));
        assert_eq!(cli.dir, Some(PathBuf::from("/tmp/kvp")));
        assert!(cli.unsafe_mode);
        assert!(matches!(cli.command, Command::Dump));
    }

    #[test]
    fn parse_pool_numeric_aliases() {
        let cases = [
            ("0", PoolArg::External),
            ("1", PoolArg::Guest),
            ("2", PoolArg::Auto),
            ("3", PoolArg::AutoExternal),
            ("4", PoolArg::AutoInternal),
        ];

        for (alias, expected) in cases {
            let cli =
                Cli::parse_from(["libazureinit-kvp", "--pool", alias, "info"]);
            assert_eq!(cli.pool, expected);
        }
    }

    #[test]
    fn parse_write_append() {
        let cli = Cli::parse_from([
            "libazureinit-kvp",
            "write",
            "--append",
            "k",
            "v",
        ]);
        assert!(matches!(
            cli.command,
            Command::Write { append: true, ref key, ref value }
                if key == "k" && value == "v"
        ));
    }

    #[test]
    fn pool_arg_into_kvp_pool_covers_every_variant() {
        assert_eq!(KvpPool::from(PoolArg::External), KvpPool::External);
        assert_eq!(KvpPool::from(PoolArg::Guest), KvpPool::Guest);
        assert_eq!(KvpPool::from(PoolArg::Auto), KvpPool::Auto);
        assert_eq!(KvpPool::from(PoolArg::AutoExternal), KvpPool::AutoExternal);
        assert_eq!(KvpPool::from(PoolArg::AutoInternal), KvpPool::AutoInternal);
    }

    #[test]
    fn pool_name_covers_every_variant() {
        assert_eq!(pool_name(KvpPool::External), "external");
        assert_eq!(pool_name(KvpPool::Guest), "guest");
        assert_eq!(pool_name(KvpPool::Auto), "auto");
        assert_eq!(pool_name(KvpPool::AutoExternal), "auto-external");
        assert_eq!(pool_name(KvpPool::AutoInternal), "auto-internal");
    }

    #[test]
    fn mode_name_covers_both_variants() {
        assert_eq!(mode_name(PoolMode::Safe), "safe");
        assert_eq!(mode_name(PoolMode::Unsafe), "unsafe");
    }

    #[test]
    fn dispatch_info_reports_metadata() {
        let dir = TempDir::new().unwrap();
        let (code, out) = run_dispatch(cli(&dir, Command::Info));
        assert_eq!(code, EXIT_OK);
        assert!(out.contains("pool=guest"));
        assert!(out.contains("mode=safe"));
        assert!(out.contains("records=0"));
        assert!(out.contains("empty=true"));
        assert!(out.contains("stale=false"));
        assert!(out.contains("max_key_size="));
        assert!(out.contains("max_value_size="));
    }

    #[test]
    fn dispatch_info_with_unsafe_mode_picks_unsafe_profile() {
        let dir = TempDir::new().unwrap();
        let invocation = Cli {
            pool: PoolArg::External,
            dir: Some(dir.path().to_path_buf()),
            unsafe_mode: true,
            command: Command::Info,
        };
        let (code, out) = run_dispatch(invocation);
        assert_eq!(code, EXIT_OK);
        assert!(out.contains("pool=external"));
        assert!(out.contains("mode=unsafe"));
    }

    #[test]
    fn dispatch_write_then_read() {
        let dir = TempDir::new().unwrap();
        let (code, _) = run_dispatch(cli(
            &dir,
            Command::Write {
                append: false,
                key: "k".into(),
                value: "v".into(),
            },
        ));
        assert_eq!(code, EXIT_OK);

        let (code, out) =
            run_dispatch(cli(&dir, Command::Read { key: "k".into() }));
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, "v\n");
    }

    #[test]
    fn dispatch_write_append_keeps_prior_values() {
        let dir = TempDir::new().unwrap();
        run_dispatch(cli(
            &dir,
            Command::Write {
                append: false,
                key: "k".into(),
                value: "1".into(),
            },
        ));
        run_dispatch(cli(
            &dir,
            Command::Write {
                append: true,
                key: "k".into(),
                value: "2".into(),
            },
        ));

        let (_, dumped) = run_dispatch(cli(&dir, Command::Dump));
        assert_eq!(dumped, "k=1\nk=2\n");
    }

    #[test]
    fn dispatch_read_missing_returns_not_found() {
        let dir = TempDir::new().unwrap();
        let (code, out) = run_dispatch(cli(
            &dir,
            Command::Read {
                key: "missing".into(),
            },
        ));
        assert_eq!(code, EXIT_NOT_FOUND);
        assert!(out.is_empty());
    }

    #[test]
    fn dispatch_dump_and_entries_emit_records() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        store.insert("b", "two").unwrap();
        store.insert("a", "one").unwrap();

        let (_, dumped) = run_dispatch(cli(&dir, Command::Dump));
        assert!(dumped.contains("a=one"));
        assert!(dumped.contains("b=two"));

        let (_, entries) = run_dispatch(cli(&dir, Command::Entries));
        // entries are sorted by key
        assert_eq!(entries, "a=one\nb=two\n");
    }

    #[test]
    fn dispatch_delete_prints_removed_flag() {
        let dir = TempDir::new().unwrap();
        store_at(&dir).insert("k", "v").unwrap();

        let (code, out) =
            run_dispatch(cli(&dir, Command::Delete { key: "k".into() }));
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, "true\n");

        let (code, out) = run_dispatch(cli(
            &dir,
            Command::Delete {
                key: "missing".into(),
            },
        ));
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, "false\n");
    }

    #[test]
    fn dispatch_clear_yes_branch_empties_store() {
        let dir = TempDir::new().unwrap();
        store_at(&dir).insert("k", "v").unwrap();

        let (code, _) = run_dispatch(cli(
            &dir,
            Command::Clear {
                if_stale: false,
                yes: true,
            },
        ));
        assert_eq!(code, EXIT_OK);
        assert!(store_at(&dir).is_empty().unwrap());
    }

    #[test]
    fn dispatch_clear_if_stale_branch_succeeds_on_fresh_store() {
        let dir = TempDir::new().unwrap();
        let (code, _) = run_dispatch(cli(
            &dir,
            Command::Clear {
                if_stale: true,
                yes: false,
            },
        ));
        assert_eq!(code, EXIT_OK);
    }

    #[test]
    fn dispatch_clear_without_flags_returns_usage_error() {
        let dir = TempDir::new().unwrap();
        let invocation = cli(
            &dir,
            Command::Clear {
                if_stale: false,
                yes: false,
            },
        );
        let mut out = Vec::new();
        let err = dispatch(invocation, &mut out).unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE_OR_VALIDATION);
        assert!(err.to_string().contains("clear requires --yes"));
    }

    #[test]
    fn dispatch_is_stale_returns_not_found_when_fresh() {
        let dir = TempDir::new().unwrap();
        let (code, out) = run_dispatch(cli(&dir, Command::IsStale));
        assert_eq!(code, EXIT_NOT_FOUND);
        assert_eq!(out, "false\n");
    }

    #[test]
    fn dispatch_is_stale_returns_ok_when_pool_predates_boot() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        store.insert("k", "v").unwrap();
        set_mtime_to_epoch(store.path());

        let (code, out) = run_dispatch(cli(&dir, Command::IsStale));
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, "true\n");
    }

    #[test]
    fn dispatch_load_from_file_replaces_pool() {
        let dir = TempDir::new().unwrap();
        let input = dir.path().join("records.txt");
        std::fs::write(&input, "a=1\nb=2\n").unwrap();

        let (code, _) =
            run_dispatch(cli(&dir, Command::Load { file: Some(input) }));
        assert_eq!(code, EXIT_OK);
        let (_, entries) = run_dispatch(cli(&dir, Command::Entries));
        assert_eq!(entries, "a=1\nb=2\n");
    }

    #[test]
    fn dispatch_propagates_validation_error_from_write() {
        let dir = TempDir::new().unwrap();
        let invocation = cli(
            &dir,
            Command::Write {
                append: false,
                key: String::new(),
                value: "v".into(),
            },
        );
        let mut out = Vec::new();
        let err = dispatch(invocation, &mut out).unwrap_err();
        assert!(matches!(err, CliError::Kvp(KvpError::EmptyKey)));
        assert_eq!(err.exit_code(), EXIT_USAGE_OR_VALIDATION);
    }

    #[test]
    fn dispatch_store_io_error_propagates() {
        // Putting a regular file where the store expects a directory
        // makes the kernel return ENOTDIR for any path beneath it,
        // which propagates as KvpError::Io.
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not-a-directory").unwrap();

        let invocation = Cli {
            pool: PoolArg::Guest,
            dir: Some(blocker),
            unsafe_mode: false,
            command: Command::Info,
        };
        let mut out = Vec::new();
        let err = dispatch(invocation, &mut out).unwrap_err();
        assert!(matches!(err, CliError::Kvp(KvpError::Io(_))));
        assert_eq!(err.exit_code(), EXIT_IO);
    }

    #[test]
    fn load_from_reader_parses_key_value_records() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        let code = load_from_reader(&store, Cursor::new("x=1\ny=2\n")).unwrap();
        assert_eq!(code, EXIT_OK);
        assert_eq!(store.read("x").unwrap().as_deref(), Some("1"));
        assert_eq!(store.read("y").unwrap().as_deref(), Some("2"));
    }

    #[test]
    fn load_from_reader_surfaces_parse_errors() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        let err =
            load_from_reader(&store, Cursor::new("bad-line\n")).unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
    }

    #[test]
    fn load_from_path_surfaces_io_errors() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        let err =
            load(&store, Some(dir.path().join("does-not-exist"))).unwrap_err();
        assert!(matches!(err, CliError::Io(_)));
        assert_eq!(err.exit_code(), EXIT_IO);
    }

    #[test]
    fn parse_key_value_lines_accepts_empty_lines_and_pairs() {
        let parsed = parse_key_value_lines("a=1\n\nb=2\n").unwrap();
        assert_eq!(
            parsed,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
            ]
        );
    }

    #[test]
    fn parse_key_value_lines_rejects_missing_separator() {
        let err = parse_key_value_lines("a=1\ninvalid\n").unwrap_err();
        assert_eq!(err.exit_code(), EXIT_USAGE_OR_VALIDATION);
        assert_eq!(err.to_string(), "line 2 must be in key=value format");
    }

    #[test]
    fn cli_error_display_covers_every_variant() {
        let usage = CliError::Usage("u".to_string());
        assert_eq!(usage.to_string(), "u");
        assert_eq!(usage.exit_code(), EXIT_USAGE_OR_VALIDATION);

        let kvp_validation = CliError::Kvp(KvpError::EmptyKey);
        assert_eq!(kvp_validation.to_string(), "KVP key must not be empty");
        assert_eq!(kvp_validation.exit_code(), EXIT_USAGE_OR_VALIDATION);

        let kvp_io = CliError::Kvp(KvpError::Io(io::Error::new(
            io::ErrorKind::Other,
            "kvp-io",
        )));
        assert!(kvp_io.to_string().contains("kvp-io"));
        assert_eq!(kvp_io.exit_code(), EXIT_IO);

        let io_err =
            CliError::Io(io::Error::new(io::ErrorKind::Other, "raw-io"));
        assert_eq!(io_err.to_string(), "raw-io");
        assert_eq!(io_err.exit_code(), EXIT_IO);
    }

    #[test]
    fn cli_error_from_conversions() {
        let from_kvp: CliError = KvpError::EmptyKey.into();
        assert!(matches!(from_kvp, CliError::Kvp(_)));

        let from_io: CliError =
            io::Error::new(io::ErrorKind::Other, "boom").into();
        assert!(matches!(from_io, CliError::Io(_)));
    }
}
