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
//! - The azure-init data directory is configurable via the Config struct (defaulting to `/var/lib/azure-init/`).
//! - Creates the provisioning status file upon successful provisioning.
//! - Prevents unnecessary re-provisioning on reboot, unless the VM ID changes.
//!
//! # Behavior
//! - On **first boot**, provisioning runs and a file is created: `/var/lib/azure-init/{vm_id}`
//! - On **reboot**, if the same VM ID exists, provisioning is skipped.
//! - If the **VM ID changes** (e.g., due to VM cloning), provisioning runs again.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::{Config, DEFAULT_AZURE_INIT_DATA_DIR};
use crate::error::Error;

/// This function determines the effective provisioning directory.
///
/// If a [`Config`] is provided, this function returns `config.azure_init_data_dir.path`.
/// Otherwise, it falls back to the default `/var/lib/azure-init/`.
fn get_provisioning_dir(config: Option<&Config>) -> PathBuf {
    config
        .map(|cfg| cfg.azure_init_data_dir.path.clone())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_AZURE_INIT_DATA_DIR))
}

/// This function checks if the azure-init data directory is present, and if not,
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

/// Converts the first three fields of a 16-byte array from big-endian to
/// the native endianness, then returns it as a `Uuid`.
///
/// This partially swaps the UUID so that d1 (4 bytes), d2 (2 bytes), and d3 (2 bytes)
/// are converted from big-endian to the local endianness, leaving the final 8 bytes as-is.
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
    uuid::Uuid::from_bytes(bytes)
}

/// Retrieves the VM ID by reading `/sys/class/dmi/id/product_uuid` and byte-swapping if Gen1.
///
/// The VM ID is a unique system identifier that persists across reboots but changes
/// when a VM is cloned or redeployed.
///
/// # Returns
/// - `Some(String)` containing the VM ID if retrieval is successful.
/// - `None` if something fails or the output is empty.
pub fn get_vm_id() -> Option<String> {
    private_get_vm_id(None, None)
}

fn private_get_vm_id(
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
                let swapped_uuid =
                    swap_uuid_to_little_endian(*uuid_parsed.as_bytes());
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

/// Checks whether a provisioning status file exists for the current VM ID.
///
/// If the provisioning status file exists, it indicates that provisioning has already been
/// completed, and the process should be skipped. If the file does not exist or the VM ID has
/// changed, provisioning should proceed.
///
/// # Parameters
/// - `config`: An optional configuration reference used to determine the provisioning directory.
///   If `None`, the default provisioning directory defined by `DEFAULT_AZURE_INIT_DATA_DIR` is used.
///
/// # Returns
/// - `true` if provisioning is complete (i.e., the provisioning file exists).
/// - `false` if provisioning has not been completed (i.e., no provisioning file exists).
pub fn is_provisioning_complete(config: Option<&Config>) -> bool {
    private_is_provisioning_complete(config, None)
}

fn private_is_provisioning_complete(
    config: Option<&Config>,
    vm_id: Option<String>,
) -> bool {
    let vm_id = vm_id.or_else(get_vm_id);

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

/// Marks provisioning as complete by creating a provisioning status file.
///
/// This function ensures that the provisioning directory exists, retrieves the VM ID,
/// and creates a `{vm_id}.provisioned` file in the provisioning directory.
///
/// # Parameters
/// - `config`: An optional configuration reference used to determine the provisioning directory.
///   If `None`, the default provisioning directory defined by `DEFAULT_AZURE_INIT_DATA_DIR` is used.
///
/// # Returns
/// - `Ok(())` if the provisioning status file was successfully created.
/// - `Err(Error)` if an error occurred while creating the provisioning file.
pub fn mark_provisioning_complete(
    config: Option<&Config>,
) -> Result<(), Error> {
    private_mark_provisioning_complete(config, None)
}

fn private_mark_provisioning_complete(
    config: Option<&Config>,
    vm_id: Option<String>,
) -> Result<(), Error> {
    check_provision_dir(config)?;

    let vm_id = vm_id.or_else(get_vm_id);

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

    /// Creates a temporary directory and returns a default `Config`
    /// whose `azure_init_data_dir` points to that temp directory.
    /// Also returns the `TempDir` so it remains in scope for the test.
    fn create_test_config() -> (Config, TempDir) {
        let test_dir = TempDir::new().unwrap();

        let mut test_config = Config::default();
        test_config.azure_init_data_dir.path = test_dir.path().to_path_buf();

        (test_config, test_dir)
    }

    #[test]
    fn test_mark_provisioning_complete() {
        let (test_config, test_dir) = create_test_config();

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        let vm_id =
            private_get_vm_id(Some(mock_vm_id_path.to_str().unwrap()), None)
                .unwrap();

        let file_path = test_dir.path().join(format!("{}.provisioned", vm_id));
        assert!(
            !file_path.exists(),
            "File should not exist before provisioning"
        );

        private_mark_provisioning_complete(
            Some(&test_config),
            Some(vm_id.clone()),
        )
        .unwrap();
        assert!(file_path.exists(), "Provisioning file should be created");
    }

    #[test]
    fn test_is_provisioning_complete() {
        let (test_config, test_dir) = create_test_config();

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440001")
            .unwrap();

        let vm_id =
            private_get_vm_id(Some(mock_vm_id_path.to_str().unwrap()), None)
                .unwrap();

        let file_path = test_dir.path().join(format!("{}.provisioned", vm_id));
        fs::File::create(&file_path).unwrap();

        assert!(
            private_is_provisioning_complete(
                Some(&test_config),
                Some(vm_id.clone())
            ),
            "Provisioning should be complete if file exists"
        );
    }

    #[test]
    fn test_provisioning_skipped_on_simulated_reboot() {
        let (test_config, test_dir) = create_test_config();

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440002")
            .unwrap();

        let vm_id =
            private_get_vm_id(Some(mock_vm_id_path.to_str().unwrap()), None)
                .unwrap();

        assert!(
            !private_is_provisioning_complete(
                Some(&test_config),
                Some(vm_id.clone())
            ),
            "Should need provisioning initially"
        );

        private_mark_provisioning_complete(
            Some(&test_config),
            Some(vm_id.clone()),
        )
        .unwrap();

        // Simulate a "reboot" by calling again
        assert!(
            private_is_provisioning_complete(
                Some(&test_config),
                Some(vm_id.clone())
            ),
            "Provisioning should be skipped on second run (file exists)"
        );
    }

    #[test]
    fn test_get_vm_id_mocked_gen1_vs_gen2() {
        let test_dir = TempDir::new().unwrap();

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();

        let mock_efi_path = test_dir.path().join("mock_efi_file");

        // Simulate Gen1: don't create the mock EFI file => it doesn't exist => is_vm_gen1() returns true
        let vm_id_gen1 = private_get_vm_id(
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

        let vm_id_gen2 = private_get_vm_id(
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
