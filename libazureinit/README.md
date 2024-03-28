# libazureinit

A common library for provisioning Linux VMs on Azure.

Features:

* retrieve provisioning metadata from Azure Instance Metadata Service
* configure the VM according to the provisioning metadata
* report provisioning complete to Azure platform
* basic features for instance initialisation

[azure-init](https://github.com/Azure/azure-init) is a reference implementation that leverages the APIs provided by libazureinit.

The goal is to provide APIs for other components to perform VM provisioning on Azure platform.

For other instructions, like installing Rust, building the project, testing, etc. please refer to [azure-init's README](https://github.com/Azure/azure-init/blob/main/README.md).
