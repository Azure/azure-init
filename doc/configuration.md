# Custom Configuration for Azure-init

## Quick Start

Getting started with azure-init configuration is simple. Here are the basic steps:

1. **Default Configuration**: With no configuration file, azure-init uses sensible defaults
2. **Basic Configuration**: Create a file at `/etc/azure-init/azure-init.toml` with your settings
3. **Modular Configuration**: Add specialized configuration files to `/etc/azure-init/azure-init.toml.d/`

**Example: Basic SSH Configuration**
```toml
# /etc/azure-init/azure-init.toml
[ssh]
query_sshd_config = true
authorized_keys_path = "/etc/ssh/authorized_keys"
```

**Example: Modular Configuration**
```toml
# /etc/azure-init/azure-init.toml.d/01-timeouts.toml
[imds]
connection_timeout_secs = 5.0
read_timeout_secs = 30
```

## Objective

The azure-init custom configuration architecture enables flexible management of various settings for virtual machines provisioned with the agent.
Customizable settings include SSH, IMDS, provisioning media, azure proxy agent, wireserver, and telemetry.

## Design

The system supports default configurations with user-specified overrides via configuration files or CLI parameters.

### Key Features

- **Config Validation**: Validates user-provided settings against supported options, rejecting invalid values during deserialization.
- **Built-in Defaults**: The system defines defaults directly in the code, eliminating the need for a separate default config file.
- **Merging of Configurations**: Combines configuration sources hierarchically, applying defaults first, followed by file-based overrides, and finally CLI parameters for the highest precedence.

## Config File Structure

**Format**: TOML

- Users can override default settings by supplying a single configuration file or multiple `.toml` files in a directory.

### CLI Parameters

Example: `--config /etc/azure-init/`

## Configuration Hierarchy for `azure-init`

Configuration can be set via a single file, a directory containing multiple files, or the default values defined in the code.

### Configuration Loading Order

#### 1. Defaults in Code

The configuration process starts with the built-in defaults specified in `Config::default()`.

#### 2. Base File and Directory Loading Logic

- After applying default values, `azure-init` checks for a base `azure-init.toml` file. If it exists, it is loaded as the base configuration.
- If an `azure-init.toml.d` directory exists, its `.toml` files are loaded and merged in lexicographical order.
- If neither the `azure-init.toml` nor the directory exists, the configuration remains as defined by the built-in defaults.

     ```text
     /etc/azure-init/
     ├── azure-init.toml            # Base configuration
     └── azure-init.toml.d/
         ├── 01-network.toml        # Additional network configuration
         ├── 02-ssh.toml            # Additional SSH configuration
         └── 99-overrides.toml      # Final overrides
     ```

- Each `.toml` file is merged into the configuration in the sorted order. If two files define the same configuration field, the value from the file processed last will take precedence. For example, in the order above, the final value(s) would come from `99-overrides.toml`.

#### 3. CLI Override (`--config`)

- The `--config` flag specifies a configuration path that can point to either a single file or a directory.
  - **File:** If a file is specified, it is merged as the final layer, overriding all prior configurations.
  - **Directory:** If a directory is specified, `.toml` files within it are loaded and merged in, following the same rules specified in the Directory Loading Logic section.
  - **Example:** `azure-init --config /path/to/custom-config.toml`

### Example: Directory with Multiple .toml Files

**Command:**

```sh
azure-init --config /path/to/custom-config-directory
```

**Directory Structure:**

```bash
/path/to/custom-config-directory/
├── azure-init.toml                # Base configuration
└── azure-init.toml.d/
    ├── 01-network.toml            # Network configuration
    ├── 02-ssh.toml                # SSH configuration
    └── 99-overrides.toml          # Overrides
```

**Order of Merging:**

1. Applies defaults from `Config::default()` as defined in `config.rs`.
2. Loads `azure-init.toml` as the base configuration, if present.
3. Merges configuration `.toml` files found in `azure-init.toml.d` in lexicographical order. The last file in the sorted order takes precedence.
   - `01-network.toml`
   - `02-ssh.toml`
   - `99-overrides.toml`
4. Applies any CLI overrides, either from a file or a directory.

## Configuration Options

Below are the key configuration sections and their options:

### SSH Configuration
```toml
[ssh]
authorized_keys_path = ".ssh/authorized_keys"  # Path for storing SSH authorized keys
query_sshd_config = true  # Whether to query sshd for config paths
```

### User Provisioning
```toml
[hostname_provisioners]
backends = ["hostnamectl"]  # List of backends for hostname provisioning

[user_provisioners]
backends = ["useradd"]  # List of backends for user creation

[password_provisioners]
backends = ["passwd"]  # List of backends for password management
```

### Network Configuration
```toml
[imds]
connection_timeout_secs = 2.0  # Timeout for initial IMDS connection
read_timeout_secs = 60  # Timeout for reading data from IMDS
total_retry_timeout_secs = 300  # Total time to retry IMDS connections

[wireserver]
connection_timeout_secs = 2.0  # Timeout for wireserver connection
read_timeout_secs = 60  # Timeout for reading data from wireserver
total_retry_timeout_secs = 1200  # Total time to retry wireserver connections
```

### Feature Toggles
```toml
[provisioning_media]
enable = true  # Enable provisioning media processing

[azure_proxy_agent]
enable = true  # Enable Azure proxy agent

[telemetry]
kvp_diagnostics = true  # Enable KVP diagnostics telemetry
```

## Validation and Deserialization Process

Azure-init uses strict validation on configuration fields to ensure they match expected types and values.
If a configuration includes an unsupported value or incorrect type, deserialization will fail.

### Error Handling During Deserialization

- When a configuration file is loaded, its contents are parsed and converted from `.toml` into structured data.
If a field in the file contains an invalid value (e.g., `query_sshd_config` is set to `"not_a_boolean"` instead of `true` or `false`), the parsing process will fail with a deserialization error due to the mismatched type.

### Propagation of Deserialization Errors

- When deserialization fails, an error is logged to indicate that the configuration file could not be parsed correctly. 
This error propagates through the application, causing the provisioning process to fail.
The application will not proceed with provisioning if the configuration is invalid.

### Example of an Unsupported Value

Here's an example configuration with an invalid value for `query_sshd_config`.
This field expects a boolean (`true` or `false`), but in this case, an unsupported string value `"not_a_boolean"` is provided.

```toml
# Invalid value for query_sshd_config (not a boolean)
[ssh]
query_sshd_config = "not_a_boolean" # This will cause a validation failure
```

## Complete Configuration Example

```toml
[ssh]
authorized_keys_path = ".ssh/authorized_keys"
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

## Behavior of Azure-init on Invalid Configuration

Azure-init handles configuration issues by logging errors and either using default values or halting functionality, depending on the severity of the issue.
Here's how it responds to different types of problems:

### 1. Invalid Configuration

- If a configuration file contains syntax errors (e.g., malformed TOML) or unsupported values for fields (e.g., invalid enums), azure-init logs the error and terminates.
The provisioning process does not proceed when configuration parsing fails.

### 2. Missing or Invalid SSH Configuration

- `query_sshd_config = true`:
  - Azure-init attempts to dynamically query the authorized keys path using the `sshd -G` command.
  - If `sshd -G` succeeds: The dynamically queried path is used for the authorized keys.
  - If `sshd -G` fails: The failure is logged, but azure-init continues using the fallback path specified in authorized_keys_path (default: `.ssh/authorized_keys`).
- `query_sshd_config = false`:
  - `azure-init` skips querying `sshd -G` entirely
  - The value in `authorized_keys_path` is used directly, without any dynamic path detection.

### 3. Handling of Provisioners in Azure-init

The azure-init configuration allows for custom settings of hostnames, user creation, and password setup through the use of provisioners. 
If `backends` are specified but do not contain a valid provisioner, azure-init logs an error and halts provisioning.

## Troubleshooting Configuration Issues

### Common Configuration Problems

1. **Syntax Errors in TOML**
   - **Symptom**: Azure-init fails to start with "failed to parse configuration" error
   - **Solution**: Validate your TOML syntax with a TOML validator

2. **Incorrect Value Types**
   - **Symptom**: Error message about type mismatch (e.g., "expected boolean, found string")
   - **Solution**: Ensure the configuration value matches the expected type (e.g., `true` instead of `"true"`)

3. **Missing Directories**
   - **Symptom**: Configuration files aren't being loaded
   - **Solution**: Verify that `/etc/azure-init/` directory exists and has proper permissions

4. **Configuration File Ordering Issues**
   - **Symptom**: Expected configuration values aren't taking effect
   - **Solution**: Check the lexicographical ordering of your .toml files in the .d directory

### Debugging Configuration Loading

To verify which configuration files are being loaded and in what order, you can enable `DEBUG` level logging:

```bash
RUST_LOG=debug azure-init
```

This will output detailed information about each configuration file as it's loaded and processed.

## Package Considerations

To ensure smooth operation and compatibility across distributions, azure-init should be packaged with a consistent configuration setup.

- Distributions packaging azure-init are expected to maintain the base configuration file at `/etc/azure-init/azure-init.toml`, with necessary overrides applied from a `.d` subdirectory.
