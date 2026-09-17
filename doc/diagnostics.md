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

`DIAG` selects the current format; `DIAG_V*` is reserved for other schemas.
Do not parse an unsupported schema using this layout. Agent versions identify
the producer, not the schema.

| Field | Meaning |
|---|---|
| `DIAG` | Native diagnostic format identifier |
| `agent` | Reporting agent, conventionally `name/VERSION`, such as `azure-init/0.1.1` |
| `vm_id` | VM UUID |
| `kind` | `start`, `finish`, or `event` |
| `name` | Operation or observation, such as `provision:run`, `imds`, or `dmesg` |
| `event_id` | UUID shared by an operation's start and finish; a standalone event has its own UUID |
| `timestamp` | Emission time as RFC 3339 UTC with a `Z` suffix |
| `encoding` | Value representation: `none`, `zlib+b64`, or `gz+b64` |
| `result` | `success` or `fail`, when applicable |
| `duration` | Nonnegative elapsed seconds, when measured |
| `chunk_index` | Decimal chunk index starting at zero, including for a single record |

All fields except `result` and `duration` are required and nonempty. Key fields
cannot contain `|` or NUL; there is no key-field escaping. Agent strings remain
opaque to readers, including unversioned names and older naming conventions.
UUID spellings are preserved rather than rewritten during reading.

## Timing and Correlation

| Kind | Result | Duration | Meaning |
|---|---|---|---|
| `start` | Empty | Empty | An operation began |
| `finish` | Required | Required | An operation ended with a reported outcome and elapsed time |
| `event` | Optional | Optional | A standalone observation; outcome and timing are independent |

A start and finish share an event ID, agent, VM and operation name. A finish
carries its own elapsed duration; do not derive it by subtracting wall-clock
timestamps. Either endpoint remains valid if its counterpart is missing.

### Timestamps and Durations

Timestamps use RFC 3339 UTC `Z` form, with zero, three, six or nine fractional
digits. The default is milliseconds, for example `2026-08-31T12:34:56.789Z`.
Numeric offsets and other fractional widths are invalid in native records.

Durations are decimal **seconds**: `0.312000` is 312 milliseconds. Emission
defaults to six fractional digits, independently of timestamp precision.
Producers can select zero, three, six or nine digits; lower digits are discarded.

Accept decimal digits with an optional decimal point and one to nine fractional
digits. The whole-seconds component is at most `18446744073709551615`. Signs,
exponents, NaN, infinity and empty fractional parts are invalid. An empty field
means absent; `0` is a measured zero.

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

### Kusto Consumption

Use `zlib+b64` for new compressed telemetry targeting Kusto's
[zlib function](https://learn.microsoft.com/en-us/kusto/query/zlib-base64-decompress-function),
which requires window size 15. Pass the complete reassembled base64 value:

```kusto
print message = zlib_decompress_from_base64_string("eJwLSS0uUSguKcrMS1cwNDIGACxqBQ4=")
```

The result is `Test string 123`. Actual gzip data requires the separate
[gzip function](https://learn.microsoft.com/en-us/kusto/query/gzip-base64-decompress),
which does not support optional gzip header fields. Both return strings, not
arbitrary binary artifacts. Cloud-init needs the source-specific extraction
described [below](#cloud-init-compatibility).

## Limits and Invalid Records

Native producer limits count UTF-8 bytes after encoding:

| Item | Limit |
|---|---|
| Complete key, including delimiters and index | 254 bytes |
| Encoded value per chunk | 1,022 bytes |
| Chunks per payload | 1,023, indexed 0 through 1022 |
| Agent / name | 32 / 48 bytes |
| Each UUID | 36 bytes |

These budgets do not impose equivalent read limits on other producers' records.
Encoded size also does not bound decompressed size.

Invalid metadata, unsupported encodings, duplicate or missing indices, invalid
base64, corrupt or truncated compressed streams, and trailing bytes after a
completed stream prevent decoding. Preserve the affected records rather than
silently dropping or repairing them. Unrelated keys are not diagnostic errors.

There is no total-chunk-count field. Missing trailing records in a plain-text
payload cannot be detected if the remaining indices are contiguous from zero.
Compressed streams additionally allow completion and checksum validation.

## Provisioning Reports

The exact key `PROVISIONING_REPORT` identifies a separate, single-record health
report. Its value is pipe-delimited CSV whose fields contain `key=value`.
Fields containing pipes, quotes or newlines use CSV double-quote escaping;
split each decoded field on its first `=`. Field order is not significant for
reading.

Required fields are `result`, `agent`, `vm_id`, `pps_type` and an RFC 3339
`timestamp`. Report results are `success` or `error`, not the diagnostic
`success`/`fail` tokens. Error reports also require `reason` and may include
`documentation_url`. Pre-provisioning types are `None`, `PreprovisionedOSDisk`,
`Running`, `Savable` and `Unknown`.

Other fields are ordered supporting data, including duplicate supporting-data
keys. Required fields cannot be duplicated. On error reports, `reason` and
`documentation_url` cannot be duplicated either. Empty text values remain
supported. Report identities and timestamp spellings are preserved; native
diagnostic UUID and timestamp restrictions are not imposed on reports.

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

Compressed artifacts put a JSON object with `encoding` and `data` inside `msg`;
extract `data` after reassembly and unescaping.
Cloud-init's [Azure producer](https://github.com/canonical/cloud-init/blob/main/cloudinit/sources/helpers/azure.py)
uses zlib compression and line-wrapped base64 while labeling the envelope
`gz+b64`. Remove ASCII base64 whitespace and accept zlib or gzip under that
source label, using the actual stream format to select the decompressor.
This exception does not apply to native `gz+b64` records. Other envelope
encodings are unsupported; messages without an encoding envelope are text.

## Implementation Notes

The following describe `libazureinit-kvp`, not requirements for a consumer's API.

### Reader Output

Each decoded group appears at its first record's position. Failed groups retain
their physical records and positions. Reads do not modify the pool or sort by
timestamp. Decoding errors are attached to preserved records:

| Condition | Error token |
|---|---|
| Unsupported `DIAG_V*` schema | `unsupported_version` |
| Missing index zero or an index gap | `incomplete_group` |
| Repeated index | `duplicate_chunk` |
| Unsupported encoding or invalid payload | `undecodable` |
| Invalid metadata or report | `malformed` |

Unrelated keys have no decoding error. Invalid pool framing or UTF-8 fails the
whole read. Grouping uses memory proportional to input and decoded content;
there is no decoded-size cap.

Parsed JSON represents decoded binary payloads as base64 objects. Those bytes
are already decompressed; do not send them to a decompressor again. Durations
serialize as numeric seconds, with possible floating-point precision loss at
large values. Stored durations use integer arithmetic to retain the selected
precision. Cloud-init durations are rounded to microseconds during conversion,
and the source encoding label is retained.

### Writer Choices

Validation precedes writing, but I/O failure may leave a partial batch. Pool
cleanup is explicit. The maximum native key is 251 bytes, within the 254-byte
budget even at nanosecond precision and the largest supported duration:

| Key component | Maximum bytes |
|---|---:|
| `DIAG` | 4 |
| Agent | 32 |
| VM UUID | 36 |
| Kind | 6 |
| Name | 48 |
| Event UUID | 36 |
| Timestamp | 30 |
| Encoding | 8 |
| Result | 7 |
| Duration | 30 |
| Chunk index | 4 |
| Ten pipe delimiters | 10 |
| Total | 251 |

Report writers emit success fields as `result`, `agent`, `pps_type`, `vm_id`,
`timestamp`, then extras. Failure order is `result`, `reason`, `agent`, extras,
`pps_type`, `vm_id`, `timestamp`, then the optional documentation URL. Consumers
must not depend on that order.