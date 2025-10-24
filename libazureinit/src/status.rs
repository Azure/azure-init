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

use fs2::FileExt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::{Config, DEFAULT_AZURE_INIT_DATA_DIR};
use crate::error::Error;

/// This function determines the effective provisioning directory.
///
/// If a [`Config`] is provided, this function returns `config.azure_init_data_dir.path`.
/// Otherwise, it falls back to the default `/var/lib/azure-init/`.
pub fn get_provisioning_dir(config: Option<&Config>) -> PathBuf {
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

        if let Err(e) =
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        {
            tracing::warn!(
                "Failed to set permissions on {}: {}",
                dir.display(),
                e
            );
        } else {
            tracing::info!(
                "Set secure permissions (700) on provisioning directory: {}",
                dir.display()
            );
        }
    }

    Ok(())
}

/// Determines if VM is a gen1 or gen2 based on EFI detection,
/// Returns `true` if it is a Gen1 VM (i.e., not UEFI/Gen2).
///
/// # Parameters:
/// * `sysfs_efi_path` - An optional override for the default EFI path (`/sys/firmware/efi`).
/// * `dev_efi_path`   - An optional override for the default EFI device path (`/dev/efi`).
///
/// If both parameters are `None`, the function checks the real system paths:
/// `/sys/firmware/efi` and `/dev/efi`.
fn is_vm_gen1(
    sysfs_efi_path: Option<&str>,
    dev_efi_path: Option<&str>,
) -> bool {
    let sysfs_efi = sysfs_efi_path.unwrap_or("/sys/firmware/efi");
    let dev_efi = dev_efi_path.unwrap_or("/dev/efi");

    // If *either* efi path exists, this is Gen2; if *neither* exist, Gen1
    // (equivalent to `!(exists(sysfs_efi) || exists(dev_efi))`)
    !Path::new(sysfs_efi).exists() && !Path::new(dev_efi).exists()
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
        tracing::info!(target: "libazureinit::status::retrieved_vm_id", "VM ID file is empty at path: {}", path);
        return None;
    }

    if is_vm_gen1(sysfs_efi_path, dev_efi_path) {
        match Uuid::parse_str(&system_uuid) {
            Ok(uuid_parsed) => {
                let swapped_uuid =
                    swap_uuid_to_little_endian(*uuid_parsed.as_bytes());
                let final_id = swapped_uuid.to_string();
                tracing::info!(
                    target: "libazureinit::status::retrieved_vm_id",
                    "VM ID (Gen1, swapped): {}",
                    final_id
                );
                Some(final_id)
            }
            Err(e) => {
                tracing::error!(
                    "Failed to parse system UUID '{}': {}",
                    system_uuid,
                    e
                );
                Some(system_uuid)
            }
        }
    } else {
        tracing::info!(
            target: "libazureinit::status::retrieved_vm_id",
            "VM ID (Gen2, no swap): {}",
            system_uuid
        );
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
pub fn is_provisioning_complete(config: Option<&Config>, vm_id: &str) -> bool {
    let file_path =
        get_provisioning_dir(config).join(format!("{vm_id}.provisioned"));

    if std::path::Path::new(&file_path).exists() {
        tracing::info!("Provisioning already complete. Skipping...");
        return true;
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
/// - `vm_id`: The VM ID for this provisioning instance.
///
/// # Returns
/// - `Ok(())` if the provisioning status file was successfully created.
/// - `Err(Error)` if an error occurred while creating the provisioning file.
pub fn mark_provisioning_complete(
    config: Option<&Config>,
    vm_id: &str,
) -> Result<(), Error> {
    check_provision_dir(config)?;
    let file_path =
        get_provisioning_dir(config).join(format!("{vm_id}.provisioned"));

    match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600) // Ensures correct permissions from the start
        .open(&file_path)
    {
        Ok(file) => {
            file.lock_exclusive()?;

            tracing::info!(
                target: "libazureinit::status::success",
                "Provisioning complete. File created: {}",
                file_path.display()
            );
        }
        Err(error) => {
            tracing::error!(
                ?error,
                file_path=?file_path,
                "Failed to create provisioning status file"
            );
            return Err(error.into());
        }
    }

    Ok(())
}

/// Marks provisioning as failed by creating a failure status file with the error report.
///
/// This function ensures that the provisioning directory exists and creates a
/// `{vm_id}.failed` file containing the encoded error report.
///
/// # Parameters
/// - `config`: An optional configuration reference used to determine the provisioning directory.
///   If `None`, the default provisioning directory defined by `DEFAULT_AZURE_INIT_DATA_DIR` is used.
/// - `vm_id`: The VM ID for this provisioning instance.
/// - `error_report`: The encoded error report string to write to the file.
///
/// # Returns
/// - `Ok(())` if the failure status file was successfully created.
/// - `Err(Error)` if an error occurred while creating the failure file.
pub fn mark_provisioning_failure(
    config: Option<&Config>,
    vm_id: &str,
    error_report: &str,
) -> Result<(), Error> {
    check_provision_dir(config)?;
    let file_path =
        get_provisioning_dir(config).join(format!("{vm_id}.failed"));

    match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&file_path)
    {
        Ok(mut file) => {
            use std::io::Write;

            file.lock_exclusive()?;

            writeln!(file, "{error_report}")?;
            tracing::info!(
                target: "libazureinit::status::failure",
                "Provisioning failure recorded. File created: {}",
                file_path.display()
            );
        }
        Err(error) => {
            tracing::error!(
                ?error,
                file_path=?file_path,
                "Failed to create provisioning failure file"
            );
            return Err(error.into());
        }
    }

    Ok(())
}

/// Checks if a provisioning state file has been reported to Azure.
///
/// This function reads the file and checks if it contains the "REPORTED" marker
/// on a line by itself, indicating that the provisioning status has already been
/// sent to the Azure health endpoint.
///
/// # Parameters
/// - `file_path`: The path to the provisioning state file (`.provisioned` or `.failed`).
///
/// # Returns
/// - `true` if the file contains the "REPORTED" marker.
/// - `false` if the file does not contain the marker or cannot be read.
pub fn has_been_reported(file_path: &Path) -> bool {
    fs::read_to_string(file_path)
        .map(|content| content.lines().any(|line| line.trim() == "REPORTED"))
        .unwrap_or(false)
}

/// Marks a provisioning state file as reported by appending the "REPORTED" marker.
///
/// This function appends a new line with "REPORTED" to the file, indicating that
/// the provisioning status has been successfully sent to the Azure health endpoint.
///
/// # Parameters
/// - `file_path`: The path to the provisioning state file (`.provisioned` or `.failed`).
///
/// # Returns
/// - `Ok(())` if the marker was successfully appended.
/// - `Err(Error)` if the file could not be opened or written to.
pub fn mark_reported(file_path: &Path) -> Result<(), Error> {
    let mut file = OpenOptions::new().append(true).open(file_path)?;

    file.lock_exclusive()?;

    writeln!(file, "REPORTED")?;

    tracing::info!(
        target: "libazureinit::status::reported",
        "Marked provisioning status as reported: {}",
        file_path.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::fs::{create_dir, remove_dir};
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
    fn test_gen1_vm() {
        assert!(is_vm_gen1(
            Some("/nonexistent_sysfs_efi"),
            Some("/nonexistent_dev_efi")
        ));
    }

    #[test]
    fn test_gen2_vm_with_sysfs_efi() {
        let mock_path = "/tmp/mock_efi";
        create_dir(mock_path).ok();
        assert!(!is_vm_gen1(Some(mock_path), Some("/nonexistent_dev_efi")));
        remove_dir(mock_path).ok();
    }

    #[test]
    fn test_gen2_vm_with_dev_efi() {
        let mock_path = "/tmp/mock_dev_efi";
        create_dir(mock_path).ok();
        assert!(!is_vm_gen1(Some("/nonexistent_sysfs_efi"), Some(mock_path)));
        remove_dir(mock_path).ok();
    }

    #[test]
    fn test_mark_provisioning_complete() {
        let (test_config, test_dir) = create_test_config();

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();
        let vm_id = private_get_vm_id(
            Some(mock_vm_id_path.to_str().unwrap()),
            None,
            None,
        )
        .unwrap();

        let file_path = test_dir.path().join(format!("{}.provisioned", vm_id));
        assert!(
            !file_path.exists(),
            "File should not exist before provisioning"
        );

        mark_provisioning_complete(Some(&test_config), &vm_id).unwrap();
        assert!(file_path.exists(), "Provisioning file should be created");
    }

    #[test]
    fn test_is_provisioning_complete() {
        let (test_config, test_dir) = create_test_config();

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440001")
            .unwrap();

        let vm_id = private_get_vm_id(
            Some(mock_vm_id_path.to_str().unwrap()),
            None,
            None,
        )
        .unwrap();

        let file_path = test_dir.path().join(format!("{}.provisioned", vm_id));
        fs::File::create(&file_path).unwrap();

        assert!(
            is_provisioning_complete(Some(&test_config), &vm_id,),
            "Provisioning should be complete if file exists"
        );
    }

    #[test]
    fn test_provisioning_skipped_on_simulated_reboot() {
        let (test_config, test_dir) = create_test_config();

        let mock_vm_id_path = test_dir.path().join("mock_product_uuid");
        fs::write(&mock_vm_id_path, "550e8400-e29b-41d4-a716-446655440002")
            .unwrap();

        let vm_id = private_get_vm_id(
            Some(mock_vm_id_path.to_str().unwrap()),
            None,
            None,
        )
        .unwrap();

        assert!(
            !is_provisioning_complete(Some(&test_config), &vm_id),
            "Provisioning should NOT be complete initially"
        );

        mark_provisioning_complete(Some(&test_config), &vm_id).unwrap();

        // Simulate a "reboot" by calling again
        assert!(
            is_provisioning_complete(Some(&test_config), &vm_id,),
            "Provisioning should be skipped on second run (file exists)"
        );
    }

    #[test]
    fn test_get_vm_id_gen1() {
        let tmpdir = TempDir::new().unwrap();
        let vm_uuid_path = tmpdir.path().join("product_uuid");
        fs::write(&vm_uuid_path, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();

        // No sysfs_efi or dev_efi path created => means neither exists => expect Gen1
        let res = private_get_vm_id(
            Some(vm_uuid_path.to_str().unwrap()),
            Some("/this_does_not_exist"),
            Some("/still_nope"),
        );
        assert_eq!(
            res.unwrap(),
            "00840e55-9be2-d441-a716-446655440000",
            "Should byte-swap for Gen1"
        );
    }

    #[test]
    fn test_get_vm_id_gen2() {
        let tmpdir = TempDir::new().unwrap();
        let vm_uuid_path = tmpdir.path().join("product_uuid");
        fs::write(&vm_uuid_path, "550e8400-e29b-41d4-a716-446655440000")
            .unwrap();

        // Create a mock EFI directory => at least one path exists => Gen2
        let mock_efi_dir = tmpdir.path().join("mock_efi");
        fs::create_dir(&mock_efi_dir).unwrap();

        let res = private_get_vm_id(
            Some(vm_uuid_path.to_str().unwrap()),
            Some(mock_efi_dir.to_str().unwrap()),
            None,
        );
        assert_eq!(
            res.unwrap(),
            "550e8400-e29b-41d4-a716-446655440000",
            "Should not byte-swap for Gen2"
        );
    }

    #[test]
    fn test_mark_provisioning_failure() {
        let (test_config, test_dir) = create_test_config();
        let vm_id = "00000000-0000-0000-0000-000000000000";
        let error_report =
            "result=error|reason=test_failure|vm_id=00000000-0000-0000-0000-000000000000";

        let file_path = test_dir.path().join(format!("{}.failed", vm_id));
        assert!(
            !file_path.exists(),
            "Failed file should not exist before marking"
        );

        mark_provisioning_failure(Some(&test_config), vm_id, error_report)
            .unwrap();

        assert!(file_path.exists(), "Failed file should be created");

        // Verify content
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(
            content.contains(error_report),
            "File should contain the error report"
        );
    }

    #[test]
    fn test_has_been_reported_false() {
        let tmpdir = TempDir::new().unwrap();
        let file_path = tmpdir.path().join("test.provisioned");

        // File with content but no REPORTED marker
        fs::write(&file_path, "result=success|agent=Azure-Init/test").unwrap();

        assert!(
            !has_been_reported(&file_path),
            "Should return false when REPORTED marker is absent"
        );
    }

    #[test]
    fn test_has_been_reported_true() {
        let tmpdir = TempDir::new().unwrap();
        let file_path = tmpdir.path().join("test.provisioned");

        // File with REPORTED marker
        fs::write(&file_path, "result=success|agent=Azure-Init/test\nREPORTED")
            .unwrap();

        assert!(
            has_been_reported(&file_path),
            "Should return true when REPORTED marker is present"
        );
    }

    #[test]
    fn test_has_been_reported_nonexistent_file() {
        let tmpdir = TempDir::new().unwrap();
        let file_path = tmpdir.path().join("nonexistent.provisioned");

        assert!(
            !has_been_reported(&file_path),
            "Should return false for nonexistent file"
        );
    }

    #[test]
    fn test_mark_reported() {
        let tmpdir = TempDir::new().unwrap();
        let file_path = tmpdir.path().join("test.provisioned");

        // Create file with initial content
        fs::write(&file_path, "result=success|agent=Azure-Init/test").unwrap();

        // Mark as reported
        mark_reported(&file_path).unwrap();

        // Verify REPORTED marker was added
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(
            content.contains("REPORTED"),
            "File should contain REPORTED marker"
        );
        assert!(
            content.starts_with("result=success"),
            "Original content should be preserved"
        );
    }

    // TODO: In theory, trying to mark reported
    #[test]
    fn test_mark_reported_idempotent() {
        let tmpdir = TempDir::new().unwrap();
        let file_path = tmpdir.path().join("test.provisioned");

        // Create file
        fs::write(&file_path, "result=success|agent=Azure-Init/test").unwrap();

        // Mark as reported twice
        mark_reported(&file_path).unwrap();
        mark_reported(&file_path).unwrap();

        // Verify file still valid
        assert!(has_been_reported(&file_path));
    }
}
