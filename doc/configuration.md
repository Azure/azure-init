# Custom Configuration Design for azure-init

## Objective
The azure-init custom configuration architecture enables dynamic and flexible management of various settings for virtual machines provisioned with the light-weight agent. Customizable settings include SSH, IMDS, provisioning media, azure proxy agent, wireserver, and telemetry. This flexible design ensures that users can adapt configurations to their specific needs.

## Design
The system is designed to support default configurations, while allowing for user-specified overrides through config files or the command line interface. 

### Key Features
- **Override Support**: Default configurations are defined inside `config.rs`, with the option to override these settings via a specified configuration file or the CLI at runtime. CLI arguments take precedence over default configuration settings and any other configuration files.
- **Config Validation**: Custom enum types for for fields such as `pub enum HostnameProvisioner` validate that user-set config settings match the supported options. Invalid values will cause deserialization to fail.
- **Built-in Defaults**: The system defines defaults directly in the code using Rust's `Default` trait, eliminating the need for a separate default config file.
- **Merging of Configurations**: The `load()` and `merge()` methods merge multiple sources of configuration data. Defaults are loaded first, then overridden by values from a config file, and finally by CLI-specified configurations.

## Config File Structure
**Format**: TOML

- The configuration relies on default values defined in the source code (`config.rs`).
- Users can override these defaults by providing a TOML configuration file.
- A custom configuration file can be passed via the CLI or added by the user.

### CLI Parameters
Example: `--config /etc/azure-init/`

## Configuration Hierarchy for `azure-init`

This document outlines the configuration system for `azure-init`, which allows flexibility by merging settings from multiple sources. Configuration can be set via a single file, a directory containing multiple files, or default values defined in the code.

### Configuration Loading Order

#### 1. CLI Override (`--config`)
   - The `--config` flag specifies the path to load the configuration from, which can be either a single file or a directory.
     - **File:** If the path points to a file, only that file is loaded.
     - **Directory:** If the path points to a directory, a series of `.toml` files in the specified directory will be loaded and merged based on specific rules.
   - **Example:** `azure-init --config /path/to/custom-config.toml`

#### 2. Directory Loading Logic
   - If the `--config` parameter points to a directory, `azure-init` follows this hierarchy:
     - First, it looks for a base file named `azure-init.toml` in the directory.
     - Then, it merges any additional `.toml` files in a `.d` subdirectory within the specified directory, in lexicographical order.
     ```text
     /etc/azure-init/
     ├── azure-init.toml            # Base configuration
     └── azure-init.toml.d/
         ├── 01-network.toml        # Additional network configuration
         ├── 02-ssh.toml            # Additional SSH configuration
         └── 99-overrides.toml      # Final overrides
     ```

#### 3. Defaults in Code
   - If neither a file nor a directory is provided, `azure-init` falls back to using the default values specified in `config.rs`.

### Example 1: Single Configuration File
**Command:**
```sh
azure-init --config /path/to/custom-config.toml
```
**Order of Precedence for Merging Configurations** (lowest to highest):
1. Compile-time defaults from `config.rs`.
2. Custom configuration file, such as `/path/to/azure-init.toml`.
3. Additional configurations found in `/path/to/azure-init.toml.d`, applied in lexicographical order.
4. CLI options, which override all other settings.

Each level of configuration overwrites values from previous levels, with CLI options taking highest precedence.

### Example 2: Directory with Multiple .toml Files

**Command:**
```sh
azure-init --config /path/to/custom-config-directory
```
**Directory Structure**
```bash
/path/to/custom-config-directory/
├── azure-init.toml                # Base configuration
└── azure-init.toml.d/
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

## Validation and Deserialization Process

Azure Init uses strict validation on configuration fields to ensure they match expected types and values. For fields defined as enums or specific types, the system checks that values provided by the user align with supported options. If a configuration includes an unsupported value or incorrect type, deserialization will fail.

### Configuration Struct and Field Definitions

- The `Config` struct in Azure Init includes fields that are defined with specific types or enums, like `bool` for `query_sshd_config`.
- Each field is expected to be of a specific type when deserialized by Serde (the library used for parsing configuration files).

### Error Handling During Deserialization
- When `Config::load()` attempts to load a configuration file (either from a specified file, directory, or default
path), it reads the file content and uses `toml::from_str` to convert it into a `Config` struct.

- If a field value in the configuration file doesn’t match its expected type (e.g., if `query_sshd_config` is set to `"not_a_boolean"` instead of `true` or `false`), Serde encounters a deserialization error because it can’t map the provided value to the expected type.

### Propagation of Deserialization Errors 
 - When deserialization fails, an error is generated and logged by `Config::load_from_file`, indicating that the configuration file could not be parsed correctly. This error is wrapped in an `Error::Io` type and returned by the function.
- This `Error::Io` is propagated up the call stack to the `Config::load()` function, which in turn returns the error to the caller.
- In the main application, `Config::load()` is called with `.expect("Failed to load configuration")`. As a result, any failure in loading the configuration causes the application to terminate immediately with a clear error message. This prevents the provisioning process from proceeding with an invalid configuration and provides immediate feedback to the user about the issue.



### Example of an Unsupported Value

Here’s an example configuration with an invalid value for `query_sshd_config`. This field expects a boolean (`true` or `false`), but in this case, an unsupported string value `"not_a_boolean"` is provided.

```toml
# Invalid value for query_sshd_config (not a boolean)
[ssh]
query_sshd_config = "not_a_boolean" # This will cause a validation failure
```


## Configuration Fields

#### Ssh Struct
- **authorized_keys_path**: `PathBuf`
  - **Default**: `.ssh/authorized_keys`
  - **Description**: Specifies the file path for storing SSH authorized keys when configuring SSH access.
- **query_sshd_config**: `bool`
  - **Default**: `true`
  - **Description**: Controls whether `azure-init` queries the SSH configuration dynamically via `sshd -G`. If set to `true` and `sshd -G` succeeds, 
                     the authorized keys path is set according to the SSH configuration. If `sshd -G` fails, `azure-init` reports the error but 
                     continues with `authorized_keys_path`.


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
- **total_retry_timeout_secs**: `u32`
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
- **total_retry_timeout_secs**: `u32`
  - **Default**: `1200` seconds
  - **Description**: Specifies the total retry timeout period for WireServer requests.

#### Telemetry Struct:
- **kvp_diagnostics**: `bool`
  - **Default**: `true`
  - **Description**: Controls whether key-value pair diagnostics are enabled for telemetry.

## Sample Configuration File

```toml
[ssh]
authorized_keys_path = ".ssh/authorized_keys"
configure_password_authentication = false
authorized_keys_path_query_mode = "disabled"
query_sshd_config = true

[hostname_provisioners]
backends = ["hostnamectl"]

[user_provisioners]
backends = ["useradd"]

[password_provisioners]
backends = ["passwd"]

[imds]
connection_timeout_secs = 2.0
read_timeout_secs = 60
total_retry_timeout_secs = 300

[provisioning_media]
enable = true

[azure_proxy_agent]
enable = true

[wireserver]
connection_timeout_secs = 2.0
read_timeout_secs = 60
total_retry_timeout_secs = 1200

[telemetry]
kvp_diagnostics = true
```

## Behavior of `azure-init` on Invalid Configuration

`azure-init` has built-in handling for various types of configuration issues. When a misconfiguration is detected, it logs errors and, depending on the severity, either continues with default values or halts the affected functionality. Here’s a breakdown of its behavior for different types of issues:

## Behavior of `azure-init` on Invalid Configuration or Other Configuration Errors

`azure-init` has built-in handling for various types of configuration issues. When a misconfiguration is detected, it logs errors and, depending on the severity, either continues with default values or halts the affected functionality. Below is a breakdown of its behavior for different types of issues:

### 1. Invalid Configuration Syntax

- **Description**: If a configuration file contains syntax errors (e.g., malformed TOML), `azure-init` logs the parsing error and fails.
- **Behavior**: This prevents `azure-init` from proceeding with potentially corrupted settings.

### 2. Unsupported Values for Enum Settings

- **Description**: If a configuration option has a value that does not match the supported enum options, `azure-init` will be unable to deserialize these settings.
- **Behavior**:
  - Logs a descriptive error message, indicating the unsupported value and the expected options.
  - Continues using defaults or halts only the affected component (depending on the criticality of the setting), while allowing other components to run normally.

### 3. Missing or Invalid SSH Configuration

- **Using `sshd -G`**:
  - If `sshd -G` succeeds, the authorized keys path is determined dynamically based on the output of the `sshd -G` command.
  - If `sshd -G` fails or cannot retrieve the `authorizedkeysfile`:
    - If `query_sshd_config` is enabled (default): `azure-init` will report the failure but will continue, using the path specified in `authorized_keys_path`, typically `.ssh/authorized_keys`.
    - If `query_sshd_config` is disabled, `azure-init` does not query `sshd -G` and instead directly uses `authorized_keys_path` as provided.

- **When `sshd -G` is Disabled**:
  - If `query_sshd_config` is set to `false`, `azure-init` will use the specified `authorized_keys_path` as-is without attempting to retrieve the path from `sshd -G`.
  - If `authorized_keys_path` is not configured and `sshd -G` is disabled, `azure-init` logs an error and halts, as an explicit authorized keys path is required in this case.

### 4. Handling of Provisioners in `azure-init`

The `azure-init` configuration allows for custom settings of hostnames, user creation, and password setup through the use of provisioners. If no provisioner is specified, `azure-init` defaults to the following settings:

- **HostnameProvisioner**: Defaults to `Hostnamectl`.
- **UserProvisioner**: Defaults to `Useradd`.
- **PasswordProvisioner**: Defaults to `Passwd`.

If `backends` are specified but do not contain a usable provisioner, `azure-init` will halt and log an error, indicating that no valid provisioner was found. Here’s the breakdown:

1. **HostnameProvisioner**:
   - **Default**: If unspecified, `HostnameProvisioner::Hostnamectl` is used.
   - **Failure**: If no backend can set the hostname, `azure-init` logs an error (`Error::NoHostnameProvisioner`) and halts.

2. **UserProvisioner**:
   - **Default**: If unspecified, `UserProvisioner::Useradd` is used.
   - **Failure**: If no backend can create the user, `azure-init` logs an error (`Error::NoUserProvisioner`) and halts.

3. **PasswordProvisioner**:
   - **Default**: If unspecified, `PasswordProvisioner::Passwd` is used.
   - **Failure**: If no backend can set the password, `azure-init` logs an error (`Error::NoPasswordProvisioner`) and halts.

### 5. Missing Non-Critical Configuration Settings

- **Description**: For optional settings (e.g., `telemetry`, `wireserver`), if configuration values are not provided, `azure-init` defaults to values in `Config::default()`.
- **Behavior**: Allows `azure-init` to proceed while logging any defaults used for transparency.

### 6. Logging and Tracing for Troubleshooting

- **Description**: All configuration issues are logged at appropriate levels (`error`, `warn`, or `info`) to aid in debugging.
- **Behavior**:
  - Enables tracing output for debugging and identifying root causes in a step-by-step manner.
  - Logs a summary at the end of initialization, detailing any settings that defaulted due to errors.

## Package Considerations
When packaging `azure-init`, it is essential to configure the default settings to ensure smooth operation and compatibility across distributions. Below are key recommendations for maintaining configuration consistency:

- **Service File Configuration**: The service file for `azure-init` should specify `--config` pointing to `/etc/azure-init` by default. This setup ensures that `azure-init` references the correct configuration directory for each instance.

- **Distribution Responsibility**: Distributions packaging `azure-init` are expected to maintain the primary configuration file at `/etc/azure-init/azure-init.toml`. This file serves as the base configuration, with any necessary overrides applied from the `.d` subdirectory (if configured). 

This setup enables system administrators and package maintainers to manage system-wide configurations centrally while allowing flexibility through additional configuration layers if required.