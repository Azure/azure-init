// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Self-contained VM ID lookup used to auto-populate provisioning reports.
//!
//! The VM ID is read from `/sys/class/dmi/id/product_uuid` and, on Gen1 VMs,
//! the first three UUID fields are byte-swapped from big-endian to native endianness.

use std::fs;
use std::path::Path;

use uuid::Uuid;

/// Retrieves the current VM ID by reading `/sys/class/dmi/id/product_uuid`
/// and byte-swapping the result if the VM is Gen1.
///
/// # Returns
/// - `Some(String)` containing the VM ID if retrieval is successful.
/// - `None` if the file is missing, empty, or cannot be read.
pub fn get_vm_id() -> Option<String> {
    private_get_vm_id(None, None, None)
}

fn private_get_vm_id(
    product_uuid_path: Option<&str>,
    sysfs_efi_path: Option<&str>,
    dev_efi_path: Option<&str>,
) -> Option<String> {
    let path = product_uuid_path.unwrap_or("/sys/class/dmi/id/product_uuid");

    let system_uuid = match fs::read_to_string(path) {
        Ok(s) => s.trim().to_lowercase(),
        Err(err) => {
            tracing::error!("Failed to read VM ID from {}: {}", path, err);
            return None;
        }
    };

    if system_uuid.is_empty() {
        tracing::info!("VM ID file is empty at path: {}", path);
        return None;
    }

    if is_vm_gen1(sysfs_efi_path, dev_efi_path) {
        match Uuid::parse_str(&system_uuid) {
            Ok(uuid_parsed) => {
                let swapped_uuid =
                    swap_uuid_to_little_endian(*uuid_parsed.as_bytes());
                Some(swapped_uuid.to_string())
            }
            Err(err) => {
                tracing::error!("invalid VM ID UUID '{system_uuid}': {err}");
                Some(system_uuid)
            }
        }
    } else {
        Some(system_uuid)
    }
}

/// Determines whether the VM is Gen1 (i.e. not UEFI/Gen2) based on EFI
/// detection. Returns `true` when neither EFI path exists.
fn is_vm_gen1(
    sysfs_efi_path: Option<&str>,
    dev_efi_path: Option<&str>,
) -> bool {
    let sysfs_efi = sysfs_efi_path.unwrap_or("/sys/firmware/efi");
    let dev_efi = dev_efi_path.unwrap_or("/dev/efi");

    // If *either* efi path exists, this is Gen2; if *neither* exist, Gen1.
    !Path::new(sysfs_efi).exists() && !Path::new(dev_efi).exists()
}

/// Converts the first three fields of a 16-byte array from big-endian to
/// the native endianness, then returns it as a `Uuid`.
fn swap_uuid_to_little_endian(mut bytes: [u8; 16]) -> Uuid {
    let (d1, remainder) = bytes.split_at(std::mem::size_of::<u32>());
    let d1 = d1
        .try_into()
        .map(u32::from_be_bytes)
        .unwrap_or(0)
        .to_ne_bytes();

    let (d2, remainder) = remainder.split_at(std::mem::size_of::<u16>());
    let d2 = d2
        .try_into()
        .map(u16::from_be_bytes)
        .unwrap_or(0)
        .to_ne_bytes();

    let (d3, _) = remainder.split_at(std::mem::size_of::<u16>());
    let d3 = d3
        .try_into()
        .map(u16::from_be_bytes)
        .unwrap_or(0)
        .to_ne_bytes();

    let native_endian = d1.into_iter().chain(d2).chain(d3).collect::<Vec<_>>();
    debug_assert_eq!(native_endian.len(), 8);
    bytes[..native_endian.len()].copy_from_slice(&native_endian);
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn gen1_when_no_efi_paths_exist() {
        assert!(is_vm_gen1(
            Some("/nonexistent_sysfs_efi"),
            Some("/nonexistent_dev_efi")
        ));
    }

    #[test]
    fn gen2_when_sysfs_efi_exists() {
        let dir = TempDir::new().unwrap();
        let efi = dir.path().join("efi");
        fs::create_dir(&efi).unwrap();
        assert!(!is_vm_gen1(
            Some(efi.to_str().unwrap()),
            Some("/nonexistent_dev_efi")
        ));
    }

    #[test]
    fn gen2_when_dev_efi_exists() {
        let dir = TempDir::new().unwrap();
        let efi = dir.path().join("efi");
        fs::create_dir(&efi).unwrap();
        assert!(!is_vm_gen1(
            Some("/nonexistent_sysfs_efi"),
            Some(efi.to_str().unwrap())
        ));
    }

    #[test]
    fn reads_and_swaps_for_gen1() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("product_uuid");
        fs::write(&path, "550e8400-e29b-41d4-a716-446655440000").unwrap();

        let actual = private_get_vm_id(
            Some(path.to_str().unwrap()),
            Some("/nonexistent_sysfs_efi"),
            Some("/nonexistent_dev_efi"),
        )
        .unwrap();

        assert_eq!(actual, "00840e55-9be2-d441-a716-446655440000");
    }

    #[test]
    fn reads_without_swap_for_gen2() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("product_uuid");
        fs::write(&path, "550E8400-E29B-41D4-A716-446655440000").unwrap();
        let efi = dir.path().join("efi");
        fs::create_dir(&efi).unwrap();

        let actual = private_get_vm_id(
            Some(path.to_str().unwrap()),
            Some(efi.to_str().unwrap()),
            Some("/nonexistent_dev_efi"),
        )
        .unwrap();

        assert_eq!(actual, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn returns_raw_value_when_gen1_uuid_is_unparseable() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("product_uuid");
        fs::write(&path, "not-a-uuid").unwrap();

        // Gen1 (no EFI paths) but the content cannot be parsed as a UUID,
        // so the raw lowercased value is returned unchanged.
        let actual = private_get_vm_id(
            Some(path.to_str().unwrap()),
            Some("/nonexistent_sysfs_efi"),
            Some("/nonexistent_dev_efi"),
        )
        .unwrap();

        assert_eq!(actual, "not-a-uuid");
    }

    #[test]
    fn get_vm_id_public_wrapper_is_callable() {
        // Exercises the public entry point. It reads the host's
        // product_uuid if present, so the result is environment dependent;
        // we only assert that invoking it does not panic.
        let _ = get_vm_id();
    }

    #[test]
    fn returns_none_for_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("product_uuid");
        fs::write(&path, "\n").unwrap();

        assert!(private_get_vm_id(Some(path.to_str().unwrap()), None, None)
            .is_none());
    }

    #[test]
    fn returns_none_for_missing_file() {
        assert!(private_get_vm_id(
            Some("/this/path/does/not/exist"),
            None,
            None
        )
        .is_none());
    }
}
