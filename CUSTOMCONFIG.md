# Custom Configuration Design for azure-init

## Objective
The azure-init custom configuration architecture enables dynamic and flexible management of various settings for virtual machines provisioned with the light-weight agent. Customizable settings include SSH, networking, IMDS, provisioning media, azure proxy agent, wire server, error handling, and telemetry. This flexible design ensures that users can adapt configurations to their specific needs.

## Design
The system is designed to support default configurations, while allowing for user-specified overrides through config files or the command line interface. 

### Key Features
- **Override Support**: Default configurations are defined inside `config.rs`, with the option to override these settings via a specified configuration file or the CLI at runtime. CLI arguments take precedence over default configuration settings and any other configuration files.
- **Config Validation**: Custom enum types for `NetworkManager` and `SshAuthorizedKeysPathQueryMode` validate that user-set config settings match the supported options. Invalid values will cause deserialization to fail.
- **Built-in Defaults**: The system defines defaults directly in the code using Rust's `Default` trait, eliminating the need for a separate default config file.
- **Merging of Configurations**: The `load()` and `merge()` methods merge multiple sources of configuration data. Defaults are loaded first, then overridden by values from a config file, and finally by CLI-specified configurations.

## azure-init

### Config File Structure
**Format**: TOML

- The configuration relies on default values defined in the source code (`config.rs`).
- Users can override these defaults by providing a TOML configuration file.
- A custom configuration file can be passed via the CLI or added by the user.

### CLI Parameters
Example: `--config /etc/azure-init/`

## Configuration Hierarchy for `azure-init`

This document outlines the configuration system for `azure-init`, which allows flexibility by merging settings from multiple sources. Configuration can be set via a single file, a directory containing multiple files, or default values defined in the code.

---

### Configuration Loading Order

#### 1. CLI Override (`--config`)
   - The `--config` flag specifies the path to load the configuration from, which can be either a single file or a directory.
     - **File:** If the path points to a file, only that file is loaded.
     - **Directory:** If the path points to a directory, a series of `.toml` files in the specified directory will be loaded and merged based on specific rules.
   - **Example:** `azure-init --config /path/to/custom-config.toml`

#### 2. Directory Loading Logic
   - If the `--config` parameter points to a directory, `azure-init` follows this hierarchy:
     - First, it looks for a base file named `azure-init.toml` in the directory.
     - Then, it merges any additional `.toml` files in a `.d` subdirectory within the specified directory, in alphabetical order.
     ```text
     /etc/azure-init/
     ├── azure-init.toml            # Base configuration
     └── .d/
         ├── 01-network.toml        # Additional network configuration
         ├── 02-ssh.toml            # Additional SSH configuration
         └── 99-overrides.toml      # Final overrides
     ```

#### 3. Defaults in Code
   - If neither a file nor a directory is provided, `azure-init` falls back to using the default values specified in `config.rs`.

---

### Example 1: Single Configuration File
**Command:**
```sh
azure-init --config /path/to/custom-config.toml
```
**Order of Merging:**
1. Loads `/path/to/custom-config.toml`.
2. Applies any CLI overrides if present.
3. Fills in missing values with defaults from `config.rs`.

### Example 2: Directory with Multiple .toml Files

**Command:**
```sh
azure-init --config /path/to/custom-config-directory
```
**Directory Structure**
```bash
/path/to/custom-config-directory/
├── azure-init.toml                # Base configuration
└── .d/
    ├── 01-network.toml            # Network configuration
    ├── 02-ssh.toml                # SSH configuration
    └── 99-overrides.toml          # Overrides
```
**Order of Merging:**

1. Loads `azure-init.toml` as the base configuration.
2. Merges `.d` files in alphabetical order:
   - `01-network.toml`
   - `02-ssh.toml`
   - `99-overrides.toml`
3. Applies any CLI overrides if present.
4. Fills in missing values with defaults from `config.rs`.

### Example 3: Default Path without --config
**Assumption:** If no `--config` path is specified, `azure-init` will only use the default values set within `config.rs`.

**Order of Loading:**

1. Loads configuration directly from defaults specified in `config.rs`.

## Validation Behaviors

Custom enum types for `NetworkManager` and `SshAuthorizedKeysPathQueryMode` validate that config settings set by the user match the supported options. If a user enters an unsupported config value, deserialization will fail because Serde will not be able to map that value to one of the enum variants.

```rust
#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy)]
pub enum NetworkManager {
    NetworkManager,
    #[serde(rename = "systemd-networkd")]
    #[default]
    SystemdNetworkd,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, Copy)]
pub enum SshAuthorizedKeysPathQueryMode {
    #[serde(rename = "sshd -G")]
    SshdG,
    #[serde(rename = "disabled")]
    #[default]
    Disabled,
}
```

#### Example of the expected error caused by entering an unsupported config value:

```javascript
Error: Invalid value 'unsupported_value' for NetworkManager. Expected 'NetworkManager' or 'systemd-networkd'.
```

The enum also relies on the `#[serde(rename = "disabled")]` attribute, which maps the Rust enum variant `Disabled` to the string "disabled."  This allows clear internal code naming while matching formats set by users in their configuration.

### Configuration Fields

#### Network Struct:
- **manage_configuration**: `bool`
  - **Default**: `true`
  - **Description**: Controls whether network configuration is managed by the system.
- **network_manager**: `NetworkManager`
  - **Default**: `SystemdNetworkd` (as per `NetworkManager` enum)
  - **Description**: Specifies which tool manages the network configuration.

#### Ssh Struct:
- **authorized_keys_path**: `PathBuf`
  - **Default**: `~/.ssh/authorized_keys`
  - **Description**: Specifies the file path to the SSH authorized keys.
- **configure_password_authentication**: `bool`
  - **Default**: `false`
  - **Description**: Controls whether password authentication is configured for SSH.
- **authorized_keys_path_query_mode**: `SshAuthorizedKeysPathQueryMode`
  - **Default**: `Disabled` (as per `Ssh` enum)
  - **Description**: Determines if SSH key paths should be queried dynamically using `sshd -G` or rely on the default path.


#### HostnameProvisioners Struct
- **backends**: `HostnameProvisioner`
  - **Default**: `Hostnamectl`
  - **Description**: Defines the provisioner used to set the hostname.
  - **Variants**:
    - **FakeHostnamectl**: Testing provisioner that simulates `hostnamectl`.

#### UserProvisioners Struct
- **backends**: `UserProvisioner`
  - **Default**: `Useradd`
  - **Description**: Specifies the tool used to create a user on the system.
  - **Variants**:
    - **FakeUseradd**: Testing provisioner that simulates `useradd`.

#### PasswordProvisioners Struct
- **backends**: `PasswordProvisioner`
  - **Default**: `Passwd`
  - **Description**: Specifies the tool used to set user passwords.
  - **Variants**:
    - **FakePasswd**: Testing provisioner that simulates `passwd`.
#### Imds Struct:
- **connection_timeout_secs**: `f64`
  - **Default**: `2.0` seconds
  - **Description**: Specifies the timeout for IMDS connection attempts.
- **read_timeout_secs**: `u32`
  - **Default**: `60` seconds
  - **Description**: Specifies the timeout for reading from IMDS.
- **retry_timeout_secs**: `u32`
  - **Default**: `600` seconds
  - **Description**: Specifies the total retry timeout period for IMDS requests.

#### ProvisioningMedia Struct:
- **enable**: `bool`
  - **Default**: `true`
  - **Description**: Controls whether provisioning media (e.g., cloud-init metadata) is enabled.

#### AzureProxyAgent Struct:
- **enable**: `bool`
  - **Default**: `true`
  - **Description**: Controls whether the Azure Proxy Agent (used for communication with Azure services) is enabled.

#### Wireserver Struct:
- **connection_timeout_secs**: `f64`
  - **Default**: `2.0` seconds
  - **Description**: Specifies the timeout for connecting to the WireServer.
- **read_timeout_secs**: `u32`
  - **Default**: `60` seconds
  - **Description**: Specifies the timeout for reading data from the WireServer.
- **retry_timeout_secs**: `u32`
  - **Default**: `1200` seconds
  - **Description**: Specifies the total retry timeout period for WireServer requests.

#### Telemetry Struct:
- **kvp_diagnostics**: `bool`
  - **Default**: `true`
  - **Description**: Controls whether key-value pair diagnostics are enabled for telemetry.

## Configuration Schema -- Moving Towards a `Config` Struct in `Provision`

### Overview
There have been two main approaches discussed for handling configuration in `azure-init`:

1. Using a builder API in the `Provision` struct to individually configure each option.
2. Encapsulating configuration in a `Config` struct, which is then passed into the `Provision` struct.

After weighing the pros and cons, I believe the best approach moving forward is to modify the `Provision` struct to accept a `Config` object. This design simplifies the configuration system while allowing for flexible, centralized management of configuration values across the project.

### Key Changes

- **Goal**: Retain the `Config` struct in `libazureinit`, but modify the `Provision` struct so it accepts a `Config` object rather than using the builder API for each configuration option.
  
- **How It Works**: Users will create a `Config` object, which can either be manually instantiated (with defaults, if necessary) or loaded from a configuration file (e.g., TOML). This `Config` object is then passed into the `Provision` struct.

- **Result**: The `Provision` struct will no longer have builder methods for every individual configuration option. Instead, the entire configuration is passed in through the `Config` object.

### Example Implementation

```rust
// Define the Config struct in config.rs
#[derive(Default, Serialize, Deserialize, Debug)]
pub struct Config {
    pub authorized_keys_path: String,
    pub network_manager: String,
    pub connection_timeout_secs: f64,
    // other fields...
}

// Define the Provision struct, which accepts a Config object
pub struct Provision {
    config: Config,
    // other fields...
}

impl Provision {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            // other fields initialized from Config...
        }
    }

    // Additional methods for provisioning, using values from the Config object...
}
```
### Flow Description

#### Config Struct in `config.rs`:
- The `Config` struct defines all configuration options (e.g., `authorized_keys_path`, `network_manager`, etc.), and it implements the `Default` trait to provide sensible default values.
- Users can override these default values by loading a TOML file or passing values through CLI parameters.

#### Main Function in `main.rs`:
- In `main.rs`, the user-provided configuration (if any) is parsed. If a configuration file is provided, it is loaded into a `Config` object. If not, the `Config` object is instantiated with default values.
- The `Config` object is then passed to the `Provision::new()` function, which sets up the system using the specified configuration.

#### Example Flow in `main.rs`:
```rust
fn main() {
    // Load the configuration file or use defaults
    let config = load_config_from_file_or_default("/path/to/config.toml");
    
    // Initialize Provision with the Config object
    let provision = Provision::new(config);

    // Call the provision method to apply the configuration
    provision.provision_ssh();
    provision.provision_network();
    // Other provisioning logic...
}

fn load_config_from_file_or_default(path: &str) -> Config {
    // Attempt to load the config from a file, fallback to defaults if not found
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}
```

### Pros of This Approach

- **Single Source of Truth for Configuration**: The `Config` struct consolidates all configuration values, ensuring consistency and reducing the chance of duplication.

- **Flexibility**: Users can either manually create a `Config` object (by specifying custom values) or load it from a file (e.g., TOML) or CLI arguments. This flexibility makes it easier to adapt the system for different environments or user needs.

- **Standard Usage**: Many larger systems (such as `tokio` and `serde`) follow a similar pattern, encapsulating configuration in a struct that can be passed around. This design is well-understood and scalable, particularly in projects with complex configuration needs.

### Cons of This Approach

- **More Objects to Manage**: Users will need to manage both the `Config` and `Provision` objects. This might add some cognitive overhead, as they need to understand how to construct both.

- **API Complexity**: This design might be more complex for users who are accustomed to a simple builder-style API. Instead of calling individual methods for each configuration option, they must work with the `Config` object, which is an API-breaking change.

### Summary

This approach centralizes configuration in a `Config` struct and passes it into `Provision`. It eliminates the need for a method-per-configuration approach and reduces duplication, providing a single source of truth for the configuration. Given the flexibility and consistency this provides, I believe this is the best approach for the long-term maintainability of the `azure-init` project.

## Config Struct PR Changes and Notes - as of 10/25/2024

This section documents the key changes and improvements made to support a flexible configuration structure in `azure-init`, including updates to `main.rs`, `mod.rs`, `ssh.rs`, and `config.rs`.

### 1. New Configuration Hierarchy and --config Flag Support
- **Purpose**: Allows users to specify configuration through a single file or directory, providing enhanced flexibility for custom setups.
- **Changes**:
  - **`main.rs`**: 
    - Introduced CLI parsing for `--config`, allowing users to specify a file or directory. 
    - If provided, the specified path is passed into the `Config` struct’s `load` function.
    - The `--config` parameter is parsed within `provision()` and no longer directly set in `main()`, maintaining separation of concerns.

  - **`config.rs`**:
    - **`load` Function**: Updated to accept a `PathBuf` option for `--config`. When a file path is given, it loads only that file. If a directory path is specified, it loads `azure-init.toml` as the base and any `.toml` files in `.d` subdirectory in alphabetical order, merging them.
    - **Defaults**: If no `--config` is passed, defaults from `config.rs` are applied, bypassing `/etc/azure-init`.
    - **Error Handling**: Added handling for parsing errors, ensuring unhandled errors are returned as clear failure messages.
  
  - **Merge Logic**: The `merge` function was updated to handle hierarchical overrides between defaults, CLI-specified files, and base configuration, ensuring the correct application of settings.

### 2. New Enum-Based Provisioners for `Hostname`, `User`, and `Password`
- **Purpose**: Streamlines the selection of system tools for setting hostname, user, and password, enabling test mocks and extensibility.
- **Changes**:
  - **`config.rs`**: Added enums `HostnameProvisioner`, `UserProvisioner`, and `PasswordProvisioner` with default implementations for each.
  - **Test Variants**: Includes `FakeHostnamectl`, `FakeUseradd`, and `FakePasswd` variants under `#[cfg(test)]` for unit testing.

### 3. Modularized SSH Provisioning with Config-Driven Paths
- **`ssh.rs`**:
  - The SSH provisioning process now uses `authorized_keys_path` and `authorized_keys_path_query_mode` from the `Config` struct. This provides flexible handling of SSH keys via `sshd -G` or a custom path.
  - **SSH Directory Structure**: Added logic to set permissions and configure the `.ssh` directory and `authorized_keys` file, using the directory path as specified in the configuration.

### 4. Updated `Provision` Struct in `mod.rs` to Accept Config Object
- **Purpose**: Integrates the `Config` struct into provisioning, enabling flexible overrides and default handling.
- **Changes**:
  - `Provision` now accepts a `Config` object on initialization, replacing previous hard-coded values.
  - **Provisioning Flow**: Methods `hostname_provisioners`, `user_provisioners`, and `password_provisioners` were removed from `main.rs` and are now part of `Config`. Provisioning logic directly references these config-driven settings.

### Summary of Changes
These updates improve `azure-init`'s flexibility and compatibility with different systems by providing a structured configuration hierarchy and cleaner separation of concerns. The `Config` struct now serves as a single source of truth for system settings, supporting centralized configuration through CLI, files, and defaults as needed.



## Alternative / Future Configuration Schema — Move Config to the Provision Struct
### Key Changes:
- **Goal**: Eliminate the `Config` struct entirely and move all configuration into the `Provision` struct.
- **How It Works**: Every configuration value (e.g., `authorized_keys_path`) becomes a method on the `Provision` struct. Users configure everything using a builder pattern (e.g., `Provision::new().authorized_keys_path(...)`).
- **Result**: There is no longer a `Config` struct, and users do not need to write or depend on a separate configuration file for `libazureinit`.

### Example:
```rust
pub struct Provision {
    authorized_keys_path: Option<PathBuf>,
}

impl Provision {
    pub fn new(hostname: impl Into<String>, user: User) -> Self {
        Self {
            authorized_keys_path: None,
        }
    }

    pub fn authorized_keys_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.authorized_keys_path = Some(path.into());
        self
    }
}
```
### Pros:
- **Simplifies Configuration**: Users only interact with the `Provision` struct, making the configuration process consistent.
- **No Config File Dependency**: Users do not need to think about or provide configuration files. All configuration is done through the API.
- **Common Rust Pattern**: This pattern is widely used in Rust, especially in systems with many configuration options. The builder pattern allows for a fluid, chainable API that lets users configure only the options they care about.

### Cons:
- **Default Handling**: Default values would need to be set in the `Provision` struct’s builder methods. This could lead to duplication if default handling is also required in other parts of the system.
- **Increased Complexity**: As the number of configurable options grows, the `Provision` struct could become overloaded with numerous methods, making it harder to manage and maintain.

### Why This Approach May Not Be the Best First Step

While moving all configuration into the `Provision` struct and using a builder pattern has its benefits, it also presents challenges that make it less ideal as the first approach for the current stage of the project:

- **Default Handling Complexity**: Without a dedicated `Config` struct, we run the risk of scattering default-handling logic across multiple methods in the `Provision` struct, increasing the chances of duplication and errors. Managing default values centrally in the `Config` struct is more efficient, especially with the growing number of configuration fields.

- **Increased Complexity for Users**: Although the builder pattern simplifies the API, it may complicate things for users when managing large sets of configurations. This can become particularly cumbersome if more options are added, leading to bloated and complex `Provision` objects.

- **Focus on Current Priorities and Easier Transition**: Right now, the primary focus is getting the base image working and merging the open PRs. Starting with a `Config` struct provides a simpler, more centralized configuration management system, expediting integration with the existing system. This also lays a stable foundation for an easier transition to a builder pattern later, if we decide that is what we want instead. 

## Package Considerations
When packaging `azure-init`, it is essential to configure the default settings to ensure smooth operation and compatibility across distributions. Below are key recommendations for maintaining configuration consistency:

- **Service File Configuration**: The service file for `azure-init` should specify `--config` pointing to `/etc/azure-init` by default. This setup ensures that `azure-init` references the correct configuration directory for each instance.

- **Distribution Responsibility**: Distributions packaging `azure-init` are expected to maintain the primary configuration file at `/etc/azure-init/azure-init.toml`. This file serves as the base configuration, with any necessary overrides applied from the `.d` subdirectory (if configured). 

This setup enables system administrators and package maintainers to manage system-wide configurations centrally while allowing flexibility through additional configuration layers if required.