# Azure Provisioning Agent 

## Installing Rust

Rust can be installed via the command line using the following command: 
`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`. Following this installation process will also allow
for the use of Cargo, which is Rust's compiler and dependency manager. More on the usage of cargo can be found in the
building section.

## Pulling Source Code

This source code can accessed by cloning the repository to your machine with the command:
`git clone git@github.com:Azure/azure-provisioning-agent.git`

## Building the Project

Building this project can be done by going to the base of the repository in the command line and entering the command
`cargo build --all`. This project contains two binaries, the main provisioning agent and the functional testing binary,
so this command builds both. These binaries are quite small, but you can build only one by entering
`cargo build --bin <binary_name>` and indicating either `azure-provisioning-agent` or `functional_tests`.

To run the program, you must enter the command `cargo run --bin <binary_name>` and indicating the correct binary.

## Testing

There are two different sets of tests: unit tests and end-to-end (e2e tests). To run unit tests, use cargo test. 
To run end-to-end testing, use `make e2e-test`, which will create a test user, ssh directory, place mock ssh keys, and 
then clean up the test artifacts afterwards.

## Contributing

This project welcomes contributions and suggestions.  Most contributions require you to agree to a
Contributor License Agreement (CLA) declaring that you have the right to, and actually do, grant us
the rights to use your contribution. For details, visit https://cla.opensource.microsoft.com.

When you submit a pull request, a CLA bot will automatically determine whether you need to provide
a CLA and decorate the PR appropriately (e.g., status check, comment). Simply follow the instructions
provided by the bot. You will only need to do this once across all repos using our CLA.

This project has adopted the [Microsoft Open Source Code of Conduct](https://opensource.microsoft.com/codeofconduct/).
For more information see the [Code of Conduct FAQ](https://opensource.microsoft.com/codeofconduct/faq/) or
contact [opencode@microsoft.com](mailto:opencode@microsoft.com) with any additional questions or comments.

## Trademarks

This project may contain trademarks or logos for projects, products, or services. Authorized use of Microsoft 
trademarks or logos is subject to and must follow 
[Microsoft's Trademark & Brand Guidelines](https://www.microsoft.com/en-us/legal/intellectualproperty/trademarks/usage/general).
Use of Microsoft trademarks or logos in modified versions of this project must not cause confusion or imply Microsoft sponsorship.
Any use of third-party trademarks or logos are subject to those third-party's policies.
