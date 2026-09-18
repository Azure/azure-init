# KVP Diagnostics Contract

This contract defines provisioning telemetry for host-side consumers, including
EG and LPA: operations, observations and artifacts such as logs. The
[KVP contract](kvp.md) defines the underlying pool files and transport.

## Record Format

Native diagnostics are appended to guest pool 1. A diagnostic occupies one or
more records. Each key has exactly eleven pipe-delimited fields:

```text
DIAG|<agent>|<vm_id>|<kind>|<name>|<event_id>|<timestamp>|<encoding>|<result>|<duration>|<chunk_index>
```

`DIAG` selects the current format. Do not parse an unsupported diagnostic schema
using this layout. Agent versions identify the producer, not the schema.

| Field | Description | Format |
|---|---|---|
| `DIAG` | Native diagnostic format identifier | Literal `DIAG` |
| `agent` | Reporting agent, such as `azure-init/0.1.1` | UTF-8 text, conventionally `name/VERSION` |
| `vm_id` | VM identity | UUID |
| `kind` | Operation endpoint or standalone observation | `start`, `finish`, or `event` |
| `name` | Operation or observation, such as `provision:run` or `dmesg` | UTF-8 text |
| `event_id` | Shared by an operation's start and finish; unique to a standalone event | Opaque UTF-8 identifier |
| `timestamp` | Emission time | RFC 3339 timestamp |
| `encoding` | Stored value representation | `none`, `zlib+b64`, or `gz+b64` |
| `result` | Reported outcome, when applicable | `success`, `fail`, or empty |
| `duration` | Elapsed seconds, when measured | Finite nonnegative double-precision numeric text, or empty |
| `chunk_index` | Chunk position; zero for a single record | Unsigned decimal integer |

All fields except `result` and `duration` are required and nonempty. Key fields
cannot contain `|` or NUL; there is no key-field escaping.

Preserve event IDs as text: `0000000001` and `1` are distinct identifiers.

## Timing and Correlation

| Kind | Result | Duration | Meaning |
|---|---|---|---|
| `start` | Empty | Empty | An operation began |
| `finish` | Required | Required | An operation ended with a reported outcome and elapsed time |
| `event` | Optional | Optional | A standalone observation; outcome and timing are independent |

A start and finish share an event ID, agent, VM and operation name. A finish
carries its own elapsed duration to allow caller to accurately measure
the operation of interest without relying on the timestamps of the emitted
diagnostics.

### Timestamps and Durations

Timestamps must conform to RFC 3339. The writer emits UTC `Z` form with
second, millisecond (default), microsecond or nanosecond precision.

Durations are finite, nonnegative IEEE 754 double-precision seconds, in
decimal or exponent notation. Empty means absent; zero is a measured duration.
See [Implementation Notes](#implementation-notes) for reader limits and
rounding.

### Examples

An operation and a compressed observation, shown as stored key/value records:

```json
[
  {
    "key": "DIAG|azure-init/0.1.1|3f2504e0-4f89-41d3-9a0c-0305e82c3301|start|provision:run|8f3e9c4a-1b2c-4d5e-9f01-234567890abc|2026-08-31T12:34:56.789Z|none|||0",
    "value": "starting"
  },
  {
    "key": "DIAG|azure-init/0.1.1|3f2504e0-4f89-41d3-9a0c-0305e82c3301|finish|provision:run|8f3e9c4a-1b2c-4d5e-9f01-234567890abc|2026-08-31T12:34:57.101Z|none|success|0.312000|0",
    "value": "provisioning succeeded"
  },
  {
    "key": "DIAG|azure-init/0.1.1|3f2504e0-4f89-41d3-9a0c-0305e82c3301|event|example|9f3e9c4a-1b2c-4d5e-9f01-234567890abc|2026-08-31T12:34:57.102Z|zlib+b64|||0",
    "value": "eJwLSS0uUSguKcrMS1cwNDIGACxqBQ4="
  }
]
```

## Reassembly and Decoding

The producer encodes the whole payload before splitting it into records.
Group members need not be adjacent or ordered in the pool.

1. Select the schema from the first key field and validate its metadata.
2. Group by the full key excluding only `chunk_index`. Event ID alone is not
  sufficient: it would combine start and finish records.
3. Sort indices numerically and require a unique, contiguous sequence from zero.
4. Concatenate values in that order, then decode according to `encoding`.

### Encodings

| Token | Stored value | Decoded content |
|---|---|---|
| `none` | Plain UTF-8 without NUL | Text |
| `zlib+b64` | Standard padded base64 of one zlib stream (RFC 1950) | Bytes |
| `gz+b64` | Standard padded base64 of one gzip member (RFC 1952) | Bytes |

For compressed encodings, decode base64 before decompressing. Native base64 has
no whitespace. Zlib uses DEFLATE with a 32 KiB window (`wbits=15`) and no preset
dictionary; gzip uses a basic header without optional fields. The tokens are
not aliases. Compression does not imply that the decoded content is text.

## Limits

The complete key must fit in 254 UTF-8 bytes, including separators and the
chunk index. The table accounts for this writer's output with the default name
limit. Example widths use the finish record above, with default precision and
a single chunk; they are illustrative, not measured production averages.

| Field | Example bytes | Field bound (bytes) | Basis |
|---|---:|---:|---|
| `DIAG` | 4 | 4 | Fixed token |
| `agent` | 16 | 32 | `azure-init/0.1.1`; producer text limit |
| `vm_id` | 36 | 36 | Hyphenated UUID in the example; writer UUID limit |
| `kind` | 6 | 6 | `start`/`event`: 5; `finish`: 6 |
| `name` | 13 | 64 | `provision:run`; configurable producer limit, default 64 |
| `event_id` | 36 | 36 | Hyphenated UUID in the example; writer UUID limit |
| `timestamp` | 24 | 30 | UTC `Z` output: 20/24/27/30 for seconds/ms/us/ns |
| `encoding` | 4 | 8 | `none`: 4; `gz+b64`: 6; `zlib+b64`: 8 |
| `result` | 7 | 7 | Empty: 0; `fail`: 4; `success`: 7 |
| `duration` | 8 | 30 | `0.312000`; up to 20 whole-second digits, a point and 9 fractional digits |
| `chunk_index` | 1 | 4 | `0` through `1022` |
| Ten pipe separators | 10 | 10 | One byte each |
| **Total** | **165** | **267** | Sum of field widths |
| **Space remaining** | **89** | **13 over limit** | Against the 254-byte limit |

The start and compressed-event examples use 149 and 147 key bytes respectively.
The field bounds total 258 bytes with both default precisions, so not every
combination fits; oversized keys are rejected before writing. Widths count text
bytes, not the in-memory size of a double. These are writer budgets, not
universal widths for every RFC 3339 timestamp or numeric spelling.

Encoded values are limited to 1,022 bytes per chunk, with at most 1,023 chunks
per payload. Producer budgets do not impose equivalent read limits on other
producers' records, and encoded size does not bound decompressed size.

## Error Handling

Preserve records that cannot be interpreted, without silently repairing them.

There is no total-chunk-count field. Missing trailing records in a plain-text
payload cannot be detected if the remaining indices are contiguous from zero.
Compressed streams additionally allow completion and checksum validation.

## Provisioning Reports

The exact key `PROVISIONING_REPORT` identifies a separate, single-record health
report. Its value is pipe-delimited CSV whose fields contain `key=value`.
Fields containing pipes, quotes or newlines use CSV double-quote escaping;
split each decoded field on its first `=`. Field order is not significant for
reading.

| Field | Description | Format | Required |
|---|---|---|---|
| `result` | Provisioning outcome | `success` or `error` (not diagnostic `fail`) | All reports |
| `agent` | Reporting agent | UTF-8 text | All reports |
| `vm_id` | VM identity | UTF-8 text, usually a UUID | All reports |
| `pps_type` | Pre-provisioning type | `None`, `PreprovisionedOSDisk`, `Running`, `Savable`, or `Unknown` | All reports |
| `timestamp` | Report time | RFC 3339 timestamp | All reports |
| `reason` | Failure explanation | UTF-8 text | Error reports |
| `documentation_url` | Help link for a failure | URL text | No; optional for error reports |
| Other fields | Supporting data | `key=value` text fields | No |

Other fields are ordered supporting data, including duplicate supporting-data
keys. Required fields cannot be duplicated. On error reports, `reason` and
`documentation_url` cannot be duplicated either. Empty text values remain
supported. Report identities and timestamp spellings are preserved; diagnostic
UUID validation and timestamp output formatting are not imposed on reports.

Reports replace the prior provisioning result, are not chunked, and must fit
one value.

## Cloud-init Compatibility

Cloud-init uses a separate format in the same pool. Current keys include a VM
identity that older keys omit:

```text
CLOUD_INIT|<incarnation>|<type>|<name>|<vm_id>|<event_id>[|<chunk_index>]
CLOUD_INIT|<incarnation>|<type>|<name>|<event_id>[|<chunk_index>]
```

The incarnation is numeric, and the VM and event identities are UUIDs. Values
contain `name`, `type`, `ts` and `msg`; finishes add `result` and `duration`,
and chunks add `msg_i`. The value's name and type must match the key.

Apply the grouping and index checks above to the complete cloud-init base key,
including incarnation and source type. `msg_i` must match the key index, and
other metadata must agree. Join the still-escaped `msg` fragments before
unescaping once: cloud-init can split a JSON escape sequence across records,
so individual chunks need not be valid standalone JSON.

| Normalized field | Cloud-init source |
|---|---|
| Agent | Literal `CLOUD_INIT` |
| Kind | Source `start` or `finish`; all other types become `event` |
| Name | Source name |
| VM identity | Key field when present; otherwise absent |
| Event identity | Key event UUID |
| Timestamp | `ts`, RFC 3339 with offsets allowed; convert to UTC |
| Outcome | Finish `SUCCESS` becomes `success`; `FAIL` becomes `fail` |
| Duration | Finish's numeric seconds |
| Encoding | Embedded `msg` envelope, when present |

Do not turn a finish result such as `WARN` into a success or failure; retain it
as unparsed data. Other source types become events without a normalized outcome
or duration. Negative or overflowing finish durations are invalid.

Cloud-init's [Azure producer](https://github.com/canonical/cloud-init/blob/main/cloudinit/sources/helpers/azure.py)
compresses artifacts with zlib and base64-encodes the result, but labels it
`gz+b64`. The `msg` string contains a JSON object with `encoding` and `data`
fields.

Our reader reassembles and unescapes `msg`, then base64-decodes `data`, ignoring
ASCII whitespace. It selects zlib or gzip from the compressed bytes rather than
trusting the label, so cloud-init's zlib output can be read correctly. Native
`gz+b64` records remain gzip-only.

Other envelope encodings are unsupported. Messages without an encoding envelope
are plain text.

## Implementation Notes

The following describe `libazureinit-kvp`, not requirements for a consumer's API.

### Reader Output

Each decoded group appears at its first record's position. Failed groups retain
their physical records and positions. Reads do not modify the pool or sort by
timestamp. Decoding errors are attached to preserved records:

| Condition | Error token |
|---|---|
| Unsupported `DIAG` schema | `unsupported_version` |
| Missing index zero or an index gap | `incomplete_group` |
| Repeated index | `duplicate_chunk` |
| Unsupported encoding or invalid payload | `undecodable` |
| Invalid metadata or report | `malformed` |

Unrelated keys have no decoding error. Invalid pool framing or UTF-8 fails the
whole read. Grouping uses memory proportional to input and decoded content;
there is no decoded-size cap.

Parsed JSON represents decoded binary payloads as base64 objects. Those bytes
are already decompressed; do not send them to a decompressor again.

Timestamps normalize to UTC, discarding digits beyond nanoseconds. Durations
are limited to unsigned 64-bit whole seconds plus nanoseconds. Unsigned decimals
with up to nine fractional digits remain exact; other forms round to the nearest
nanosecond. Out-of-range values fail decoding; large durations may lose precision
in JSON.

Cloud-init durations round to microseconds; source encoding labels are preserved.

### Writer Choices

The writer requires UUID event IDs. The name limit defaults to 64 UTF-8 bytes
and is configurable with `DiagnosticWriter::with_max_name_bytes`; oversized
names are rejected, not truncated. Durations use fixed-point seconds with
microsecond precision by default, independently of timestamp precision;
finer digits are discarded.

Validation precedes writing, but I/O failure may leave a partial batch. Pool
cleanup is explicit. The complete key, including each chunk index, must fit
the [budget](#limits) regardless of the configured name limit.

Report writers emit success fields as `result`, `agent`, `pps_type`, `vm_id`,
`timestamp`, then extras. Failure order is `result`, `reason`, `agent`, extras,
`pps_type`, `vm_id`, `timestamp`, then the optional documentation URL. Consumers
must not depend on that order.