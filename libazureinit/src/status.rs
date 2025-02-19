// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Provisioning status management for azure-init.
//!
//! This module ensures that provisioning is performed only when necessary
//! by tracking the VM ID. It stores a provisioning status file named after
//! the VM ID in a persistent location (`/var/lib/azure-init/`).
//!
//! # Logic Overview
//! - Retrieves the VM ID using reading `/sys/class/dmi/id/product_uuid` and byte-swapping if Gen1 VM.
//! - Determines if provisioning is required by checking if a status file exists.
//! - The provisioning directory is configurable via the Config struct (defaulting to `/var/lib/azure-init/`),
//!   but can be overridden via environment variables for testing.
//! - Creates the provisioning status file upon successful provisioning.
//! - Prevents unnecessary re-provisioning on reboot, unless the VM ID changes.
//!
//! # Behavior
//! - On **first boot**, provisioning runs and a file is created: `/var/lib/azure-init/{vm_id}`
//! - On **reboot**, if the same VM ID exists, provisioning is skipped.
//! - If the **VM ID changes** (e.g., due to VM cloning), provisioning runs again.

use byteorder::{BigEndian, ByteOrder};
use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::Config;
use crate::error::Error;

/// This function first checks if the environment variable `AZURE_INIT_TEST_PROVISIONING_DIR`
/// is set (used primarily in tests). If set, it returns that directory. Otherwise, if a Config
/// is provided, it returns the directory specified by `config.provisioning_dir.path`. If no Config
/// is provided, it falls back to the default path `/var/lib/azure-init/`.
fn get_provisioning_dir(config: Option<&Config>) -> PathBuf {
    if let Ok(env_dir) = env::var("AZURE_INIT_TEST_PROVISIONING_DIR") {
        return PathBuf::from(env_dir);
    }

    // If config is provided, use config.provisioning_dir.path
    // Otherwise, fallback to /var/lib/azure-init/.
    config
        .map(|cfg| cfg.provisioning_dir.path.clone())
        .unwrap_or_else(|| PathBuf::from("/var/lib/azure-init/"))
}

/// This function checks if the provisioning directory is present, and if not,
/// it creates it.
fn check_provision_dir(config: Option<&Config>) -> Result<(), Error> {
    let dir = get_provisioning_dir(config);
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
        tracing::info!("Created provisioning directory: {}", dir.display());
    }
    Ok(())
}

/// Determines if VM is a gen1 or gen2 based on EFI detection,
/// Returns `true` if it is a Gen1 VM (i.e., not UEFI/Gen2).
///
/// # Parameters:
/// - `mock_efi_path` (optional): Used in **tests** to override the default EFI detection path.
///   - If provided, the function checks if the file exists:
///     - If **the file exists**, it simulates Gen2 (`false`).
///     - If **the file does not exist**, it simulates Gen1 (`true`).
///   - If `None` is provided, it defaults to **checking real system paths** (`/sys/firmware/efi` and `/dev/efi`).
fn is_vm_gen1(mock_efi_path: Option<&str>) -> bool {
    if let Some(path) = mock_efi_path {
        !Path::new(path).exists()
    } else {
        if Path::new("/sys/firmware/efi").exists()
            || Path::new("/dev/efi").exists()
        {
            return false;
        }
        true
    }
}

/// Converts a UUID from big-endian to little-endian format, as required for Gen1 VMs.
/// # Swap Behavior:
/// - The **first three fields** (`d1`, `d2`, `d3`) are **byte-swapped** individually.
/// - The **last 8 bytes (`d4`) remain unchanged**.
fn swap_uuid_to_little_endian(bytes: [u8; 16]) -> [u8; 16] {
    let d1 = BigEndian::read_u32(&bytes[0..4]);
    let d2 = BigEndian::read_u16(&bytes[4..6]);
    let d3 = BigEndian::read_u16(&bytes[6..8]);

    let d1_le = d1.swap_bytes();
    let d2_le = d2.swap_bytes();
    let d3_le = d3.swap_bytes();

    let mut swapped = [0u8; 16];

    swapped[0] = (d1_le >> 24) as u8;
    swapped[1] = (d1_le >> 16) as u8;
    swapped[2] = (d1_le >> 8) as u8;
    swapped[3] = (d1_le) as u8;

    swapped[4] = (d2_le >> 8) as u8;
    swapped[5] = (d2_le) as u8;

    swapped[6] = (d3_le >> 8) as u8;
    swapped[7] = (d3_le) as u8;

    swapped[8..16].copy_from_slice(&bytes[8..16]);

    swapped
}

/// Retrieves the VM ID by reading `/sys/class/dmi/id/product_uuid` and byte-swapping if Gen1.
///
/// The VM ID is a unique system identifier that persists across reboots but changes
/// when a VM is cloned or redeployed.
///
/// # Returns
/// - `Some(String)` containing the VM ID if retrieval is successful.
/// - `None` if something fails or the output is empty.
pub fn get_vm_id(
    custom_path: Option<&str>,
    mock_efi_path: Option<&str>,
) -> Option<String> {
    // Test override check
    let path = custom_path.unwrap_or("/sys/class/dmi/id/product_uuid");

    let system_uuid = match fs::read_to_string(path) {
        Ok(s) => s.trim().to_lowercase(),
        Err(err) => {
            tracing::error!("Failed to read VM ID from {}: {}", path, err);
            return None;
        }
    };

    if system_uuid.is_empty() {
        tracing::info!(target: "libazureinit::status::retrieved_vm_id", "VM ID file is empty at path: {}", path);
        return None;
    }

    if is_vm_gen1(mock_efi_path) {
        match Uuid::parse_str(&system_uuid) {
            Ok(uuid_parsed) => {
                let original_bytes = uuid_parsed.as_bytes();
                let swapped_bytes = swap_uuid_to_little_endian(*original_bytes);
                let swapped_uuid = Uuid::from_bytes(swapped_bytes);
                let final_id = swapped_uuid.to_string();
                tracing::info!(target: "libazureinit::status::retrieved_vm_id", "VM ID (Gen1, swapped): {}", final_id);
                Some(final_id)
            }
            Err(e) => {
                tracing::error!(
                    "Failed to parse system UUID '{}': {}",
                    system_uuid,
                    e
                );
                // fallback to the raw string
                Some(system_uuid)
            }
        }
    } else {
        tracing::info!(target: "libazureinit::status::retrieved_vm_id", "VM ID (Gen2, no swap): {}", system_uuid);
        Some(system_uuid)
    }
}

/// This function checks whether a provisioning status file exists for the current VM ID.
/// If the file exists, provisioning has already been completed and should be skipped.
/// If the file does not exist or the VM ID has changed, provisioning should proceed.
///
/// - Returns `true` if provisioning is complete (i.e., provisioning file exists).
/// - Returns `false` if provisioning has not been completed (i.e., no provisioning file exists).
pub fn is_provisioning_complete(
    config: Option<&Config>,
    vm_id: Option<String>,
) -> bool {
    let vm_id = vm_id.or_else(|| get_vm_id(None, None));

    if let Some(vm_id) = vm_id {
        let file_path =
            get_provisioning_dir(config).join(format!("{}.provisioned", vm_id));
        if std::path::Path::new(&file_path).exists() {
            tracing::info!("Provisioning already complete. Skipping...");
            return true;
        }
    }
    tracing::info!("Provisioning required.");
    false
}

/// This function creates an empty file named after the current VM ID in the
/// provisioning directory. The presence of this file signals that provisioning
/// has been successfully completed.
pub fn mark_provisioning_complete(
    config: Option<&Config>,
    vm_id: Option<String>,
) -> Result<(), Error> {
    check_provision_dir(config)?;

    let vm_id = vm_id.or_else(|| get_vm_id(None, None));

    if let Some(vm_id) = vm_id {
        let file_path =
            get_provisioning_dir(config).join(format!("{}.provisioned", vm_id));

        if let Err(error) = File::create(&file_path) {
            tracing::error!(
                ?error,
                file_path=?file_path,
                "Failed to create provisioning status file"
            );
            return Err(error.into());
        }

        tracing::info!(
            target: "libazureinit::status::success",
            "Provisioning complete. File created: {}",
            file_path.display()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_mark_provisioning_complete() {
        let test_dir = TempDir::new().unwrap();
        std::env::set_var(
            "AZURE_INIT_TEST_PROVISIONING_DIR",
            test_dir.path().to_str().unwrap(),
        );

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        let vm_id =
            get_vm_id(Some(mock_vm_id_path.to_str().unwrap()), None).unwrap();

        let file_path = test_dir.path().join(format!("{}.provisioned", vm_id));
        assert!(
            !file_path.exists(),
            "File should not exist before provisioning"
        );

        mark_provisioning_complete(None, Some(vm_id.clone())).unwrap();
        assert!(file_path.exists(), "Provisioning file should be created");
    }
    #[test]
    fn test_is_provisioning_complete() {
        let test_dir = TempDir::new().unwrap();
        std::env::set_var(
            "AZURE_INIT_TEST_PROVISIONING_DIR",
            test_dir.path().to_str().unwrap(),
        );

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440001")
            .unwrap();

        let vm_id =
            get_vm_id(Some(mock_vm_id_path.to_str().unwrap()), None).unwrap();
        let file_path = test_dir.path().join(format!("{}.provisioned", vm_id));
        fs::File::create(&file_path).unwrap();

        assert!(
            is_provisioning_complete(None, Some(vm_id.clone())),
            "Provisioning should be complete if file exists"
        );
    }

    #[test]
    fn test_provisioning_skipped_on_simulated_reboot() {
        let test_dir = TempDir::new().unwrap();
        std::env::set_var(
            "AZURE_INIT_TEST_PROVISIONING_DIR",
            test_dir.path().to_str().unwrap(),
        );
        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440002")
            .unwrap();
        let vm_id =
            get_vm_id(Some(mock_vm_id_path.to_str().unwrap()), None).unwrap();
        assert!(
            !is_provisioning_complete(None, Some(vm_id.clone())),
            "Should need provisioning initially"
        );

        mark_provisioning_complete(None, Some(vm_id.clone())).unwrap();

        // Simulate a "reboot" by calling again
        assert!(
            is_provisioning_complete(None, Some(vm_id.clone())),
            "Provisioning should be skipped on second run (file exists)"
        );
    }

    #[test]
    fn test_get_vm_id_mocked_gen1_vs_gen2() {
        let test_dir = TempDir::new().unwrap();
        std::env::set_var(
            "AZURE_INIT_TEST_PROVISIONING_DIR",
            test_dir.path().to_str().unwrap(),
        );

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();

        let mock_efi_path = test_dir.path().join("mock_efi_file");

        // Simulate Gen1: don't create the mock EFI file => it doesn't exist => is_vm_gen1() returns true
        let vm_id_gen1 = get_vm_id(
            Some(mock_vm_id_path.to_str().unwrap()),
            Some(mock_efi_path.to_str().unwrap()),
        )
        .unwrap();

        assert_eq!(
            vm_id_gen1, "00840e55-9be2-d441-a716-446655440000",
            "Should byte-swap for Gen1"
        );

        // Simulate Gen2: create the mock EFI file => is_vm_gen1() sees it => returns false
        fs::File::create(&mock_efi_path).unwrap();

        let vm_id_gen2 = get_vm_id(
            Some(mock_vm_id_path.to_str().unwrap()),
            Some(mock_efi_path.to_str().unwrap()),
        )
        .unwrap();

        assert_eq!(
            vm_id_gen2, "550e8400-e29b-41d4-a716-446655440000",
            "Should NOT byte-swap for Gen2"
        );
    }
}
