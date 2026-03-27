# PR: Add `libazureinit-kvp` with unified KVP pool store

## Summary

- Adds workspace member `libazureinit-kvp` in root `Cargo.toml`
- Adds `libazureinit-kvp/Cargo.toml`
- Adds `libazureinit-kvp/src/lib.rs` and `libazureinit-kvp/src/kvp_pool.rs`

## Crate Design

- One trait: `KvpStore`
- One implementation: `KvpPoolStore`

`KvpStore` splits each operation into a `backend_*` method (raw I/O,
provided by the implementor) and a public method (`write`, `read`,
`clear`) that validates inputs then delegates to the backend.

Public API:

- `write`, `read` (validate then delegate to `backend_write`/`backend_read`)
- `entries`, `entries_raw`
- `delete`, `clear`
- `is_stale`

Validation is centralized in trait default methods and policy-aware via:

- `max_key_size(&self)`
- `max_value_size(&self)`

`KvpPoolStore` is file-backed using Hyper-V KVP wire format
(fixed-size 512-byte key + 2048-byte value records), with lock-based concurrency.

## Policy and Limits

Constructor:

- `new(path: Option<PathBuf>, mode: PoolMode, truncate_on_stale: bool)`

`PoolMode`:

- `Restricted` (default): key <= 254 bytes, value <= 1022 bytes
- `Full`: key <= 512 bytes, value <= 2048 bytes

Behavior:

- Default path when `None`: `/var/lib/hyperv/.kvp_pool_1`
- Unique key cap: 1024
  - new key beyond cap is rejected
  - overwrite of existing key at cap is allowed
- `clear()` truncates the store
- `truncate_on_stale` keeps truncation caller-controlled

## Errors

`KvpError` includes explicit variants:

- `EmptyKey`
- `KeyContainsNull`
- `KeyTooLarge { max, actual }`
- `ValueTooLarge { max, actual }`
- `MaxUniqueKeysExceeded { max }`
- `Io(io::Error)`

## Testing

17 tests covering:

- restricted/full key/value boundary checks
- default and explicit path behavior
- mode getter
- unique-key cap behavior (including overwrite-at-cap and add-after-delete)
- `entries` last-write-wins and `entries_raw` duplicate preservation
- `delete`, `clear`, and stale checks
- key validation (empty and null-byte)
