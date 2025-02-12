// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Provisioning status management for azure-init.
//!
//! This module ensures that provisioning is performed only when necessary
//! by tracking the VM ID. It stores a provisioning status file named after
//! the VM ID in a persistent location (`/var/lib/azure-init/`).
//!
//! # Logic Overview
//! - Retrieves the VM ID using `dmidecode`.
//! - Determines if provisioning is required by checking if a status file exists.
//! - Creates the provisioning status file upon successful provisioning.
//! - Prevents unnecessary re-provisioning on reboot, unless the VM ID changes.
//!
//! # Behavior
//! - On **first boot**, provisioning runs and a file is created: `/var/lib/azure-init/{vm_id}`
//! - On **reboot**, if the same VM ID exists, provisioning is skipped.
//! - If the **VM ID changes** (e.g., due to VM cloning), provisioning runs again.

use std::env;
use std::fs::{self, File};
use std::path::Path;
use std::process::Command;

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

/// Retrieves the VM ID using `dmidecode`.
///
/// The VM ID is a unique system identifier that persists across reboots but changes
/// when a VM is cloned or redeployed.
///
/// # Returns
/// - `Some(String)` containing the VM ID if retrieval is successful.
/// - `None` if `dmidecode` fails or the output is empty.
fn get_vm_id() -> Option<String> {
    // Test override check
    if let Ok(mock_id) = std::env::var("MOCK_VM_ID") {
        return Some(mock_id);
    }

    Command::new("dmidecode")
        .args(["-s", "system-uuid"])
        .output()
        .ok()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        })
        .filter(|vm_id| !vm_id.is_empty())
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
