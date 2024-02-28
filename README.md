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

There are two different sets of tests: unit tests and end-to-end (e2e tests). To run unit tests, use `cargo test`. 
To run end-to-end testing, use `make e2e-test`, which will create a test user, ssh directory, place mock ssh keys, and 
then clean up the test artifacts afterwards.

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
