// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use crate::{
    write_report, KvpError, KvpPool, KvpPoolStore, PoolMode,
    ProvisioningReport, ReportPpsType,
};

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

    /// Emit machine-readable JSON for commands that produce output.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

/// Output format for commands that print data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputMode {
    Text,
    Json,
}

impl OutputMode {
    fn from_flag(json: bool) -> Self {
        if json {
            Self::Json
        } else {
            Self::Text
        }
    }
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
    /// Append KEY=VALUE lines read from --file or stdin.
    AppendMultiple {
        /// Read records from PATH instead of stdin.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Delete every record with KEY (prints `true` if any were removed).
    Delete { key: String },
    /// Delete every record matching any KEY (prints removed record count).
    DeleteMultiple {
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Clear the pool. Pass --if-stale to clear only when stale.
    Clear {
        /// Only clear if the store is currently stale.
        #[arg(long = "if-stale")]
        if_stale: bool,
    },
    /// Print whether the pool is stale (exit 0 if stale, 1 otherwise).
    IsStale,
    /// Write a success provisioning health report, overriding any existing
    /// `PROVISIONING_REPORT` record.
    ReportSuccess {
        /// Virtual machine identifier.
        #[arg(long)]
        vm_id: String,
        /// Reporting agent identifier (e.g. Azure-Init/1.2.3).
        #[arg(long)]
        agent: String,
        /// Optional human-readable message attached as an extra field.
        #[arg(long)]
        message: Option<String>,
    },
    /// Write a failure provisioning health report, overriding any existing
    /// `PROVISIONING_REPORT` record.
    ReportFailure {
        /// Virtual machine identifier.
        #[arg(long)]
        vm_id: String,
        /// Reporting agent identifier (e.g. Azure-Init/1.2.3).
        #[arg(long)]
        agent: String,
        /// Failure reason.
        #[arg(long)]
        reason: String,
        /// Optional documentation URL describing the failure.
        #[arg(long)]
        documentation_url: Option<String>,
    },
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
    let output = OutputMode::from_flag(cli.json);

    let store = match cli.dir {
        Some(dir) => KvpPoolStore::new_in(pool, dir, mode),
        None => KvpPoolStore::new(pool, mode),
    }
    .expect("KvpPoolStore construction is currently infallible");

    match cli.command {
        Command::Info => info(&store, stdout, output),
        Command::Dump => dump(&store, stdout, output),
        Command::Entries => entries(&store, stdout, output),
        Command::Read { key } => read(&store, stdout, &key, output),
        Command::Write { append, key, value } => {
            if append {
                store.append(&key, &value)?;
            } else {
                store.insert(&key, &value)?;
            }
            Ok(EXIT_OK)
        }
        Command::Load { file } => load(&store, file),
        Command::AppendMultiple { file } => append_multiple(&store, file),
        Command::Delete { key } => delete(&store, stdout, &key, output),
        Command::DeleteMultiple { keys } => {
            delete_multiple(&store, stdout, keys, output)
        }
        Command::Clear { if_stale } => {
            if if_stale {
                store.clear_if_stale()?;
            } else {
                store.clear()?;
            }
            Ok(EXIT_OK)
        }
        Command::IsStale => is_stale(&store, stdout, output),
        Command::ReportSuccess {
            vm_id,
            agent,
            message,
        } => report_success(&store, vm_id, agent, message),
        Command::ReportFailure {
            vm_id,
            agent,
            reason,
            documentation_url,
        } => report_failure(&store, vm_id, agent, reason, documentation_url),
    }
}

fn info<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    output: OutputMode,
) -> Result<u8, CliError> {
    let pool = pool_name(store.pool());
    let path = store.path().display().to_string();
    let mode = mode_name(store.mode());
    let records = store.len()?;
    let empty = store.is_empty()?;
    let stale = store.is_stale()?;
    let max_key_size = store.max_key_size();
    let max_value_size = store.max_value_size();

    match output {
        OutputMode::Text => {
            writeln!(stdout, "pool={pool}")?;
            writeln!(stdout, "path={path}")?;
            writeln!(stdout, "mode={mode}")?;
            writeln!(stdout, "records={records}")?;
            writeln!(stdout, "empty={empty}")?;
            writeln!(stdout, "stale={stale}")?;
            writeln!(stdout, "max_key_size={max_key_size}")?;
            writeln!(stdout, "max_value_size={max_value_size}")?;
        }
        OutputMode::Json => {
            let value = json!({
                "pool": pool,
                "path": path,
                "mode": mode,
                "records": records,
                "empty": empty,
                "stale": stale,
                "max_key_size": max_key_size,
                "max_value_size": max_value_size,
            });
            writeln_json(stdout, &value)?;
        }
    }
    Ok(EXIT_OK)
}

fn dump<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    output: OutputMode,
) -> Result<u8, CliError> {
    let records = store.dump()?;
    match output {
        OutputMode::Text => {
            for (key, value) in records {
                writeln!(stdout, "{key}={value}")?;
            }
        }
        OutputMode::Json => {
            let array: Vec<_> = records
                .into_iter()
                .map(|(key, value)| json!({ "key": key, "value": value }))
                .collect();
            writeln_json(stdout, &serde_json::Value::Array(array))?;
        }
    }
    Ok(EXIT_OK)
}

fn entries<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    output: OutputMode,
) -> Result<u8, CliError> {
    let mut entries: Vec<_> = store.entries()?.into_iter().collect();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
    match output {
        OutputMode::Text => {
            for (key, value) in entries {
                writeln!(stdout, "{key}={value}")?;
            }
        }
        OutputMode::Json => {
            let mut object = serde_json::Map::new();
            for (key, value) in entries {
                object.insert(key, serde_json::Value::String(value));
            }
            writeln_json(stdout, &serde_json::Value::Object(object))?;
        }
    }
    Ok(EXIT_OK)
}

fn read<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    key: &str,
    output: OutputMode,
) -> Result<u8, CliError> {
    match store.read(key)? {
        Some(value) => {
            match output {
                OutputMode::Text => writeln!(stdout, "{value}")?,
                OutputMode::Json => {
                    let payload = json!({ "key": key, "value": value });
                    writeln_json(stdout, &payload)?
                }
            }
            Ok(EXIT_OK)
        }
        None => Ok(EXIT_NOT_FOUND),
    }
}

fn delete<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    key: &str,
    output: OutputMode,
) -> Result<u8, CliError> {
    let removed = store.delete(key)?;
    match output {
        OutputMode::Text => writeln!(stdout, "{removed}")?,
        OutputMode::Json => {
            writeln_json(stdout, &json!({ "removed": removed }))?
        }
    }
    Ok(EXIT_OK)
}

fn delete_multiple<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    keys: Vec<String>,
    output: OutputMode,
) -> Result<u8, CliError> {
    let removed = store.delete_multiple(keys)?;
    match output {
        OutputMode::Text => writeln!(stdout, "{removed}")?,
        OutputMode::Json => {
            writeln_json(stdout, &json!({ "removed": removed }))?
        }
    }
    Ok(EXIT_OK)
}

fn is_stale<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    output: OutputMode,
) -> Result<u8, CliError> {
    let stale = store.is_stale()?;
    match output {
        OutputMode::Text => writeln!(stdout, "{stale}")?,
        OutputMode::Json => writeln_json(stdout, &json!({ "stale": stale }))?,
    }
    Ok(if stale { EXIT_OK } else { EXIT_NOT_FOUND })
}

fn report_success(
    store: &KvpPoolStore,
    vm_id: String,
    agent: String,
    message: Option<String>,
) -> Result<u8, CliError> {
    let mut report =
        ProvisioningReport::success(agent, vm_id, ReportPpsType::None);
    if let Some(message) = message {
        report = report.with_extra("message", message);
    }
    write_report(store, &report)?;
    Ok(EXIT_OK)
}

fn report_failure(
    store: &KvpPoolStore,
    vm_id: String,
    agent: String,
    reason: String,
    documentation_url: Option<String>,
) -> Result<u8, CliError> {
    let mut report =
        ProvisioningReport::failure(agent, vm_id, reason, ReportPpsType::None);
    if let Some(url) = documentation_url {
        report = report.with_documentation_url(url);
    }
    write_report(store, &report)?;
    Ok(EXIT_OK)
}

fn writeln_json<W: Write>(
    stdout: &mut W,
    value: &serde_json::Value,
) -> Result<(), CliError> {
    // Serializing `serde_json::Value` to a String only fails on writer
    // I/O errors, which `to_string` cannot produce, so this is safe to
    // unwrap.
    let rendered = serde_json::to_string(value)
        .expect("serde_json::Value always serializes to a String");
    stdout.write_all(rendered.as_bytes())?;
    stdout.write_all(b"\n")?;
    Ok(())
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
    store.load(parse_key_value_lines(&buf)?)?;
    Ok(EXIT_OK)
}

fn append_multiple(
    store: &KvpPoolStore,
    file: Option<PathBuf>,
) -> Result<u8, CliError> {
    match file {
        Some(path) => append_multiple_from_reader(store, fs::File::open(path)?),
        None => append_multiple_from_reader(store, io::stdin().lock()),
    }
}

fn append_multiple_from_reader<R: Read>(
    store: &KvpPoolStore,
    mut reader: R,
) -> Result<u8, CliError> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    store.append_multiple(parse_key_value_lines(&buf)?)?;
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
    use rstest::rstest;
    use tempfile::TempDir;

    fn cli(dir: &TempDir, command: Command) -> Cli {
        Cli {
            pool: PoolArg::Guest,
            dir: Some(dir.path().to_path_buf()),
            unsafe_mode: false,
            json: false,
            command,
        }
    }

    fn cli_json(dir: &TempDir, command: Command) -> Cli {
        Cli {
            pool: PoolArg::Guest,
            dir: Some(dir.path().to_path_buf()),
            unsafe_mode: false,
            json: true,
            command,
        }
    }

    fn store_at(dir: &TempDir) -> KvpPoolStore {
        KvpPoolStore::new_in(KvpPool::Guest, dir.path(), PoolMode::Safe)
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
        assert!(!cli.json);
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
            "--json",
            "dump",
        ]);
        assert!(matches!(cli.pool, PoolArg::AutoExternal));
        assert_eq!(cli.dir, Some(PathBuf::from("/tmp/kvp")));
        assert!(cli.unsafe_mode);
        assert!(cli.json);
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
    fn parse_append_multiple_file() {
        let cli = Cli::parse_from([
            "libazureinit-kvp",
            "append-multiple",
            "--file",
            "/tmp/records",
        ]);
        assert!(matches!(
            cli.command,
            Command::AppendMultiple { ref file }
                if file.as_deref() == Some(Path::new("/tmp/records"))
        ));
    }

    #[test]
    fn parse_delete_multiple_keys() {
        let cli =
            Cli::parse_from(["libazureinit-kvp", "delete-multiple", "a", "b"]);
        assert!(matches!(
            cli.command,
            Command::DeleteMultiple { ref keys }
                if keys == &vec!["a".to_string(), "b".to_string()]
        ));
    }

    #[test]
    fn parse_report_success() {
        let cli = Cli::parse_from([
            "libazureinit-kvp",
            "report-success",
            "--vm-id",
            "vm-1",
            "--agent",
            "Azure-Init/0.0.0",
            "--message",
            "all good",
        ]);
        assert!(matches!(
            cli.command,
            Command::ReportSuccess { ref vm_id, ref agent, ref message }
                if vm_id == "vm-1"
                    && agent == "Azure-Init/0.0.0"
                    && message.as_deref() == Some("all good")
        ));
    }

    #[test]
    fn parse_report_failure() {
        let cli = Cli::parse_from([
            "libazureinit-kvp",
            "report-failure",
            "--vm-id",
            "vm-1",
            "--agent",
            "Azure-Init/0.0.0",
            "--reason",
            "boom",
            "--documentation-url",
            "https://aka.ms/x",
        ]);
        assert!(matches!(
            cli.command,
            Command::ReportFailure {
                ref vm_id,
                ref agent,
                ref reason,
                ref documentation_url,
            } if vm_id == "vm-1"
                && agent == "Azure-Init/0.0.0"
                && reason == "boom"
                && documentation_url.as_deref() == Some("https://aka.ms/x")
        ));
    }

    #[test]
    fn dispatch_report_success_writes_single_record() {
        let dir = TempDir::new().unwrap();
        let command = Command::ReportSuccess {
            vm_id: "vm-1".into(),
            agent: "Azure-Init/0.0.0".into(),
            message: Some("hello".into()),
        };
        let (code, _) = run_dispatch(cli(&dir, command));
        assert_eq!(code, EXIT_OK);

        let entries = store_at(&dir).entries().unwrap();
        assert_eq!(entries.len(), 1);
        let value = entries.get("PROVISIONING_REPORT").unwrap();
        assert!(value.starts_with(
            "result=success|agent=Azure-Init/0.0.0\
|pps_type=None|vm_id=vm-1|timestamp="
        ));
        assert!(value.ends_with("|message=hello"));
    }

    #[test]
    fn dispatch_report_failure_writes_single_record() {
        let dir = TempDir::new().unwrap();
        let command = Command::ReportFailure {
            vm_id: "vm-1".into(),
            agent: "Azure-Init/0.0.0".into(),
            reason: "boom".into(),
            documentation_url: Some("https://aka.ms/x".into()),
        };
        let (code, _) = run_dispatch(cli(&dir, command));
        assert_eq!(code, EXIT_OK);

        let entries = store_at(&dir).entries().unwrap();
        assert_eq!(entries.len(), 1);
        let value = entries.get("PROVISIONING_REPORT").unwrap();
        assert!(value.starts_with(
            "result=error|reason=boom|agent=Azure-Init/0.0.0\
|pps_type=None|vm_id=vm-1|timestamp="
        ));
        assert!(value.ends_with("|documentation_url=https://aka.ms/x"));
    }

    #[test]
    fn dispatch_report_overrides_previous_record() {
        let dir = TempDir::new().unwrap();
        run_dispatch(cli(
            &dir,
            Command::ReportFailure {
                vm_id: "vm-1".into(),
                agent: "Azure-Init/0.0.0".into(),
                reason: "boom".into(),
                documentation_url: None,
            },
        ));
        run_dispatch(cli(
            &dir,
            Command::ReportSuccess {
                vm_id: "vm-1".into(),
                agent: "Azure-Init/0.0.0".into(),
                message: None,
            },
        ));

        let entries = store_at(&dir).entries().unwrap();
        assert_eq!(entries.len(), 1);
        let value = entries.get("PROVISIONING_REPORT").unwrap();
        assert!(value.starts_with("result=success|"));
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
            json: false,
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
    fn dispatch_append_multiple_preserves_existing_records() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        store.insert("a", "1").unwrap();

        let code =
            append_multiple_from_reader(&store, Cursor::new("a=2\nb=3\n"))
                .unwrap();
        assert_eq!(code, EXIT_OK);

        let (_, dumped) = run_dispatch(cli(&dir, Command::Dump));
        assert_eq!(dumped, "a=1\na=2\nb=3\n");
    }

    #[test]
    fn dispatch_delete_multiple_prints_removed_record_count() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        store
            .append_multiple(vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string()),
                ("a".to_string(), "3".to_string()),
            ])
            .unwrap();

        let (code, out) = run_dispatch(cli(
            &dir,
            Command::DeleteMultiple {
                keys: vec!["a".into(), "missing".into()],
            },
        ));
        assert_eq!(code, EXIT_OK);
        assert_eq!(out, "2\n");

        let (_, dumped) = run_dispatch(cli(&dir, Command::Dump));
        assert_eq!(dumped, "b=2\n");
    }

    #[rstest]
    #[case::unconditional(false, true)]
    #[case::if_stale_no_op_on_fresh_store(true, false)]
    fn dispatch_clear_executes_both_branches(
        #[case] if_stale: bool,
        #[case] expect_empty_after: bool,
    ) {
        let dir = TempDir::new().unwrap();
        store_at(&dir).insert("k", "v").unwrap();

        let (code, _) = run_dispatch(cli(&dir, Command::Clear { if_stale }));
        assert_eq!(code, EXIT_OK);
        assert_eq!(store_at(&dir).is_empty().unwrap(), expect_empty_after);
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
            json: false,
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
    fn append_multiple_from_reader_surfaces_parse_errors() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        let err =
            append_multiple_from_reader(&store, Cursor::new("bad-line\n"))
                .unwrap_err();
        assert!(matches!(err, CliError::Usage(_)));
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

        let kvp_io = CliError::Kvp(KvpError::Io(io::Error::other("kvp-io")));
        assert!(kvp_io.to_string().contains("kvp-io"));
        assert_eq!(kvp_io.exit_code(), EXIT_IO);

        let io_err = CliError::Io(io::Error::other("raw-io"));
        assert_eq!(io_err.to_string(), "raw-io");
        assert_eq!(io_err.exit_code(), EXIT_IO);
    }

    #[test]
    fn cli_error_from_conversions() {
        let from_kvp: CliError = KvpError::EmptyKey.into();
        assert!(matches!(from_kvp, CliError::Kvp(_)));

        let from_io: CliError = io::Error::other("boom").into();
        assert!(matches!(from_io, CliError::Io(_)));
    }

    fn parse_json(out: &str) -> serde_json::Value {
        serde_json::from_str(out.trim_end_matches('\n'))
            .expect("CLI JSON output must parse")
    }

    #[test]
    fn dispatch_info_json_emits_object_with_all_fields() {
        let dir = TempDir::new().unwrap();
        let (code, out) = run_dispatch(cli_json(&dir, Command::Info));
        assert_eq!(code, EXIT_OK);

        let json = parse_json(&out);
        assert_eq!(json["pool"], "guest");
        assert_eq!(json["mode"], "safe");
        assert_eq!(json["records"], 0);
        assert_eq!(json["empty"], true);
        assert_eq!(json["stale"], false);
        assert!(json["max_key_size"].is_number());
        assert!(json["max_value_size"].is_number());
        assert!(json["path"].is_string());
    }

    #[test]
    fn dispatch_dump_json_preserves_duplicates_and_order() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        store.insert("b", "two").unwrap();
        store.append("b", "two-prime").unwrap();
        store.insert("a", "one").unwrap();

        let (_, out) = run_dispatch(cli_json(&dir, Command::Dump));
        let json = parse_json(&out);
        let array = json.as_array().expect("dump --json returns array");
        assert_eq!(array.len(), 3);
        assert_eq!(array[0]["key"], "b");
        assert_eq!(array[0]["value"], "two");
        assert_eq!(array[1]["key"], "b");
        assert_eq!(array[1]["value"], "two-prime");
        assert_eq!(array[2]["key"], "a");
        assert_eq!(array[2]["value"], "one");
    }

    #[test]
    fn dispatch_entries_json_emits_sorted_object() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        store.insert("b", "two").unwrap();
        store.insert("a", "one").unwrap();

        let (_, out) = run_dispatch(cli_json(&dir, Command::Entries));
        let json = parse_json(&out);
        assert_eq!(json["a"], "one");
        assert_eq!(json["b"], "two");
    }

    #[test]
    fn dispatch_read_json_emits_object_when_present() {
        let dir = TempDir::new().unwrap();
        store_at(&dir).insert("k", "v").unwrap();

        let (code, out) =
            run_dispatch(cli_json(&dir, Command::Read { key: "k".into() }));
        assert_eq!(code, EXIT_OK);
        let json = parse_json(&out);
        assert_eq!(json["key"], "k");
        assert_eq!(json["value"], "v");
    }

    #[test]
    fn dispatch_delete_json_reports_removed_flag() {
        let dir = TempDir::new().unwrap();
        store_at(&dir).insert("k", "v").unwrap();

        let (code, out) =
            run_dispatch(cli_json(&dir, Command::Delete { key: "k".into() }));
        assert_eq!(code, EXIT_OK);
        assert_eq!(parse_json(&out)["removed"], true);
    }

    #[test]
    fn dispatch_delete_multiple_json_reports_count() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir);
        store.insert("a", "1").unwrap();
        store.insert("b", "2").unwrap();

        let (code, out) = run_dispatch(cli_json(
            &dir,
            Command::DeleteMultiple {
                keys: vec!["a".into(), "missing".into()],
            },
        ));
        assert_eq!(code, EXIT_OK);
        assert_eq!(parse_json(&out)["removed"], 1);
    }

    #[test]
    fn dispatch_is_stale_json_emits_object() {
        let dir = TempDir::new().unwrap();
        let (code, out) = run_dispatch(cli_json(&dir, Command::IsStale));
        assert_eq!(code, EXIT_NOT_FOUND);
        assert_eq!(parse_json(&out)["stale"], false);
    }
}
