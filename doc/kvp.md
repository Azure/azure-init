# Hyper-V KVP (Key-Value Pair) Data Exchange

KVP is a Hyper-V integration service that lets a VM and its host
exchange small string metadata over VMBus without network
connectivity.

This crate provides the guest-side implementation: it reads and
writes KVP records in the pool files. The primary write target is
pool 1 (`KVP_POOL_GUEST`, guest → host) and the primary read
target is pool 3 (`KVP_POOL_AUTO_EXTERNAL`, host → guest).

References:
- Kernel UAPI:
  [`include/uapi/linux/hyperv.h`](https://github.com/torvalds/linux/blob/master/include/uapi/linux/hyperv.h)
- Kernel driver:
  [`drivers/hv/hv_kvp.c`](https://github.com/torvalds/linux/blob/master/drivers/hv/hv_kvp.c)
- Userspace daemon:
  [`tools/hv/hv_kvp_daemon.c`](https://github.com/torvalds/linux/blob/master/tools/hv/hv_kvp_daemon.c)
- Microsoft documentation:
  [Data Exchange: Using key-value pairs](https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/integration-services-data-exchange)

---

## Architecture

Three layers on the Linux guest move data between the application
and the Hyper-V host:

```text
 ┌──────────────────────┐
 │  Guest application   │  (azure-init / libazureinit-kvp)
 │  writes pool file    │  local file I/O only
 └──────────┬───────────┘
            │  .kvp_pool_1  (flat binary file)
 ┌──────────▼───────────┐
 │   hv_kvp_daemon      │  userspace daemon
 │   reads pool file    │  passes UTF-8 through
 └──────────┬───────────┘
            │  /dev/vmbus/hv_kvp  (chardev)
 ┌──────────▼───────────┐
 │  Kernel (hv_kvp)     │  UTF-8 ↔ UTF-16LE conversion
 │  VMBus transport     │  serialized request/response
 └──────────┬───────────┘
            │  VMBus
 ┌──────────▼───────────┐
 │  Hyper-V host        │  Azure fabric / WMI
 └──────────────────────┘
```

The guest application never talks directly to VMBus. It only
reads and writes pool files; the daemon and kernel handle transport.

---

## Pool files

Four pool files live in `/var/lib/hyperv/`, corresponding to the
kernel's `hv_kvp_exchg_pool` enum. The UAPI defines a fifth index
(`KVP_POOL_AUTO_INTERNAL` = 4) but it is undocumented and has no
pool file.

| Pool | Enum | Linux pool file | Windows registry subkey | Direction | Purpose |
|------|------|-----------------|-------------------------|-----------|---------|
| 0 | `KVP_POOL_EXTERNAL` | `.kvp_pool_0` | `Virtual Machine\External` | Host → Guest | Data pushed by host admin |
| 1 | `KVP_POOL_GUEST` | `.kvp_pool_1` | `Virtual Machine\Guest` | Guest → Host | Guest-controlled data (cloud-init, azure-init writes here) |
| 2 | `KVP_POOL_AUTO` | `.kvp_pool_2` (created but unused) | `Virtual Machine\Auto` | Guest → Host | Guest intrinsics — daemon generates values dynamically, never reads this file |
| 3 | `KVP_POOL_AUTO_EXTERNAL` | `.kvp_pool_3` | `Virtual Machine\Guest\Parameter` | Host → Guest | Host-originated data describing the host, pushed to the guest |
| 4 | `KVP_POOL_AUTO_INTERNAL` | N/A | N/A | — | Undocumented; no pool file exists |

**Pool 2 note:** When the host queries pool 2, the daemon
generates the response dynamically (hostname, IP addresses, OS
version, etc.) rather than reading from the pool file.

---

## Record format

On the VMBus wire, keys and values are null-terminated UTF-16LE
inside `struct hv_kvp_exchg_msg_value`, whose field sizes define the
constants used throughout the KVP subsystem:

```c
struct hv_kvp_exchg_msg_value {
    __u32 value_type;                              // REG_SZ (string)
    __u32 key_size;                                // actual key length
    __u32 value_size;                              // actual value length
    __u8  key[HV_KVP_EXCHANGE_MAX_KEY_SIZE];       // 512 bytes
    union {
        __u8  value[HV_KVP_EXCHANGE_MAX_VALUE_SIZE]; // 2048 bytes
        __u32 value_u32;
        __u64 value_u64;
    };
};
```

Pool files use the same field widths but store UTF-8 instead
of UTF-16LE, zero-padded to full size. Each record is 2,560 bytes
(`HV_KVP_EXCHANGE_MAX_RECORD_SIZE`):

```c
struct kvp_record {
    char key[HV_KVP_EXCHANGE_MAX_KEY_SIZE];    // 512 bytes
    char value[HV_KVP_EXCHANGE_MAX_VALUE_SIZE]; // 2048 bytes
};
```

| Limit | Value | Constant |
|-------|-------|----------|
| Key field | 512 bytes | `HV_KVP_EXCHANGE_MAX_KEY_SIZE` |
| Value field | 2,048 bytes | `HV_KVP_EXCHANGE_MAX_VALUE_SIZE` |
| Record size | 2,560 bytes | `HV_KVP_EXCHANGE_MAX_RECORD_SIZE` or  `HV_KVP_EXCHANGE_MAX_KEY_SIZE` + `HV_KVP_EXCHANGE_MAX_VALUE_SIZE` |
| Max records per file | 1,024 | `HV_KVP_EXCHANGE_MAX_RECORDS` |

---

## Pool file write behavior

Three writers touch the pool files — see comparison table below for
full details.

### Source references
- `hv_kvp_daemon`: [`kvp_update_file()`](https://github.com/torvalds/linux/blob/master/tools/hv/hv_kvp_daemon.c) (upsert + full rewrite), [`kvp_update_mem_state()`](https://github.com/torvalds/linux/blob/master/tools/hv/hv_kvp_daemon.c) (re-read before every op)
- cloud-init: [`write_key()`](https://github.com/canonical/cloud-init/blob/main/cloudinit/sources/helpers/azure.py) (append + truncate), [`_break_down()`](https://github.com/canonical/cloud-init/blob/main/cloudinit/sources/helpers/azure.py) (1,016 B diagnostic chunks)
- azure-init (current): [`encode_kvp_item()`](https://github.com/Azure/azure-init/blob/main/libazureinit/src/kvp.rs) (append + split), [`truncate_guest_pool_file()`](https://github.com/Azure/azure-init/blob/main/libazureinit/src/kvp.rs) (stale-data guard)

### Comparison

| Client | Write mode | Locking | Key limit | Value limit | Overflow | Stale guard | Null-terminated | Delete | Re-read | Pool files |
|--------|-----------|---------|-----------|-------------|----------|-------------|-----------------|--------|---------|------------|
| hv_kvp_daemon | Upsert + full rewrite | `fcntl` | 512 B (field width) | 2,048 B (field width) | N/A | None | Not checked | Yes (shift + rewrite) | Yes (`kvp_update_mem_state`) | 0–3 |
| cloud-init | Append-only | `flock()` | 512 B (field width) | 1,024 B (1,023 + null-terminator) | Truncates | Truncate if `mtime` < boot | Yes | No | No | Pool 1 only |
| azure-init (current) | Append-only, batched | `flock()` (via `fs2`) | 512 B (field width) | 1,022 B/chunk | Splits across records | Truncate if `mtime` < boot (no lock) | Zero-padded (implicit) | No | No | Pool 1 only (hardcoded) |
| libazureinit-kvp (planned) | Upsert | `flock()` + `fcntl` | Error if > 254 B | Error if > 1,022 B | Error | Option to truncate if `mtime` < boot (with lock) | Explicit null-terminator | Planned | N/A (direct file I/O) | Any pool (configurable) |

#### flock vs fcntl

On Linux, `flock()` (BSD) and `fcntl` (POSIX) are independent lock
namespaces — they do not see each other. cloud-init and azure-init
(current) use `flock()`; `hv_kvp_daemon` uses `fcntl`. This works
only because the daemon re-reads the entire pool file before every
operation, so it picks up external writes regardless of lock type.

---

## Data flow and encoding

Writing to a pool file and the host reading that data are
completely decoupled. A write touches only the pool file — no
VMBus activity is triggered. The host retrieves data later, on its
own schedule.

The kernel is the sole encoding conversion point (UTF-8 ↔ UTF-16LE).
Field widths are 512 bytes (key) and 2,048 bytes (value) in
both pool files and on the wire — only the encoding differs.

*CU = UTF-16 code unit (2 bytes). For ASCII, 1 CU = 1 character.*

### Guest write (app → pool file)

The guest application writes directly to the pool file. No VMBus
or kernel involvement at this stage.

```text
 app (UTF-8)  ──►  .kvp_pool_1
 512 + 2048 B      UTF-8, zero-padded
```

Example: key = `myKey`, value = `myValue`

| Stage | Where | Encoding | Key | Value |
|-------|-------|----------|-----|-------|
| 1. App writes pool file | `.kvp_pool_1` | UTF-8, zero-padded | `myKey\0…` (512 B) | `myValue\0…` (2048 B) |

### Host read (host reads guest pool file)

When the host wants guest data, it sends a request over VMBus.
The daemon looks up the key in the pool file and the kernel encodes
the response back to UTF-16LE.

```text
 host request  ──►  kernel  ──►  daemon  ──►  pool file  ──►  daemon  ──►  kernel  ──►  host
 (UTF-16LE)         decode       lookup        (UTF-8)         respond      encode       (UTF-16LE)
                                                                            ⚠ key capped at 254 CU, value at 1,022 CU
```

Effective limits: key ≤ 254 UTF-8 bytes, value ≤ 1,022 UTF-8
bytes. Beyond this the kernel silently truncates. Invalid UTF-8
strings will fail the entire operation as `utf8s_to_utf16s()` will
fail.

Example: key = `myKey`, value = `myValue`

| Stage | Where | Encoding | Key | Value |
|-------|-------|----------|-----|-------|
| 1. Host requests over VMBus | VMBus wire | UTF-16LE | `m\0y\0K\0e\0y\0…` | *(enumerate by index)* |
| 2. Kernel decodes for daemon | kernel → daemon | UTF-16LE → UTF-8 | `myKey\0` | — |
| 3. Daemon looks up key | `.kvp_pool_1` | UTF-8 | `myKey` | `myValue` |
| 4. Kernel encodes response | daemon → VMBus | UTF-8 → UTF-16LE | `m\0y\0K\0e\0y\0…` | `m\0y\0V\0a\0l\0u\0e\0…` |
| 5. Host receives | VMBus wire | UTF-16LE | 5 CU of 254 max (+ null-terminator) | 7 CU of 1,022 max (+ null-terminator) |

Step 4 is where the off-by-one bug caps output at 254 / 1,022 CU
instead of 255 / 1,023.

### Host write (host → guest pool file)

The host pushes data to the guest over VMBus. The kernel decodes
to UTF-8 and the daemon writes the record to the pool file.

```text
 host (UTF-16LE)  ──►  kernel (UTF-16LE→UTF-8)  ──►  daemon (UTF-8)  ──►  .kvp_pool_3
 256 + 1024 CU         utf16s_to_utf8s               pass-through         512 + 2048 B
 (incl null)           capped at MAX_*_SIZE−1+null
```

Effective limits: key ≤ 255 UTF-8 bytes + null-terminator, value ≤ 1,023
UTF-8 bytes + null-terminator (host sends max 256 / 1,024 CU including null-terminator).

Example: key = `hostKey`, value = `hostValue`

| Stage | Where | Encoding | Key | Value |
|-------|-------|----------|-----|-------|
| 1. Host sends `KVP_OP_SET` | VMBus wire | UTF-16LE | `h\0o\0s\0t\0K\0e\0y\0…` (512 B) | `h\0o\0s\0t\0V\0a\0l\0u\0e\0…` (2048 B) |
| 2. Kernel decodes for daemon | kernel → daemon | UTF-16LE → UTF-8 | `hostKey\0` | `hostValue\0` |
| 3. Daemon writes pool file | `.kvp_pool_3` | UTF-8, zero-padded | `hostKey\0…` (512 B) | `hostValue\0…` (2048 B) |
| 4. App reads pool file | `.kvp_pool_3` | UTF-8 | `hostKey` | `hostValue` |

For ASCII text the safe limits for guest-written data are 254
bytes (key) and 1,022 bytes (value). Staying within these
limits guarantees the kernel delivers the data to the host with
no truncation.

### Constants

| Constant | Value | Meaning |
|----------|-------|---------|
| `HV_KVP_EXCHANGE_MAX_KEY_SIZE` | 512 | UAPI key field width in bytes |
| `HV_KVP_EXCHANGE_MAX_VALUE_SIZE` | 2,048 | UAPI value field width in bytes |
| `HV_KVP_EXCHANGE_MAX_RECORD_SIZE` | 2,560 | Single record size (key + value) |
| `HV_KVP_EXCHANGE_MAX_RECORDS` | 1,024 | Max records per pool file |
| `HV_KVP_SAFE_MAX_UTF8_KEY_SIZE` | 255 | 254 UTF-8 bytes + null-terminator; no kernel truncation on write path |
| `HV_KVP_SAFE_MAX_UTF8_VALUE_SIZE` | 1,023 | 1,022 UTF-8 bytes + null-terminator; no kernel truncation on write path |
