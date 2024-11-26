# Azure-Init

[![Github CI](https://github.com/Azure/azure-init/actions/workflows/ci.yaml/badge.svg)](https://github.com/Azure/azure-init/actions)

A reference implementation for provisioning Linux VMs on Azure.

Azure-init configures Linux guests from provisioning metadata.
Contrary to complex guest configuration and customisation systems like e.g. cloud-init, azure-init aims to be minimal.
It strictly focuses on basic instance initialisation from Azure metadata.

Azure-init has very few requirements on its environment, so it may run in a very early stage of the boot process.

## Installing Rust

To install Rust see here: https://www.rust-lang.org/tools/install.

## Building the Project

Building this project can be done by going to the base of the repository in the command line and entering the command
`cargo build --all`. This project contains two binaries, the main provisioning agent and the functional testing binary,
so this command builds both. These binaries are quite small, but you can build only one by entering
`cargo build --bin <binary_name>` and indicating either `azure-init` or `functional_tests`.

To run the program, you must enter the command `cargo run --bin <binary_name>` and indicating the correct binary.

## Testing

Azure-init includes two types of tests: unit tests and end-to-end (e2e) tests.

### Running Unit Tests

To run unit tests, use the following commands based on the scope of testing:

1. **Repository-Wide Unit Tests:**
From the root directory of the repository, run:

```
cargo test
```

This will execute the unit tests defined in the top-level binaries and modules but will **not** include tests from submodules like libazureinit.

2. **Unit Tests in** `libazureinit`:

To run the full set of unit tests, including those within the `libazureinit` library, navigate to the `libazureinit` directory and run:
```
cd libazureinit
cargo test
```

### Running End-to-End (e2e) Tests
End-to-end tests validate the integration of the entire system. These tests require additional setup, such as setting a subscription ID. To run e2e tests, use the following command from the repository root:

```
make e2e-test
```

This command will:

1. Create a test user and associated SSH directory.
2. Place mock SSH keys for testing.
3. Run the tests and then clean up any test artifacts generated during the process.

## Contributing

Contribution require you to agree to Microsoft's Contributor License Agreement (CLA).
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
