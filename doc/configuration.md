# Custom Configuration Design for azure-init

## Objective

The azure-init custom configuration architecture enables dynamic and flexible management of various settings for virtual machines provisioned with the light-weight agent. Customizable settings include SSH, IMDS, provisioning media, azure proxy agent, wireserver, and telemetry.

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
3. Merges `.toml` files found from `azure-init.toml.d` in lexicographical order. The last file in the sorted order takes precedence.
   - `01-network.toml`
   - `02-ssh.toml`
   - `99-overrides.toml`
4. Applies any CLI overrides, either from a file or a directory.

## Validation and Deserialization Process

Azure Init uses strict validation on configuration fields to ensure they match expected types and values. If a configuration includes an unsupported value or incorrect type, deserialization will fail.

### Error Handling During Deserialization

- When a configuration file is loaded, its contents are parsed and converted from `.toml` into structured data. If a field in the file contains an invalid value (e.g., `query_sshd_config` is set to `"not_a_boolean"` instead of `true` or `false`), the parsing process will fail with a deserialization error due to the mismatched type.

### Propagation of Deserialization Errors

- When deserialization fails, an error is logged to indicate that the configuration file could not be parsed correctly. This error propagates through the application, causing the provisioning process to fail. The application will not proceed with provisioning if the configuration is invalid.

### Example of an Unsupported Value

Here’s an example configuration with an invalid value for `query_sshd_config`. This field expects a boolean (`true` or `false`), but in this case, an unsupported string value `"not_a_boolean"` is provided.

```toml
# Invalid value for query_sshd_config (not a boolean)
[ssh]
query_sshd_config = "not_a_boolean" # This will cause a validation failure
```

## Sample of Valid Configuration File

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

## Behavior of `azure-init` on Invalid Configuration

`azure-init` handles configuration issues by logging errors and either using default values or halting functionality, depending on the severity of the issue. Here’s how it responds to different types of problems:

### 1. Invalid Configuration

- If a configuration file contains syntax errors (e.g., malformed TOML) or unsupported values for fields (e.g., invalid enums),  `azure-init` logs the error and terminates. The provisioning process does not proceed when configuration parsing fails.

### 2. Missing or Invalid SSH Configuration

- `query_sshd_config = true`:
  - `azure-init` attempts to dynamically query the authorized keys path using the `sshd -G` command.
  - If `sshd -G` succeeds: The dynamically queried path is used for the authorized keys.
  - If `sshd -G` fails: The failure is logged, but azure-init continues using the fallback path specified in authorized_keys_path (default: `.ssh/authorized_keys`).
- `query_sshd_config = false`:
  - `azure-init` skips querying `sshd -G` entirely
  - The value in `authorized_keys_path` is used directly, without any dynamic path detection.

### 3. Handling of Provisioners in `azure-init`

The `azure-init` configuration allows for custom settings of hostnames, user creation, and password setup through the use of provisioners. If `backends` are specified but do not contain a valid provisioner, `azure-init` logs an error  and halts provisioning.

## Package Considerations

To ensure smooth operation and compatibility across distributions, `azure-init`, should be packaged with a consistent configuration setup.

- Distributions packaging `azure-init` are expected to maintain the base configuration file at `/etc/azure-init/azure-init.toml`, with necessary overrides applied from a `.d` subdirectory.
