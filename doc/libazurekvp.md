# `libazureinit-kvp`

`libazureinit-kvp` is the storage layer for Hyper-V KVP (Key-Value Pair)
pool files used by Azure guests.

It defines:
- `KvpStore`: storage trait with explicit read/write/delete semantics.
- `HyperVKvpStore`: production implementation backed by the Hyper-V
  binary pool file format.
- `KvpLimits`: exported key/value byte limits for Hyper-V and Azure.

## Record Format

The Hyper-V pool file record format is fixed width:
- Key field: 512 bytes
- Value field: 2048 bytes
- Total record size: 2560 bytes

Records are appended to the file and zero-padded to fixed widths.

## Store Semantics

### `write(key, value)`

- Append-only behavior: each call appends one new record.
- Duplicate keys are allowed in the file.
- Returns an error when:
  - key is empty
  - key byte length exceeds `max_key_size`
  - value byte length exceeds `max_value_size`
  - an I/O error occurs
- Oversized values are rejected by the store (no silent truncation).
  Higher layers are responsible for chunking/splitting when required.

### `read(key)`

- Scans records and returns the value from the most recent matching key
  (last-write-wins).
- Returns `Ok(None)` when the key is missing or file does not exist.

### `entries()`

- Returns `HashMap<String, String>`.
- Deduplicates duplicate keys using last-write-wins, matching `read`.
- This exposes a logical unique-key view even though the file itself is
  append-only and may contain multiple records per key.

### `delete(key)`

- Rewrites the file without any matching key records.
- Returns `true` if at least one record was removed, else `false`.

## Truncate Semantics (`truncate_if_stale`)

`HyperVKvpStore::truncate_if_stale` clears stale records from previous
boots by comparing file `mtime` to the current boot timestamp.

- If file predates boot: truncate to zero length.
- If file is current: leave unchanged.
- If lock contention occurs (`WouldBlock`): return `Ok(())` and skip.
- Non-contention lock failures are returned as errors.

## Limits and Azure Compatibility

`KvpLimits` is exported so callers (including diagnostics layers) can
enforce and reuse exact bounds.

- `KvpLimits::hyperv()`
  - `max_key_size = 512`
  - `max_value_size = 2048`
- `KvpLimits::azure()`
  - `max_key_size = 512`
  - `max_value_size = 1022` (UTF-16: 511 characters + null terminator)

Why Azure limit is lower for values:
- Hyper-V record format allows 2048-byte values.
- Azure host handling is stricter; values beyond 1022 bytes are
  silently truncated by host-side consumers.
- For Azure VMs, use `KvpLimits::azure()` and rely on higher-level
  chunking when larger payloads must be preserved.

## Record Count Behavior

There is no explicit record-count cap in this storage layer.
The file grows with each append until external constraints (disk space,
retention policy, or caller behavior) are applied.

## References

- [Hyper-V Data Exchange Service (KVP)](https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/reference/integration-services#hyper-v-data-exchange-service-kvp)