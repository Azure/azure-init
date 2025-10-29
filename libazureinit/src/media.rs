// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This module provides functionality for handling media devices, including mounting,
//! unmounting, and reading [`OVF`] (Open Virtualization Format) environment data. It defines
//! the [`Media`] struct with state management for [`Mounted`] and [`Unmounted`] states, as well
//! as utility functions for parsing [`OVF`] environment data and retrieving mounted devices
//! with CDROM-type filesystems.
//!
//! # Overview
//!
//! The `media` module is designed to manage media devices in a cloud environment. It
//! includes functionality to mount and unmount media devices, read [`OVF`] environment data,
//! and parse the data into structured formats. This is particularly useful for provisioning
//! virtual machines with specific configurations.
//!
//! # Key Components
//!
//! - [`Media`]: A struct representing a media device, with state management for [`Mounted`] and [`Unmounted`] states.
//! - [`Mounted`] and [`Unmounted`]: Zero-sized structs used to indicate the state of a [`Media`] instance.
//! - [`parse_ovf_env`]: A function to parse [`OVF`] environment data from a string.
//! - [`mount_parse_ovf_env`]: A function to mount a media device, read its [`OVF`] environment data, and return the parsed data.
//! - [`get_mount_device`]: A function to retrieve a list of mounted devices with CDROM-type filesystems.
//!
//! [`Media`]: struct.Media.html
//! [`Mounted`]: struct.Mounted.html
//! [`Unmounted`]: struct.Unmounted.html
//! [`parse_ovf_env`]: fn.parse_ovf_env.html
//! [`mount_parse_ovf_env`]: fn.mount_parse_ovf_env.html
//! [`get_mount_device`]: fn.get_mount_device.html
//! [`OVF`]: https://www.dmtf.org/standards/ovf

use std::fs;
use std::fs::create_dir_all;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;
use serde_xml_rs::from_str;

use tracing;
use tracing::instrument;

use crate::error::Error;
use fstab::FsTab;

/// Represents a media device.
///
/// # Type Parameters
///
/// * `State` - The state of the media, either `Mounted` or `Unmounted`.
#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct Environment {
    #[serde(rename = "wa:ProvisioningSection")]
    pub provisioning_section: ProvisioningSection,
    #[serde(rename = "wa:PlatformSettingsSection")]
    pub platform_settings_section: PlatformSettingsSection,
}

/// Provisioning section of the environment configuration.
#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct ProvisioningSection {
    #[serde(rename = "wa:Version")]
    pub version: String,
    #[serde(rename = "LinuxProvisioningConfigurationSet")]
    pub linux_prov_conf_set: LinuxProvisioningConfigurationSet,
}

/// Linux provisioning configuration set.
#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct LinuxProvisioningConfigurationSet {
    #[serde(rename = "UserName")]
    pub username: String,
    #[serde(default = "default_password", rename = "UserPassword")]
    pub password: String,
    #[serde(rename = "HostName")]
    pub hostname: String,
}

/// Platform settings section of the environment configuration.
#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct PlatformSettingsSection {
    #[serde(rename = "wa:Version")]
    pub version: String,
    #[serde(rename = "PlatformSettings")]
    pub platform_settings: PlatformSettings,
}

/// Platform settings details.
#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct PlatformSettings {
    #[serde(default = "default_preprov", rename = "PreprovisionedVm")]
    pub preprovisioned_vm: bool,
    #[serde(default = "default_preprov_type", rename = "PreprovisionedVmType")]
    pub preprovisioned_vm_type: String,
}

/// Returns an empty string as the default password.
///
/// # Returns
///
/// A `String` containing an empty password.
fn default_password() -> String {
    "".to_owned()
}

/// Returns `false` as the default value for preprovisioned VM.
///
/// # Returns
///
/// A `bool` indicating that the VM is not preprovisioned.
fn default_preprov() -> bool {
    false
}

/// Returns "None" as the default type for preprovisioned VM.
///
/// # Returns
///
/// A `String` containing "None" as the default preprovisioned VM type.
fn default_preprov_type() -> String {
    "None".to_owned()
}

/// Path to the default mount device.
pub const PATH_MOUNT_DEVICE: &str = "/dev/sr0";
/// Path to the default mount point.
pub const PATH_MOUNT_POINT: &str = "/run/azure-init/media/";

/// Valid filesystems for CDROM devices.
const CDROM_VALID_FS: &[&str] = &["iso9660", "udf"];
/// Path to the mount table file.
const MTAB_PATH: &str = "/etc/mtab";

/// Retrieves a list of mounted devices with CDROM-type filesystems.
///
/// # Arguments
///
/// * `path` - Optional path to the mount table file.
///
/// # Returns
///
/// A `Result` containing a vector of device paths as strings, or an `Error`.
#[instrument(skip_all)]
pub fn get_mount_device(path: Option<&Path>) -> Result<Vec<String>, Error> {
    let fstab = FsTab::new(path.unwrap_or_else(|| Path::new(MTAB_PATH)));
    let entries = fstab.get_entries()?;

    // Retrieve the names of all devices that have cdrom-type filesystem (e.g., udf)
    let cdrom_devices = entries
        .into_iter()
        .filter_map(|entry| {
            if CDROM_VALID_FS.contains(&entry.vfs_type.as_str()) {
                Some(entry.fs_spec)
            } else {
                None
            }
        })
        .collect();

    Ok(cdrom_devices)
}

/// Represents the state of a mounted media.
#[derive(Debug)]
pub struct Mounted;

/// Represents the state of an unmounted media.
#[derive(Debug)]
pub struct Unmounted;

/// Represents a media device.
///
/// # Type Parameters
///
/// * `State` - The state of the media, either `Mounted` or `Unmounted`.
#[derive(Debug)]
pub struct Media<State = Unmounted> {
    device_path: PathBuf,
    mount_path: PathBuf,
    state: std::marker::PhantomData<State>,
}

impl Media<Unmounted> {
    /// Creates a new `Media` instance.
    ///
    /// # Arguments
    ///
    /// * `device_path` - The path to the media device.
    /// * `mount_path` - The path where the media will be mounted.
    ///
    /// # Returns
    ///
    /// A new `Media` instance in the `Unmounted` state.
    pub fn new(device_path: PathBuf, mount_path: PathBuf) -> Media<Unmounted> {
        Media {
            device_path,
            mount_path,
            state: std::marker::PhantomData,
        }
    }

    /// Mounts the media device.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `Media` instance in the `Mounted` state, or an `Error`.
    #[instrument(skip_all)]
    pub fn mount(self) -> Result<Media<Mounted>, Error> {
        create_dir_all(&self.mount_path)?;

        let metadata = fs::metadata(&self.mount_path)?;
        let permissions = metadata.permissions();
        let mut new_permissions = permissions;
        new_permissions.set_mode(0o700);
        fs::set_permissions(&self.mount_path, new_permissions)?;

        let mut command = Command::new("mount");
        command
            .arg("-o")
            .arg("ro")
            .arg(&self.device_path)
            .arg(&self.mount_path);
        crate::run(command)?;

        Ok(Media {
            device_path: self.device_path,
            mount_path: self.mount_path,
            state: std::marker::PhantomData,
        })
    }
}

impl Media<Mounted> {
    /// Unmounts the media device.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure.
    #[instrument]
    pub fn unmount(self) -> Result<(), Error> {
        let mut command = Command::new("umount");
        command.arg(self.mount_path);
        crate::run(command)?;

        let mut command = Command::new("eject");
        command.arg(self.device_path);
        crate::run(command)
    }

    /// Reads the OVF environment data to a string.
    ///
    /// # Returns
    ///
    /// A `Result` containing the OVF environment data as a string, or an `Error`.
    #[instrument(skip_all)]
    pub fn read_ovf_env_to_string(&self) -> Result<String, Error> {
        let mut file_path = self.mount_path.clone();
        file_path.push("ovf-env.xml");
        let mut file =
            File::open(file_path.to_str().unwrap_or(PATH_MOUNT_POINT))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;

        Ok(contents)
    }
}

/// Parses the OVF environment data.
///
/// # Arguments
///
/// * `ovf_body` - A string slice containing the OVF environment data.
///
/// # Returns
///
/// A `Result` containing the parsed `Environment` struct, or an `Error`.
///
/// # Example
///
/// ```
/// use libazureinit::media::parse_ovf_env;
///
/// // Example dummy OVF environment data
/// let ovf_body = r#"
/// <Environment xmlns="http://schemas.dmtf.org/ovf/environment/1"
///     xmlns:wa="http://schemas.microsoft.com/windowsazure">
///     <wa:ProvisioningSection>
///         <wa:Version>1.0</wa:Version>
///         <LinuxProvisioningConfigurationSet>
///             <UserName>myusername</UserName>
///             <UserPassword></UserPassword>
///             <DisableSshPasswordAuthentication>false</DisableSshPasswordAuthentication>
///             <HostName>myhostname</HostName>
///         </LinuxProvisioningConfigurationSet>
///     </wa:ProvisioningSection>
///     <wa:PlatformSettingsSection>
///         <wa:Version>1.0</wa:Version>
///         <PlatformSettings>
///             <PreprovisionedVm>false</PreprovisionedVm>
///             <PreprovisionedVmType>None</PreprovisionedVmType>
///         </PlatformSettings>
///     </wa:PlatformSettingsSection>
/// </Environment>
/// "#;
///
/// let environment = parse_ovf_env(ovf_body).unwrap();
/// assert_eq!(environment.provisioning_section.linux_prov_conf_set.username, "myusername");
/// assert_eq!(environment.provisioning_section.linux_prov_conf_set.password, "");
/// assert_eq!(environment.provisioning_section.linux_prov_conf_set.hostname, "myhostname");
/// assert_eq!(environment.platform_settings_section.platform_settings.preprovisioned_vm, false);
/// assert_eq!(environment.platform_settings_section.platform_settings.preprovisioned_vm_type, "None");
/// ```
#[instrument(skip_all)]
pub fn parse_ovf_env(ovf_body: &str) -> Result<Environment, Error> {
    let environment: Environment = from_str(ovf_body)?;

    if !environment
        .provisioning_section
        .linux_prov_conf_set
        .password
        .is_empty()
    {
        Err(Error::NonEmptyPassword)
    } else {
        Ok(environment)
    }
}

/// Mounts the given device, gets OVF environment data, and returns it.
///
/// # Arguments
///
/// * `dev` - A string containing the device path.
///
/// # Returns
///
/// A `Result` containing the parsed `Environment` struct, or an `Error`.
#[instrument(skip_all)]
pub fn mount_parse_ovf_env(dev: String) -> Result<Environment, Error> {
    let mount_media =
        Media::new(PathBuf::from(dev), PathBuf::from(PATH_MOUNT_POINT));
    let mounted = mount_media.mount().map_err(|e| {
        tracing::error!(error = ?e, "Failed to mount media.");
        e
    })?;

    let ovf_body = mounted.read_ovf_env_to_string()?;
    let environment = parse_ovf_env(ovf_body.as_str())?;

    mounted.unmount().map_err(|e| {
        tracing::error!(error = ?e, "Failed to remove media.");
        e
    })?;

    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_get_ovf_env_none_missing() {
        let ovf_body = r#"
        <Environment xmlns="http://schemas.dmtf.org/ovf/environment/1" 
            xmlns:oe="http://schemas.dmtf.org/ovf/environment/1" 
            xmlns:wa="http://schemas.microsoft.com/windowsazure" 
            xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"> 
            <wa:ProvisioningSection>
                <wa:Version>1.0</wa:Version>
                <LinuxProvisioningConfigurationSet xmlns="http://schemas.microsoft.com/windowsazure" 
                    xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
                    <ConfigurationSetType>LinuxProvisioningConfiguration</ConfigurationSetType>
                    <UserName>myusername</UserName>
                    <UserPassword></UserPassword>
                    <DisableSshPasswordAuthentication>false</DisableSshPasswordAuthentication>
                    <HostName>myhostname</HostName>
                </LinuxProvisioningConfigurationSet>
            </wa:ProvisioningSection>
            <wa:PlatformSettingsSection>
                <wa:Version>1.0</wa:Version>
                <PlatformSettings xmlns="http://schemas.microsoft.com/windowsazure" 
                    xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
                    <KmsServerHostname>kms.core.windows.net</KmsServerHostname>
                    <ProvisionGuestAgent>true</ProvisionGuestAgent>
                    <GuestAgentPackageName i:nil="true"/>
                    <RetainWindowsPEPassInUnattend>true</RetainWindowsPEPassInUnattend>
                    <RetainOfflineServicingPassInUnattend>true</RetainOfflineServicingPassInUnattend>
                    <PreprovisionedVm>false</PreprovisionedVm>
                    <PreprovisionedVmType>None</PreprovisionedVmType>
                    <EnableTrustedImageIdentifier>false</EnableTrustedImageIdentifier>
                </PlatformSettings>
            </wa:PlatformSettingsSection>
        </Environment>"#;

        let environment: Environment = parse_ovf_env(ovf_body).unwrap();

        assert_eq!(
            environment
                .provisioning_section
                .linux_prov_conf_set
                .username,
            "myusername"
        );
        assert_eq!(
            environment
                .provisioning_section
                .linux_prov_conf_set
                .password,
            ""
        );
        assert_eq!(
            environment
                .provisioning_section
                .linux_prov_conf_set
                .hostname,
            "myhostname"
        );
        assert_eq!(
            environment
                .platform_settings_section
                .platform_settings
                .preprovisioned_vm,
            false
        );
        assert_eq!(
            environment
                .platform_settings_section
                .platform_settings
                .preprovisioned_vm_type,
            "None"
        );
    }

    #[test]
    fn test_get_ovf_env_missing_type() {
        let ovf_body = r#"
        <Environment xmlns="http://schemas.dmtf.org/ovf/environment/1" 
            xmlns:oe="http://schemas.dmtf.org/ovf/environment/1" 
            xmlns:wa="http://schemas.microsoft.com/windowsazure" 
            xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"> 
            <wa:ProvisioningSection>
                <wa:Version>1.0</wa:Version>
                <LinuxProvisioningConfigurationSet 
                    xmlns="http://schemas.microsoft.com/windowsazure" 
                    xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
                    <ConfigurationSetType>LinuxProvisioningConfiguration</ConfigurationSetType>
                    <UserName>myusername</UserName>
                    <UserPassword></UserPassword>
                    <DisableSshPasswordAuthentication>false</DisableSshPasswordAuthentication>
                    <HostName>myhostname</HostName>
                </LinuxProvisioningConfigurationSet>
            </wa:ProvisioningSection>
            <wa:PlatformSettingsSection>
                <wa:Version>1.0</wa:Version>
                <PlatformSettings xmlns="http://schemas.microsoft.com/windowsazure" 
                    xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
                    <KmsServerHostname>kms.core.windows.net</KmsServerHostname>
                    <ProvisionGuestAgent>true</ProvisionGuestAgent>
                    <GuestAgentPackageName i:nil="true"/>
                    <RetainWindowsPEPassInUnattend>true</RetainWindowsPEPassInUnattend>
                    <RetainOfflineServicingPassInUnattend>true</RetainOfflineServicingPassInUnattend>
                    <PreprovisionedVm>false</PreprovisionedVm>
                    <EnableTrustedImageIdentifier>false</EnableTrustedImageIdentifier>
                </PlatformSettings>
            </wa:PlatformSettingsSection>
        </Environment>"#;

        let environment: Environment = parse_ovf_env(ovf_body).unwrap();

        assert_eq!(
            environment
                .provisioning_section
                .linux_prov_conf_set
                .username,
            "myusername"
        );
        assert_eq!(
            environment
                .provisioning_section
                .linux_prov_conf_set
                .password,
            ""
        );
        assert_eq!(
            environment
                .provisioning_section
                .linux_prov_conf_set
                .hostname,
            "myhostname"
        );
        assert_eq!(
            environment
                .platform_settings_section
                .platform_settings
                .preprovisioned_vm,
            false
        );
        assert_eq!(
            environment
                .platform_settings_section
                .platform_settings
                .preprovisioned_vm_type,
            "None"
        );
    }

    #[test]
    fn test_get_ovf_env_password_provided() {
        let ovf_body = r#"
        <Environment xmlns="http://schemas.dmtf.org/ovf/environment/1" 
            xmlns:oe="http://schemas.dmtf.org/ovf/environment/1" 
            xmlns:wa="http://schemas.microsoft.com/windowsazure" 
            xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"> 
            <wa:ProvisioningSection>
                <wa:Version>1.0</wa:Version>
                <LinuxProvisioningConfigurationSet xmlns="http://schemas.microsoft.com/windowsazure" 
                    xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
                    <ConfigurationSetType>LinuxProvisioningConfiguration</ConfigurationSetType>
                    <UserName>myusername</UserName>
                    <UserPassword>mypassword</UserPassword>
                    <DisableSshPasswordAuthentication>false</DisableSshPasswordAuthentication>
                    <HostName>myhostname</HostName>
                </LinuxProvisioningConfigurationSet>
            </wa:ProvisioningSection>
            <wa:PlatformSettingsSection>
                <wa:Version>1.0</wa:Version>
                <PlatformSettings xmlns="http://schemas.microsoft.com/windowsazure" 
                    xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
                    <KmsServerHostname>kms.core.windows.net</KmsServerHostname>
                    <ProvisionGuestAgent>true</ProvisionGuestAgent>
                    <GuestAgentPackageName i:nil="true"/>
                    <RetainWindowsPEPassInUnattend>true</RetainWindowsPEPassInUnattend>
                    <RetainOfflineServicingPassInUnattend>true</RetainOfflineServicingPassInUnattend>
                    <PreprovisionedVm>true</PreprovisionedVm>
                    <EnableTrustedImageIdentifier>false</EnableTrustedImageIdentifier>
                </PlatformSettings>
            </wa:PlatformSettingsSection>
        </Environment>"#;
        match parse_ovf_env(ovf_body) {
            Err(Error::NonEmptyPassword) => {}
            _ => panic!("Non-empty passwords aren't allowed"),
        };
    }

    #[test]
    fn test_get_mount_device_with_cdrom_entries() {
        let mut temp_file =
            NamedTempFile::new().expect("Failed to create temporary file");
        let mount_table = r#"
            /dev/sr0 /mnt/cdrom iso9660 ro,user,noauto 0 0
            /dev/sr1 /mnt/cdrom2 udf ro,user,noauto 0 0
        "#;
        temp_file
            .write_all(mount_table.as_bytes())
            .expect("Failed to write to temporary file");
        let temp_path = temp_file.into_temp_path();
        let result = get_mount_device(Some(temp_path.as_ref()));

        let list_devices = result.unwrap();
        assert_eq!(
            list_devices,
            vec!["/dev/sr0".to_string(), "/dev/sr1".to_string()]
        );
    }

    #[test]
    fn test_get_mount_device_without_cdrom_entries() {
        let mut temp_file =
            NamedTempFile::new().expect("Failed to create temporary file");
        let mount_table = r#"
            /dev/sda1 / ext4 defaults 0 0
            /dev/sda2 /home ext4 defaults 0 0
        "#;
        temp_file
            .write_all(mount_table.as_bytes())
            .expect("Failed to write to temporary file");
        let temp_path = temp_file.into_temp_path();
        let result = get_mount_device(Some(temp_path.as_ref()));

        let list_devices = result.unwrap();
        assert!(list_devices.is_empty());
    }

    #[test]
    fn test_get_mount_device_with_mixed_entries() {
        let mut temp_file =
            NamedTempFile::new().expect("Failed to create temporary file");
        let mount_table = r#"
            /dev/sr0 /mnt/cdrom iso9660 ro,user,noauto 0 0
            /dev/sda1 / ext4 defaults 0 0
            /dev/sr1 /mnt/cdrom2 udf ro,user,noauto 0 0
        "#;
        temp_file
            .write_all(mount_table.as_bytes())
            .expect("Failed to write to temporary file");
        let temp_path = temp_file.into_temp_path();
        let result = get_mount_device(Some(temp_path.as_ref()));

        let list_devices = result.unwrap();
        assert_eq!(
            list_devices,
            vec!["/dev/sr0".to_string(), "/dev/sr1".to_string()]
        );
    }

    #[test]
    fn test_get_mount_device_with_empty_table() {
        let mut temp_file =
            NamedTempFile::new().expect("Failed to create temporary file");
        let mount_table = "";
        temp_file
            .write_all(mount_table.as_bytes())
            .expect("Failed to write to temporary file");
        let temp_path = temp_file.into_temp_path();
        let result = get_mount_device(Some(temp_path.as_ref()));

        let list_devices = result.unwrap();
        assert!(list_devices.is_empty());
    }
}
