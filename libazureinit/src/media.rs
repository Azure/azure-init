// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::fs::create_dir_all;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;
use serde_xml_rs::from_str;

use tracing;

use crate::error::Error;
use block_utils::Device;


#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct Environment {
    #[serde(rename = "ProvisioningSection")]
    pub provisioning_section: ProvisioningSection,
    #[serde(rename = "PlatformSettingsSection")]
    pub platform_settings_section: PlatformSettingsSection,
}

#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct ProvisioningSection {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "LinuxProvisioningConfigurationSet")]
    pub linux_prov_conf_set: LinuxProvisioningConfigurationSet,
}

#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct LinuxProvisioningConfigurationSet {
    #[serde(rename = "UserName")]
    pub username: String,
    #[serde(default = "default_password", rename = "UserPassword")]
    pub password: String,
    #[serde(rename = "HostName")]
    pub hostname: String,
}

#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct PlatformSettingsSection {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "PlatformSettings")]
    pub platform_settings: PlatformSettings,
}

#[derive(Debug, Default, Deserialize, PartialEq, Clone)]
pub struct PlatformSettings {
    #[serde(default = "default_preprov", rename = "PreprovisionedVm")]
    pub preprovisioned_vm: bool,
    #[serde(default = "default_preprov_type", rename = "PreprovisionedVmType")]
    pub preprovisioned_vm_type: String,
}

fn default_password() -> String {
    "".to_owned()
}

fn default_preprov() -> bool {
    false
}

fn default_preprov_type() -> String {
    "None".to_owned()
}

pub const PATH_MOUNT_DEVICE: &str = "/dev/sr0";
pub const PATH_MOUNT_POINT: &str = "/run/azure-init/media/";

const CDROM_VALID_FS: &[&str] = &["iso9660", "udf"];

// Get a mounted device with any filesystem for CDROM
pub fn get_mount_device<F>(get_devices: F) -> Result<Vec<String>, Error>
where
    F: Fn() -> Result<Vec<Device>, Error>,
{
    let devices = get_devices()?;
    let list_devices: Vec<String> = devices
        .into_iter()
        .filter_map(|dev| {
            if CDROM_VALID_FS.contains(&dev.fs_type.to_str()) {
                Some(dev.name)
            } else {
                None
            }
        })
        .collect();

    Ok(list_devices)
}

// Wrapper function
pub fn get_wrapped_mount_devices() -> Result<Vec<String>, Error> {
    get_mount_device(|| block_utils::get_mounted_devices().map_err(Error::from))
}

// Some zero-sized structs that just provide states for our state machine
pub struct Mounted;
pub struct Unmounted;

pub struct Media<State = Unmounted> {
    device_path: PathBuf,
    mount_path: PathBuf,
    state: std::marker::PhantomData<State>,
}

impl Media<Unmounted> {
    pub fn new(device_path: PathBuf, mount_path: PathBuf) -> Media<Unmounted> {
        Media {
            device_path,
            mount_path,
            state: std::marker::PhantomData,
        }
    }

    pub fn mount(self) -> Result<Media<Mounted>, Error> {
        create_dir_all(&self.mount_path)?;

        let metadata = fs::metadata(&self.mount_path)?;
        let permissions = metadata.permissions();
        let mut new_permissions = permissions;
        new_permissions.set_mode(0o700);
        fs::set_permissions(&self.mount_path, new_permissions)?;

        let mount_status = Command::new("mount")
            .arg("-o")
            .arg("ro")
            .arg(&self.device_path)
            .arg(&self.mount_path)
            .status()?;

        if !mount_status.success() {
            Err(Error::SubprocessFailed {
                command: "mount".to_string(),
                status: mount_status,
            })
        } else {
            Ok(Media {
                device_path: self.device_path,
                mount_path: self.mount_path,
                state: std::marker::PhantomData,
            })
        }
    }
}

impl Media<Mounted> {
    pub fn unmount(self) -> Result<(), Error> {
        let umount_status =
            Command::new("umount").arg(self.mount_path).status()?;
        if !umount_status.success() {
            return Err(Error::SubprocessFailed {
                command: "umount".to_string(),
                status: umount_status,
            });
        }

        let eject_status =
            Command::new("eject").arg(self.device_path).status()?;
        if !eject_status.success() {
            Err(Error::SubprocessFailed {
                command: "eject".to_string(),
                status: eject_status,
            })
        } else {
            Ok(())
        }
    }

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

// Mount the given device, get OVF environment data, return it.
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
    use block_utils::{Device, DeviceType, MediaType, FilesystemType};
    use uuid::Uuid;
    use crate::error::Error; 

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
    fn test_get_mount_device() {
        // Mock function to return a predefined list of devices
        let mock_get_mounted_devices = || -> Result<Vec<Device>, Error> {
            Ok(vec![
                Device {
                    id: Some(Uuid::new_v4()),
                    name: "device1".to_string(),
                    media_type: MediaType::NVME, // Adjust this to match the MediaType variants
                    device_type: DeviceType::Disk,
                    capacity: 700_000_000,
                    fs_type: FilesystemType::Ntfs, // Adjust this to match the FilesystemType variants
                    serial_number: Some("12345".to_string()),
                    logical_block_size: Some(512),
                    physical_block_size: Some(512),
                },
                Device {
                    id: Some(Uuid::new_v4()),
                    name: "device2".to_string(),
                    media_type: MediaType::Rotational, // Adjust this to match the MediaType variants
                    device_type: DeviceType::Disk,
                    capacity: 1_000_000_000,
                    fs_type: FilesystemType::Ext4,
                    serial_number: Some("67890".to_string()),
                    logical_block_size: Some(512),
                    physical_block_size: Some(512),
                },
                Device {
                    id: Some(Uuid::new_v4()),
                    name: "device3".to_string(),
                    media_type: MediaType::NVME, // Adjust this to match the MediaType variants
                    device_type: DeviceType::Disk,
                    capacity: 700_000_000,
                    fs_type: FilesystemType::Xfs, // Adjust this to match the FilesystemType variants
                    serial_number: Some("54321".to_string()),
                    logical_block_size: Some(512),
                    physical_block_size: Some(512),
                },
            ])
        };

        let result = get_mount_device(mock_get_mounted_devices);

        assert!(result.is_ok());
        let list_devices = result.unwrap();
        assert_eq!(list_devices, vec!["device1".to_string(), "device3".to_string()]);
    }
}
