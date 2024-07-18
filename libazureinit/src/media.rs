// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
const MTAB_PATH: &str = "/etc/mtab";

// Get a mounted device with any filesystem for CDROM
#[instrument]
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

// Some zero-sized structs that just provide states for our state machine
#[derive(Debug)]
pub struct Mounted;
#[derive(Debug)]
pub struct Unmounted;

#[derive(Debug)]
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

    #[instrument]
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
    #[instrument]
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

    #[instrument]
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

// Mount the given device, get OVF environment data, return it.
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
