# Azure-Init

[![Github CI](https://github.com/Azure/azure-init/actions/workflows/ci.yaml/badge.svg)](https://github.com/Azure/azure-init/actions)

A reference implementation for provisioning Linux VMs on Azure.

## What is Azure-init?

Azure-init is a lightweight provisioning agent that configures Linux virtual machines using Azure metadata. Unlike complex guest configuration systems (such as cloud-init), azure-init focuses exclusively on the essential initialization tasks for Azure VMs:

- Setting up user accounts
- Configuring SSH keys for authentication
- Setting the hostname
- Managing passwords
- Processing VM provisioning metadata

Azure-init is designed to be minimal, fast, and reliable, with very few dependencies. This allows it to run in the early stages of the boot process when initializing Linux VMs in Azure.

## Key Features

- **Minimal footprint**: Small binary size and few dependencies
- **Fast execution**: Optimized for quick VM provisioning
- **Early boot compatibility**: Can run in very early boot stages
- **Azure-specific**: Tailored for the Azure environment
- **Configurable**: Allows customization through configuration files

## Architecture

Azure-init consists of two main components:

1. **azure-init** - The main provisioning agent binary
2. **libazureinit** - A library that provides core functionality for accessing Azure services

The agent communicates with the Azure Instance Metadata Service (IMDS) to retrieve VM-specific configuration data, and then applies the appropriate configurations to the Linux system.

## Getting Started

### Prerequisites

- Rust programming environment
- Access to an Azure subscription (for e2e testing)

### Installing Rust

To install Rust see here: https://www.rust-lang.org/tools/install.

### Building the Project

Building this project can be done by going to the base of the repository in the command line and entering the command
`cargo build --all`. This project contains two binaries, the main provisioning agent and the functional testing binary,
so this command builds both. These binaries are quite small, but you can build only one by entering
`cargo build --bin <binary_name>` and indicating either `azure-init` or `functional_tests`.

To run the program, you must enter the command `cargo run --bin <binary_name>` and indicating the correct binary.

## Configuration

Azure-init supports customization through configuration files. The default configuration path is `/etc/azure-init/azure-init.toml`, but additional configuration can be provided in the `/etc/azure-init/azure-init.toml.d/` directory.

For detailed information about configuration options and structure, see [configuration.md](doc/configuration.md).

## Testing

Azure-init includes two types of tests: unit tests and end-to-end (e2e) tests.

### Running Unit Tests

From the root directory of the repository, run:

```
cargo test --verbose --all-features --workspace
```

This will run the unit tests for every library in the repository, not just for azure-init. 
Doing so ensures your testing will match what is run in the CI pipeline. 

### Running End-to-End (e2e) Tests
Please refer to [E2E_TESTING.md](docs/E2E_TESTING.md) for end-to-end testing.

## Advanced Topics

### Understanding the Tracing System
Azure-init includes a sophisticated tracing system for monitoring and debugging. For details, see [libazurekvp.md](doc/libazurekvp.md).

## Contributing

Contributions require you to agree to Microsoft's Contributor License Agreement (CLA).
Please refer to [CONTRIBUTING.md](CONTRIBUTING.md) for detailed instructions.

This project adheres to the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/).
Check out [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for a brief collection of links and references.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft 
trademarks or logos is subject to and must follow 
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.

## libazureinit

For common library used by this reference implementation, please refer to [libazureinit](https://github.com/Azure/azure-init/tree/main/libazureinit/).