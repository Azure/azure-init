// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use crate::{
    write_report, DiagnosticEvent, DiagnosticRecord, DiagnosticsKvp, KvpError,
    KvpPool, KvpPoolStore, PoolMode, ProvisioningReport, ReportPpsType,
};

const EXIT_OK: u8 = 0;
const EXIT_NOT_FOUND: u8 = 1;
const EXIT_USAGE_OR_VALIDATION: u8 = 2;
const EXIT_IO: u8 = 3;

/// Default reporting agent identifier, derived from this crate's version
const DEFAULT_AGENT: &str =
    concat!("libazureinit-kvp/", env!("CARGO_PKG_VERSION"));

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
    ///
    /// With --parse-diagnostics, reassemble chunked diagnostic events and
    /// decode each record instead of printing raw KEY=VALUE lines.
    Dump {
        /// Reassemble chunked diagnostic events and decode each record as
        /// an event, raw, or malformed entry.
        #[arg(long)]
        parse_diagnostics: bool,
        /// Also print raw (non-event) records such as PROVISIONING_REPORT.
        /// Only applies to the unfiltered view; --level/--name/--tail
        /// produce an events-only view where raw records never appear.
        #[arg(
            long,
            requires = "parse_diagnostics",
            conflicts_with_all = ["level", "name", "tail"]
        )]
        include_raw: bool,
        /// Only show events at this level (error, warn, info, debug,
        /// trace).
        #[arg(long, requires = "parse_diagnostics")]
        level: Option<String>,
        /// Only show events whose name contains this substring.
        #[arg(long, requires = "parse_diagnostics")]
        name: Option<String>,
        /// Print only the last COUNT events (default 20 when COUNT is
        /// omitted).
        #[arg(
            short = 'n',
            long = "tail",
            num_args = 0..=1,
            default_missing_value = "20",
            requires = "parse_diagnostics"
        )]
        tail: Option<usize>,
    },
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
    /// Clear the pool. Pass --if-stale to clear only when stale, or
    /// --diagnostics to remove only diagnostic event keys.
    Clear {
        /// Only clear if the store is currently stale.
        #[arg(long = "if-stale", conflicts_with = "diagnostics")]
        if_stale: bool,
        /// Remove every diagnostic event key (valid or malformed),
        /// leaving raw records such as PROVISIONING_REPORT intact.
        #[arg(long)]
        diagnostics: bool,
    },
    /// Print whether the pool is stale (exit 0 if stale, 1 otherwise).
    IsStale,
    /// Write a success provisioning health report, overriding any existing
    /// `PROVISIONING_REPORT` record.
    ReportSuccess {
        /// Virtual machine identifier.
        #[arg(long)]
        vm_id: Option<String>,
        /// Reporting agent identifier (e.g. libazureinit-kvp/0.1.0).
        #[arg(long, default_value = DEFAULT_AGENT)]
        agent: String,
        /// Additional key=value supporting data, comma-separated
        /// (e.g. --supporting-data k1=v1,k2=v2). The flag may also be
        /// repeated.
        #[arg(long, value_parser = parse_supporting_data)]
        supporting_data: Vec<SupportingData>,
    },
    /// Write a failure provisioning health report, overriding any existing
    /// `PROVISIONING_REPORT` record.
    ReportFailure {
        /// Virtual machine identifier ID.
        #[arg(long)]
        vm_id: Option<String>,
        /// Reporting agent identifier (e.g. libazureinit-kvp/0.1.0).
        #[arg(long, default_value = DEFAULT_AGENT)]
        agent: String,
        /// Failure reason.
        #[arg(long)]
        reason: String,
        /// Optional documentation URL describing the failure.
        #[arg(long)]
        documentation_url: Option<String>,
        /// Additional key=value supporting data, comma-separated
        /// (e.g. --supporting-data k1=v1,k2=v2). The flag may also be
        /// repeated.
        ///
        /// To keep a literal comma in a value, wrap that value in matching
        /// single or double quotes. The shell strips quotes first, so wrap
        /// the whole argument to let the inner quotes through:
        ///
        /// --supporting-data "k1='a,b',k2=v2"  ->  k1=a,b  k2=v2
        ///
        /// --supporting-data 'k1="a,b",k2=v2'  ->  k1=a,b  k2=v2
        ///
        /// Quotes are only special at the start of a value and must be
        /// balanced.
        #[arg(long, value_parser = parse_supporting_data)]
        supporting_data: Vec<SupportingData>,
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
        Command::Dump {
            parse_diagnostics,
            include_raw,
            level,
            name,
            tail,
        } => {
            let parse = parse_diagnostics.then_some(ParseDiagnosticsArgs {
                include_raw,
                level,
                name,
                tail,
            });
            dump(&store, stdout, parse, output)
        }
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
        Command::Clear {
            if_stale,
            diagnostics,
        } => {
            if diagnostics {
                DiagnosticsKvp::new(store.clone(), "", "").clear()?;
            } else if if_stale {
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
            supporting_data,
        } => report_success(&store, vm_id, agent, supporting_data),
        Command::ReportFailure {
            vm_id,
            agent,
            reason,
            documentation_url,
            supporting_data,
        } => report_failure(
            &store,
            vm_id,
            agent,
            reason,
            documentation_url,
            supporting_data,
        ),
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

/// Options for the `dump --parse-diagnostics` view; absent for a raw dump.
struct ParseDiagnosticsArgs {
    include_raw: bool,
    level: Option<String>,
    name: Option<String>,
    tail: Option<usize>,
}

fn dump<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    parse: Option<ParseDiagnosticsArgs>,
    output: OutputMode,
) -> Result<u8, CliError> {
    if let Some(parse) = parse {
        // --level/--name/--tail select the decoded events-only view;
        // otherwise every record is shown (raw hidden unless
        // --include-raw).
        if parse.level.is_some() || parse.name.is_some() || parse.tail.is_some()
        {
            return diagnostics_events(
                store,
                stdout,
                parse.level,
                parse.name,
                parse.tail,
                output,
            );
        }
        return diagnostics_records(store, stdout, parse.include_raw, output);
    }

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

fn diagnostics_records<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    include_raw: bool,
    output: OutputMode,
) -> Result<u8, CliError> {
    let diagnostics = DiagnosticsKvp::new(store.clone(), "", "");
    let records: Vec<_> = diagnostics
        .records()?
        .into_iter()
        .filter(|record| {
            include_raw || !matches!(record, DiagnosticRecord::Raw { .. })
        })
        .collect();

    match output {
        OutputMode::Text => {
            for record in &records {
                let line = match record {
                    DiagnosticRecord::Event { event, chunks } => format!(
                        "event level={} name={} event_id={} chunks={} \
                         message={}",
                        event.level,
                        event.name,
                        event.event_id,
                        chunks,
                        event.message
                    ),
                    DiagnosticRecord::Raw { key, value } => {
                        format!("raw key={key} value={value}")
                    }
                    DiagnosticRecord::Malformed { key, value, reason } => {
                        format!(
                            "malformed key={key} reason={reason} \
                             value={value}"
                        )
                    }
                };
                writeln!(stdout, "{line}")?;
            }
        }
        OutputMode::Json => {
            let array: Vec<_> =
                records.iter().map(diagnostics_record_json).collect();
            writeln_json(stdout, &serde_json::Value::Array(array))?;
        }
    }
    Ok(EXIT_OK)
}

fn diagnostics_events<W: Write>(
    store: &KvpPoolStore,
    stdout: &mut W,
    level: Option<String>,
    name: Option<String>,
    tail: Option<usize>,
    output: OutputMode,
) -> Result<u8, CliError> {
    let level = level.as_deref().map(parse_level_filter).transpose()?;

    let diagnostics = DiagnosticsKvp::new(store.clone(), "", "");
    let mut events = diagnostics.events()?;

    if let Some(level) = level {
        events.retain(|event| event.level == level);
    }
    if let Some(needle) = name.as_deref() {
        events.retain(|event| event.name.contains(needle));
    }
    if let Some(count) = tail {
        let excess = events.len().saturating_sub(count);
        events.drain(..excess);
    }

    match output {
        OutputMode::Text => {
            for event in &events {
                let line = format!(
                    "event level={} name={} event_id={} message={}",
                    event.level, event.name, event.event_id, event.message
                );
                writeln!(stdout, "{line}")?;
            }
        }
        OutputMode::Json => {
            let array: Vec<_> =
                events.iter().map(diagnostics_event_json).collect();
            writeln_json(stdout, &serde_json::Value::Array(array))?;
        }
    }
    Ok(EXIT_OK)
}

/// Parse a `--level` filter argument into a [`tracing::Level`].
fn parse_level_filter(level: &str) -> Result<tracing::Level, CliError> {
    level.parse::<tracing::Level>().map_err(|_| {
        CliError::Usage(format!(
            "invalid level '{level}' (expected error, warn, info, debug, \
             or trace)"
        ))
    })
}

/// Render a [`DiagnosticRecord`] as a JSON object.
fn diagnostics_record_json(record: &DiagnosticRecord) -> serde_json::Value {
    match record {
        DiagnosticRecord::Event { event, chunks } => {
            let mut value = diagnostics_event_json(event);
            if let serde_json::Value::Object(map) = &mut value {
                map.insert("chunks".to_string(), json!(chunks));
            }
            value
        }
        DiagnosticRecord::Raw { key, value } => json!({
            "kind": "raw",
            "key": key,
            "value": value,
        }),
        DiagnosticRecord::Malformed { key, value, reason } => json!({
            "kind": "malformed",
            "key": key,
            "value": value,
            "reason": reason,
        }),
    }
}

/// Render a [`DiagnosticEvent`] as a JSON object (without chunk count).
fn diagnostics_event_json(event: &DiagnosticEvent) -> serde_json::Value {
    json!({
        "kind": "event",
        "level": event.level.to_string(),
        "name": event.name,
        "event_id": event.event_id,
        "message": event.message,
    })
}

fn report_success(
    store: &KvpPoolStore,
    vm_id: Option<String>,
    agent: String,
    supporting_data: Vec<SupportingData>,
) -> Result<u8, CliError> {
    let vm_id = resolve_vm_id(vm_id)?;
    let mut report =
        ProvisioningReport::success(agent, vm_id, ReportPpsType::None);
    for (key, value) in supporting_data.into_iter().flat_map(|data| data.0) {
        report = report.with_extra(key, value);
    }
    write_report(store, &report)?;
    Ok(EXIT_OK)
}

fn report_failure(
    store: &KvpPoolStore,
    vm_id: Option<String>,
    agent: String,
    reason: String,
    documentation_url: Option<String>,
    supporting_data: Vec<SupportingData>,
) -> Result<u8, CliError> {
    let vm_id = resolve_vm_id(vm_id)?;
    let mut report =
        ProvisioningReport::failure(agent, vm_id, reason, ReportPpsType::None);
    for (key, value) in supporting_data.into_iter().flat_map(|data| data.0) {
        report = report.with_extra(key, value);
    }
    if let Some(url) = documentation_url {
        report = report.with_documentation_url(url);
    }
    write_report(store, &report)?;
    Ok(EXIT_OK)
}

/// Resolve the VM ID for a report, falling back to the current VM's ID when
/// `--vm-id` was not supplied.
fn resolve_vm_id(vm_id: Option<String>) -> Result<String, CliError> {
    resolve_vm_id_with(vm_id, crate::vm_id::get_vm_id)
}

/// Resolve the VM ID using `lookup` to determine the current VM's ID when
/// `--vm-id` was not supplied. Split out from [`resolve_vm_id`] so the
/// auto-detection branches can be tested without touching the host's DMI data.
fn resolve_vm_id_with(
    vm_id: Option<String>,
    lookup: impl FnOnce() -> Option<String>,
) -> Result<String, CliError> {
    match vm_id {
        Some(vm_id) => Ok(vm_id),
        None => lookup().ok_or_else(|| {
            CliError::Usage(
                "unable to determine the current VM ID automatically; \
                 pass --vm-id explicitly"
                    .to_string(),
            )
        }),
    }
}

/// One or more `key=value` supporting-data pairs parsed from a single
/// `--supporting-data` argument.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SupportingData(Vec<(String, String)>);

/// Parse a `--supporting-data` argument into its `key=value` pairs.
///
/// Fields are comma-separated. A value may be wrapped in matching single or
/// double quotes so it can contain literal commas; the quotes are honored
/// only when they wrap the *entire* value (the opening quote immediately
/// follows `=` and the matching quote ends the field) and are stripped from
/// the stored value. Empty fields (such as a trailing comma) are ignored.
///
/// Supported (input -> parsed pairs):
/// - `k=v` -> `k`=`v`
/// - `k1=v1,k2=v2` -> `k1`=`v1`, `k2`=`v2`
/// - `k='a,b'` or `k="a,b"` -> `k`=`a,b` (quotes protect the comma)
/// - `k=a'b` -> `k`=`a'b` (a quote not at the value start is literal)
/// - `k=v,` -> `k`=`v` (trailing/empty field ignored)
///
/// Rejected:
/// - `novalue` -> missing `=`
/// - `=v` -> empty key
/// - `k='a,b` -> unterminated quote
/// - `k='a,b'x` -> characters after a quoted value
fn parse_supporting_data(raw: &str) -> Result<SupportingData, String> {
    let mut pairs = Vec::new();
    for field in split_supporting_data_fields(raw)? {
        if field.is_empty() {
            continue;
        }
        pairs.push(parse_key_value_pair(field)?);
    }
    Ok(SupportingData(pairs))
}

/// Split a `--supporting-data` argument into `key=value` field slices on
/// top-level commas. See [`parse_supporting_data`] for the quoting rules.
fn split_supporting_data_fields(raw: &str) -> Result<Vec<&str>, String> {
    let bytes = raw.as_bytes();
    let len = bytes.len();
    let mut fields = Vec::new();
    let mut idx = 0;

    loop {
        let field_start = idx;

        while idx < len && bytes[idx] != b'=' && bytes[idx] != b',' {
            idx += 1;
        }

        if idx < len && bytes[idx] == b'=' {
            idx += 1;
            if idx < len && (bytes[idx] == b'\'' || bytes[idx] == b'"') {
                let quote = bytes[idx];
                idx += 1;
                let mut closed = false;
                while idx < len {
                    let ch = bytes[idx];
                    idx += 1;
                    if ch == quote {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(format!(
                        "supporting data '{raw}' has an unterminated quote"
                    ));
                }
                if idx < len && bytes[idx] != b',' {
                    return Err(format!(
                        "supporting data '{raw}' has unexpected characters \
                         after a quoted value"
                    ));
                }
            } else {
                while idx < len && bytes[idx] != b',' {
                    idx += 1;
                }
            }
        }

        fields.push(&raw[field_start..idx]);

        if idx >= len {
            break;
        }
        idx += 1;
    }

    Ok(fields)
}

/// Parse a single `key=value` supporting-data pair, stripping one layer of
/// surrounding single or double quotes from the value.
fn parse_key_value_pair(raw: &str) -> Result<(String, String), String> {
    let (key, value) = raw.split_once('=').ok_or_else(|| {
        format!("supporting data '{raw}' must be in key=value format")
    })?;
    if key.is_empty() {
        return Err(format!("supporting data '{raw}' has an empty key"));
    }
    Ok((key.to_string(), unquote(value).to_string()))
}

/// Strip one layer of matching surrounding single or double quotes.
fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        if (first == b'\'' || first == b'"') && bytes[bytes.len() - 1] == first
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn writeln_json<W: Write>(
    stdout: &mut W,
    value: &serde_json::Value,
) -> Result<(), CliError> {
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

    /// A plain `dump` command with no diagnostics parsing.
    fn dump_cmd() -> Command {
        Command::Dump {
            parse_diagnostics: false,
            include_raw: false,
            level: None,
            name: None,
            tail: None,
        }
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
        assert!(matches!(cli.command, Command::Dump { .. }));
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
            "--supporting-data",
            "k1=v1,k2=v2",
        ]);
        assert!(matches!(
            cli.command,
            Command::ReportSuccess { ref vm_id, ref agent, ref supporting_data }
                if vm_id.as_deref() == Some("vm-1")
                    && agent == "Azure-Init/0.0.0"
                    && supporting_data == &vec![SupportingData(vec![
                        ("k1".to_string(), "v1".to_string()),
                        ("k2".to_string(), "v2".to_string()),
                    ])]
        ));
    }

    #[test]
    fn parse_report_supporting_data_preserves_commas_in_values() {
        let cli = Cli::parse_from([
            "libazureinit-kvp",
            "report-success",
            "--supporting-data",
            "k1='foo,bar',k2=foo2",
        ]);
        assert!(matches!(
            cli.command,
            Command::ReportSuccess { ref supporting_data, .. }
                if supporting_data == &vec![SupportingData(vec![
                    ("k1".to_string(), "foo,bar".to_string()),
                    ("k2".to_string(), "foo2".to_string()),
                ])]
        ));
    }

    #[test]
    fn parse_report_success_defaults_agent_and_vm_id() {
        let cli = Cli::parse_from(["libazureinit-kvp", "report-success"]);
        assert!(matches!(
            cli.command,
            Command::ReportSuccess { ref vm_id, ref agent, ref supporting_data }
                if vm_id.is_none()
                    && agent == DEFAULT_AGENT
                    && supporting_data.is_empty()
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
            "--supporting-data",
            "details=bad config",
        ]);
        assert!(matches!(
            cli.command,
            Command::ReportFailure {
                ref vm_id,
                ref agent,
                ref reason,
                ref documentation_url,
                ref supporting_data,
            } if vm_id.as_deref() == Some("vm-1")
                && agent == "Azure-Init/0.0.0"
                && reason == "boom"
                && documentation_url.as_deref() == Some("https://aka.ms/x")
                && supporting_data == &vec![SupportingData(vec![
                    ("details".to_string(), "bad config".to_string()),
                ])]
        ));
    }

    #[test]
    fn parse_report_rejects_supporting_data_without_equals() {
        let result = Cli::try_parse_from([
            "libazureinit-kvp",
            "report-success",
            "--supporting-data",
            "novalue",
        ]);
        assert!(result.is_err());
    }

    #[rstest]
    #[case::key_value("k=v", Ok(("k".to_string(), "v".to_string())))]
    #[case::value_with_commas(
        "k=foo,bar",
        Ok(("k".to_string(), "foo,bar".to_string()))
    )]
    #[case::quoted_value(
        "k='foo,bar'",
        Ok(("k".to_string(), "foo,bar".to_string()))
    )]
    #[case::missing_separator(
        "novalue",
        Err("supporting data 'novalue' must be in key=value format".to_string())
    )]
    #[case::empty_key("=v", Err("supporting data '=v' has an empty key".to_string()))]
    fn parse_key_value_pair_handles_input(
        #[case] raw: &str,
        #[case] expected: Result<(String, String), String>,
    ) {
        assert_eq!(parse_key_value_pair(raw), expected);
    }

    #[rstest]
    #[case::single("k=v", vec![("k", "v")])]
    #[case::multiple("k1=v1,k2=v2", vec![("k1", "v1"), ("k2", "v2")])]
    #[case::single_quoted_comma("k1='a,b',k2=v2", vec![("k1", "a,b"), ("k2", "v2")])]
    #[case::double_quoted_comma("k1=\"a,b\"", vec![("k1", "a,b")])]
    #[case::apostrophe_in_bare_value_is_literal(
        "k1=it's,k2=v2",
        vec![("k1", "it's"), ("k2", "v2")]
    )]
    #[case::quote_in_middle_is_literal("k=a'b", vec![("k", "a'b")])]
    #[case::trailing_comma_ignored("k=v,", vec![("k", "v")])]
    #[case::empty_input("", vec![])]
    #[case::leading_comma_ignored(",k=v", vec![("k", "v")])]
    #[case::only_commas_ignored(",,", vec![])]
    #[case::consecutive_commas_ignored(
        "k1=v1,,k2=v2",
        vec![("k1", "v1"), ("k2", "v2")]
    )]
    #[case::empty_quoted_value("k=''", vec![("k", "")])]
    #[case::empty_bare_value("k=", vec![("k", "")])]
    #[case::bare_value_with_equals("k=a=b", vec![("k", "a=b")])]
    #[case::quoted_value_with_equals_and_comma(
        "k='a=b,c'",
        vec![("k", "a=b,c")]
    )]
    #[case::mixed_quote_types(
        "k1='a,b',k2=\"c,d\"",
        vec![("k1", "a,b"), ("k2", "c,d")]
    )]
    #[case::quoted_value_then_trailing_comma("k='a,b',", vec![("k", "a,b")])]
    fn parse_supporting_data_splits_and_unquotes(
        #[case] raw: &str,
        #[case] expected: Vec<(&str, &str)>,
    ) {
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            parse_supporting_data(raw).unwrap(),
            SupportingData(expected)
        );
    }

    #[rstest]
    #[case::missing_separator(
        "k1=v1,bad",
        "supporting data 'bad' must be in key=value format"
    )]
    #[case::empty_key("=v", "supporting data '=v' has an empty key")]
    #[case::unterminated_quote(
        "k1='a,b",
        "supporting data 'k1='a,b' has an unterminated quote"
    )]
    #[case::chars_after_quoted_value(
        "k1='a,b'x",
        "supporting data 'k1='a,b'x' has unexpected characters after a quoted value"
    )]
    #[case::chars_after_empty_quoted_value(
        "k=''x",
        "supporting data 'k=''x' has unexpected characters after a quoted value"
    )]
    #[case::double_quote_unterminated(
        "k=\"a,b",
        "supporting data 'k=\"a,b' has an unterminated quote"
    )]
    #[case::missing_key_on_quoted_field(
        "k1=v1,'a,b'",
        "supporting data ''a' must be in key=value format"
    )]
    fn parse_supporting_data_rejects_invalid_fields(
        #[case] raw: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(parse_supporting_data(raw).unwrap_err(), expected);
    }

    #[rstest]
    #[case::explicit(Some("vm-1"), Some("auto-vm"), Ok("vm-1"))]
    #[case::fallback_to_lookup(None, Some("auto-vm"), Ok("auto-vm"))]
    #[case::lookup_fails(
        None,
        None,
        Err("unable to determine the current VM ID automatically")
    )]
    fn resolve_vm_id_resolves_from_flag_or_lookup(
        #[case] vm_id: Option<&str>,
        #[case] lookup: Option<&str>,
        #[case] expected: Result<&str, &str>,
    ) {
        let result = resolve_vm_id_with(vm_id.map(str::to_string), || {
            lookup.map(str::to_string)
        });

        match expected {
            Ok(vm) => assert_eq!(result.unwrap(), vm),
            Err(needle) => {
                let err = result.unwrap_err();
                assert!(matches!(err, CliError::Usage(_)));
                assert_eq!(err.exit_code(), EXIT_USAGE_OR_VALIDATION);
                assert!(err.to_string().contains(needle));
            }
        }
    }

    #[test]
    fn dispatch_report_success_writes_single_record() {
        let dir = TempDir::new().unwrap();
        let command = Command::ReportSuccess {
            vm_id: Some("vm-1".into()),
            agent: "Azure-Init/0.0.0".into(),
            supporting_data: vec![SupportingData(vec![
                ("endpoint".into(), "http://example.com".into()),
                ("status".into(), "404".into()),
            ])],
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
        assert!(value.ends_with("|endpoint=http://example.com|status=404"));
    }

    #[test]
    fn dispatch_report_success_defaults_agent() {
        let dir = TempDir::new().unwrap();
        let command = Command::ReportSuccess {
            vm_id: Some("vm-1".into()),
            agent: DEFAULT_AGENT.into(),
            supporting_data: Vec::new(),
        };
        let (code, _) = run_dispatch(cli(&dir, command));
        assert_eq!(code, EXIT_OK);

        let entries = store_at(&dir).entries().unwrap();
        let value = entries.get("PROVISIONING_REPORT").unwrap();
        assert!(value.contains(&format!("agent={DEFAULT_AGENT}")));
    }

    #[test]
    fn dispatch_report_failure_writes_single_record() {
        let dir = TempDir::new().unwrap();
        let command = Command::ReportFailure {
            vm_id: Some("vm-1".into()),
            agent: "Azure-Init/0.0.0".into(),
            reason: "boom".into(),
            documentation_url: Some("https://aka.ms/x".into()),
            supporting_data: Vec::new(),
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
    fn dispatch_report_failure_places_supporting_data_before_pps_type() {
        let dir = TempDir::new().unwrap();
        let command = Command::ReportFailure {
            vm_id: Some("vm-1".into()),
            agent: "Azure-Init/0.0.0".into(),
            reason: "boom".into(),
            documentation_url: None,
            supporting_data: vec![SupportingData(vec![(
                "details".into(),
                "bad config".into(),
            )])],
        };
        let (code, _) = run_dispatch(cli(&dir, command));
        assert_eq!(code, EXIT_OK);

        let entries = store_at(&dir).entries().unwrap();
        let value = entries.get("PROVISIONING_REPORT").unwrap();
        assert!(value.starts_with(
            "result=error|reason=boom|agent=Azure-Init/0.0.0\
|details=bad config|pps_type=None|vm_id=vm-1|timestamp="
        ));
    }

    #[test]
    fn dispatch_report_overrides_previous_record() {
        let dir = TempDir::new().unwrap();
        run_dispatch(cli(
            &dir,
            Command::ReportFailure {
                vm_id: Some("vm-1".into()),
                agent: "Azure-Init/0.0.0".into(),
                reason: "boom".into(),
                documentation_url: None,
                supporting_data: Vec::new(),
            },
        ));
        run_dispatch(cli(
            &dir,
            Command::ReportSuccess {
                vm_id: Some("vm-1".into()),
                agent: "Azure-Init/0.0.0".into(),
                supporting_data: Vec::new(),
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

        let (_, dumped) = run_dispatch(cli(&dir, dump_cmd()));
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

        let (_, dumped) = run_dispatch(cli(&dir, dump_cmd()));
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

        let (_, dumped) = run_dispatch(cli(&dir, dump_cmd()));
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

        let (_, dumped) = run_dispatch(cli(&dir, dump_cmd()));
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

        let (code, _) = run_dispatch(cli(
            &dir,
            Command::Clear {
                if_stale,
                diagnostics: false,
            },
        ));
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

        let (_, out) = run_dispatch(cli_json(&dir, dump_cmd()));
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
