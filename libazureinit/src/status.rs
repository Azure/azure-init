// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Provisioning status management for azure-init.
//!
//! This module ensures that provisioning is performed only when necessary
//! by tracking the VM ID. It stores a provisioning status file named after
//! the VM ID in a persistent location (`/var/lib/azure-init/`).
//!
//! # Logic Overview
//! - Retrieves the VM ID using reading `/sys/class/dmi/id/product_uuid` and byte-swapping if gen1 VM.
//! - Determines if provisioning is required by checking if a status file exists.
//! - Creates the provisioning status file upon successful provisioning.
//! - Prevents unnecessary re-provisioning on reboot, unless the VM ID changes.
//!
//! # Behavior
//! - On **first boot**, provisioning runs and a file is created: `/var/lib/azure-init/{vm_id}`
//! - On **reboot**, if the same VM ID exists, provisioning is skipped.
//! - If the **VM ID changes** (e.g., due to VM cloning), provisioning runs again.

use byteorder::{BigEndian, ByteOrder, LittleEndian};
use std::env;
use std::fs::{self, File};
use std::path::Path;
use uuid::Uuid;

use crate::error::Error;

/// Directory where the provisioning status files are stored.
const DEFAULT_PROVISION_DIR: &str = "/var/lib/azure-init/";

fn get_provisioning_dir() -> String {
    env::var("TEST_PROVISION_DIR")
        .unwrap_or_else(|_| DEFAULT_PROVISION_DIR.to_string())
}

/// This function checks if the provisioning directory is present, and if not,
/// it creates it.
fn check_provision_dir() -> Result<(), Error> {
    let dir = get_provisioning_dir();
    if !Path::new(&dir).exists() {
        fs::create_dir_all(&dir)?;
        tracing::info!("Created provisioning directory: {}", &dir);
    }
    Ok(())
}

/// Determines if VM is a gen1 or gen2 based on EFI detection,
/// Returns `true` if it is a Gen1 VM (i.e., not UEFI/Gen2).
fn is_vm_gen1() -> bool {
    if Path::new("/sys/firmware/efi").exists() {
        return false;
    }
    if Path::new("/dev/efi").exists() {
        return false;
    }
    true
}

/// A helper function that reorders the UUID's first three fields into little-endian.
// The standard UUID fields are: d1 (4 bytes), d2 (2 bytes), d3 (2 bytes), and d4 (8 bytes).
fn swap_uuid_to_little_endian(bytes: [u8; 16]) -> [u8; 16] {
    let d1 = BigEndian::read_u32(&bytes[0..4]);
    let d2 = BigEndian::read_u16(&bytes[4..6]);
    let d3 = BigEndian::read_u16(&bytes[6..8]);

    let d1_le = d1.swap_bytes();
    let d2_le = d2.swap_bytes();
    let d3_le = d3.swap_bytes();

    let mut swapped = [0u8; 16];
    LittleEndian::write_u32(&mut swapped[0..4], d1_le);
    LittleEndian::write_u16(&mut swapped[4..6], d2_le);
    LittleEndian::write_u16(&mut swapped[6..8], d3_le);

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
pub fn get_vm_id() -> Option<String> {
    // Test override check
    if let Ok(mock_id) = std::env::var("MOCK_VM_ID") {
        return Some(mock_id);
    }

    let system_uuid = match fs::read_to_string("/sys/class/dmi/id/product_uuid")
    {
        Ok(s) => s.trim().to_lowercase(),
        Err(err) => {
            tracing::error!(
                "Failed to read /sys/class/dmi/id/product_uuid: {}",
                err
            );
            return None;
        }
    };

    if system_uuid.is_empty() {
        tracing::info!(target: "libazureinit::status::retrieved_vm_id", "system-uuid is empty");
        return None;
    }

    if is_vm_gen1() {
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
pub fn is_provisioning_complete() -> bool {
    if let Some(vm_id) = get_vm_id() {
        let file_path =
            format!("{}/{}.provisioned", get_provisioning_dir(), vm_id);
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
pub fn mark_provisioning_complete() -> Result<(), Error> {
    check_provision_dir()?;

    if let Some(vm_id) = get_vm_id() {
        let file_path =
            format!("{}/{}.provisioned", get_provisioning_dir(), vm_id);

        if let Err(error) = File::create(&file_path) {
            tracing::error!(
                ?error,
                file_path,
                "Failed to create provisioning status file"
            );
            return Err(error.into());
        }

        tracing::info!(
            target: "libazureinit::status::success",
            "Provisioning complete. File created: {}",
            file_path
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
            "TEST_PROVISION_DIR",
            test_dir.path().to_str().unwrap(),
        );

        std::env::set_var("MOCK_VM_ID", "FAKE-VM-ID-123");

        let file_path = test_dir.path().join("FAKE-VM-ID-123.provisioned");
        assert!(
            !file_path.exists(),
            "File should not exist before provisioning"
        );
        mark_provisioning_complete().unwrap();
        assert!(file_path.exists(), "Provisioning file should be created");
    }

    #[test]
    fn test_is_provisioning_complete() {
        let test_dir = TempDir::new().unwrap();
        std::env::set_var(
            "TEST_PROVISION_DIR",
            test_dir.path().to_str().unwrap(),
        );
        std::env::set_var("MOCK_VM_ID", "FAKE-VM-ID-456");

        assert!(
            !is_provisioning_complete(),
            "Provisioning should be needed if file doesn't exist"
        );

        let file_path = test_dir.path().join("FAKE-VM-ID-456.provisioned");
        fs::File::create(&file_path).unwrap();
        assert!(
            is_provisioning_complete(),
            "Provisioning should be complete if file exists"
        );
    }

    #[test]
    fn test_provisioning_skipped_on_simulated_reboot() {
        let test_dir = TempDir::new().unwrap();
        std::env::set_var(
            "TEST_PROVISION_DIR",
            test_dir.path().to_str().unwrap(),
        );
        std::env::set_var("MOCK_VM_ID", "FAKE-VM-ID-999");
        assert!(
            !is_provisioning_complete(),
            "Should need provisioning initially"
        );

        mark_provisioning_complete().unwrap();

        // Simulate a "reboot" by calling again
        assert!(
            is_provisioning_complete(),
            "Provisioning should be skipped on second run (file exists)"
        );
    }
}
