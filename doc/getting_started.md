# Getting Started with Azure-init

This guide provides step-by-step instructions to get started with azure-init for both development and usage scenarios.

## For Users

### Prerequisites

- An Azure subscription
- A Linux VM running in Azure

### Basic Usage

Azure-init typically needs to be pre-installed and configured in Linux VM images that are Azure-optimized, since the agent is not currently run on Azure Linux VMs by default.
At the moment, adding azure-init to a Linux image requires following the process found in the [SIG image testing guide](e2e_testing.md#about-sig-image-testing).
If you're using such an image, azure-init will automatically run during the boot process and handle VM initialization.

### Verifying Azure-init Operation

To check that azure-init is working properly on your VM:

1. Connect to your SIG VM via SSH
2. Check the azure-init service status:

```bash
sudo systemctl status azure-init
```

3. Review the logs:

```bash
sudo journalctl -u azure-init
```

### Custom Configuration

1. Create or edit the configuration file:

```bash
sudo mkdir -p /etc/azure-init
sudo nano /etc/azure-init/azure-init.toml
```

2. Add your configuration settings (example):

```toml
[ssh]
authorized_keys_path = "/home/azureuser/.ssh/authorized_keys"
query_sshd_config = true

[imds]
connection_timeout_secs = 5.0
```

3. Restart the service:

```bash
sudo systemctl restart azure-init
```

## For Developers

### Setting Up the Development Environment

1. **Install Rust:**

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

2. **Fork and Clone the Repository:**

```bash
git clone https://github.com/{your-fork}/azure-init.git
cd azure-init
```

3. **Install dependencies:**

For Debian/Ubuntu:
```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libudev-dev
```

For RHEL/Fedora:
```bash
sudo dnf install -y gcc pkg-config systemd-devel
```

### Building the Project

Build all components:

```bash
cargo build --all
```

Or just the main binary:

```bash
cargo build --bin azure-init
```

### Running Tests

Run the unit tests:

```bash
cargo test --verbose --all-features --workspace
```

### Development Workflow

1. **Make code changes**
2. **Build and test locally:**

```bash
cargo build
cargo test
```

3. **Test with a local configuration:**

**Warning**: Avoid running the `azure-init` binary locally, as this runs the risk of modifying your local system.
To test your changes, it is highly advised to use the [E2E Testing Guide](e2e_testing.md).

### Debugging

If these changes are part of a PR to the main Azure-Init repo, the CI pipeline will run a mock IMDS server and the azure-init binary in two separate Docker containers.
Both containers will output all logs they have access to in order to better debug where failures are taking place.

## Next Steps

- Review the [Configuration Guide](configuration.md) for detailed configuration options
- Understand the [Tracing System](libazurekvp.md) for monitoring and debugging
- Explore [End-to-End Testing](e2e_testing.md) for comprehensive testing
