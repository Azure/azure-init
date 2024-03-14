// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs;
use std::fs::create_dir_all;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;

use serde::Deserialize;
use serde_xml_rs::from_str;

#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct Environment {
    #[serde(rename = "ProvisioningSection")]
    pub provisioning_section: ProvisioningSection,
    #[serde(rename = "PlatformSettingsSection")]
    pub platform_settings_section: PlatformSettingsSection,
}

#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct ProvisioningSection {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "LinuxProvisioningConfigurationSet")]
    pub linux_prov_conf_set: LinuxProvisioningConfigurationSet,
}

#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct LinuxProvisioningConfigurationSet {
    #[serde(rename = "UserName")]
    pub username: String,
    #[serde(default = "default_password", rename = "UserPassword")]
    pub password: String,
    #[serde(rename = "HostName")]
    pub hostname: String,
}

#[derive(Debug, Deserialize, PartialEq, Clone)]
pub struct PlatformSettingsSection {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "PlatformSettings")]
    pub platform_settings: PlatformSettings,
}

#[derive(Debug, Deserialize, PartialEq, Clone)]
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

pub fn make_temp_directory() -> Result<(), Box<dyn std::error::Error>> {
    let file_path = "/run/azure-init/tmp/";

    create_dir_all(file_path)?;

    let metadata = fs::metadata(file_path).unwrap();
    let permissions = metadata.permissions();
    let mut new_permissions = permissions.clone();
    new_permissions.set_mode(0o700);
    fs::set_permissions(file_path, new_permissions).unwrap();

    Ok(())
}

pub fn read_ovf_env_to_string() -> Result<String, Box<dyn std::error::Error>> {
    let file_path = "/run/azure-init/tmp/ovf-env.xml";
    let mut file = File::open(file_path).expect("Failed to open file");
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .expect("Failed to read file");

    Ok(contents)
}

pub fn parse_ovf_env(
    ovf_body: &str,
) -> Result<Environment, Box<dyn std::error::Error>> {
    let environment: Environment = from_str(ovf_body)?;

    if environment
        .provisioning_section
        .linux_prov_conf_set
        .password
        .is_empty()
    {
        return Err("Password is empty".into());
    }

    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "mypassword"
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
            "mypassword"
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
    fn test_get_ovf_env_missing_password() {
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

        assert!(parse_ovf_env(ovf_body)
            .err()
            .unwrap()
            .downcast::<std::io::Error>()
            .is_err());
    }

    #[test]
    fn test_get_ovf_env_missing_three() {
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
                    <EnableTrustedImageIdentifier>false</EnableTrustedImageIdentifier>
                </PlatformSettings>
            </wa:PlatformSettingsSection>
        </Environment>"#;

        assert!(parse_ovf_env(ovf_body)
            .err()
            .unwrap()
            .downcast::<std::io::Error>()
            .is_err());
    }
}
