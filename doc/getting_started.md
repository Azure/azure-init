# Getting Started with Azure-init

This guide provides step-by-step instructions to get started with azure-init for both development and usage scenarios.

## For Users

### Prerequisites

- An Azure subscription
- A Linux VM running in Azure

### Basic Usage

Azure-init is typically pre-installed and configured in Linux VM images that are Azure-optimized. If you're using such an image, azure-init will automatically run during the boot process and handle VM initialization.

### Verifying Azure-init Operation

To check that azure-init is working properly on your VM:

1. Connect to your VM via SSH
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

2. **Clone the repository:**

```bash
git clone https://github.com/Azure/azure-init.git
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

```bash
cargo run --bin azure-init -- --config /path/to/your/config.toml
```

4. **For more thorough testing, use E2E tests** (see [E2E Testing Guide](../docs/E2E_TESTING.md))

### Debugging

1. **Enable debug logging:**

```bash
RUST_LOG=debug cargo run --bin azure-init
```

2. **Test with mock data:**

Create a file with mock Azure metadata and use it for testing:

```bash
# Create a mock IMDS response
echo '{"compute":{"name":"testvm"}}' > /tmp/mock-imds.json

# Use the mock data
AZURE_INIT_MOCK_IMDS_FILE=/tmp/mock-imds.json cargo run --bin azure-init
```

## Next Steps

- Review the [Configuration Guide](configuration.md) for detailed configuration options
- Learn about the [Architecture](architecture.md) of azure-init
- Understand the [Tracing System](libazurekvp.md) for monitoring and debugging
- Explore [End-to-End Testing](../docs/E2E_TESTING.md) for comprehensive testing