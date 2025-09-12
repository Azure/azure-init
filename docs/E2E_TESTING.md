# Azure-init End-to-end Testing

End-to-end tests validate the integration of azure-init in a real Azure VM environment. These tests verify that azure-init can properly provision Linux VMs according to Azure metadata.

## Prerequisites

Before running end-to-end tests, ensure you have:

1. **Azure CLI** installed and configured
2. An active **Azure subscription** (set as `SUBSCRIPTION_ID` environment variable)
3. **SSH keys** set up (default: `~/.ssh/id_rsa` and `~/.ssh/id_rsa.pub`)
4. **jq** installed for JSON parsing
5. Appropriate **Azure permissions** to create resource groups, VMs, and Shared Image Gallery resources

## Two Testing Approaches

There are two ways to run end-to-end tests for azure-init, both of which involve creating resources in your Azure cloud subscription:

1. **Direct VM Testing (Simplest)** - Creates a VM using a standard Ubuntu image and runs functional tests directly
2. **SIG Image Testing (Advanced)** - Creates a custom SIG image with azure-init pre-installed, then tests with that image

For most users, the **Direct VM Testing** approach is recommended as it's simpler and faster.

## Important: Binary Compatibility Between Build and Target Environments

When running end-to-end tests, an important consideration is the binary compatibility between your build environment (where the `functional_tests` binary is compiled) and the target environment (the Azure VM where the tests will run).

### Understanding the Issue

The `functional_tests` binary is:
1. Built on your local system
2. Copied to an Azure VM via SSH
3. Executed on the VM to test azure-init functionality

If your build environment has newer libraries (especially glibc) than the target VM, you may encounter compatibility errors like:

```
./functional_tests: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_X.XX' not found
```

This occurs because your local build is dynamically linked against a newer version of glibc than what's available on the standard Ubuntu VM image in Azure.

### General Solutions

There are several approaches to solve this binary compatibility issue:

1. **Build on a matching environment**: Ensure your build environment matches the target VM OS version/distribution
2. **Use static linking**: Configure Rust to statically link the binary (may increase binary size)
3. **Cross-compilation**: Use cross-compilation tools to build for the target environment
4. **Containerization**: Use Docker or another container platform to build in an environment matching the target
5. **Target older glibc**: Configure the linker to target an older glibc version

### Example: Using Docker to Build Compatible Binaries

One approach is to use Docker to build the binary in an environment that matches the target VM:

```yaml
# Example docker-compose.yml
version: '3'

services:
  build-functional-tests:
    image: ubuntu:22.04  # Match the Azure VM image OS version
    volumes:
      - .:/azure-init
    working_dir: /azure-init
    command: >
      bash -c "
        apt-get update && 
        apt-get install -y curl build-essential pkg-config libssl-dev git &&
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y &&
        . $$HOME/.cargo/env &&
        cargo build --bin functional_tests
      "
```

This approach ensures the binary is built in an environment that matches the target VM OS version and libraries.

## Direct VM Testing (Recommended)

### About Direct VM Testing

This approach works by:
1. Building the `functional_tests` binary in a compatible environment
2. Creating a temporary VM in your Azure subscription using a standard Ubuntu image
3. Copying the functional tests binary to the VM via SSH
4. Running the tests on the VM to validate functionality
5. Automatically deleting the VM and resource group after testing

The entire process happens in your Azure cloud subscription, but the test is controlled from your local machine. This approach is faster because it skips the image creation process, but it may not test the actual VM provisioning with azure-init since azure-init is not pre-installed in the standard image.

### Quickstart

To run e2e tests using the direct approach, use the following command from the repository root:

```bash
export SUBSCRIPTION_ID=$(az account show --query id -o tsv)
make e2e-test
```

This command will:

1. Build the functional tests binary
2. Create a new VM in your Azure subscription using a standard Ubuntu image 
3. Copy the functional tests binary to the VM
4. Run the tests on the VM
5. Automatically clean up resources when done

### Custom Configuration

You can customize the VM creation with environment variables:

```bash
RG="mytest-azinit" LOCATION="westus2" VM_SIZE="Standard_D2s_v3" make e2e-test
```

## Advanced SIG Image Testing

For more advanced testing scenarios, you can create a custom Shared Image Gallery (SIG) image with azure-init pre-installed.

### About SIG Image Testing

This approach provides a more complete end-to-end test because:
1. It creates a custom VM image with azure-init pre-installed and properly configured
2. The VM provisioning process actually uses azure-init during boot
3. It allows testing of the full provisioning flow from VM creation to user setup

This approach is more thorough but takes significantly longer (30+ minutes for image creation) as it involves:
1. Creating a base VM
2. Installing azure-init on that VM
3. Generalizing the VM
4. Capturing it as a SIG image
5. Creating a second VM from that image
6. Running tests on the second VM

All of these steps happen in your Azure cloud subscription, controlled from your local machine.

**Note**: When using SIG image testing, you should still use the Docker-built binary to ensure compatibility with the target VM environment.

### What is a Shared Image Gallery (SIG)?

Azure Shared Image Gallery is a service that helps you build structure and organization around your custom VM images. In our testing, we can use SIG to create a custom VM image with azure-init pre-installed, allowing us to test the provisioning process in a controlled environment.

The SIG is created only within your own Azure subscription and is not publicly accessible.

### Step 1: Preparation of Azure SIG image

To create an Azure SIG image for end-to-end testing:

```bash
demo/image_creation.sh
```

This script:

1. Creates an Azure resource group and storage account
2. Deploys a virtual machine with a base image
3. Installs and configures azure-init on the VM
4. Generalizes the VM and captures it as a SIG image
5. Publishes the SIG image for testing

You can customize the image creation with environment variables:

```bash
RG="mytest-azinit" LOCATION="westeurope" VM_SIZE="Standard_D2ds_v5" BASE_IMAGE="Canonical:0001-com-ubuntu-server-jammy:22_04-lts-gen2:latest" demo/image_creation.sh
```

**Note**: The `BASE_IMAGE` should be a Debian derivative like Ubuntu. When the build host OS differs from the target host OS, the `functional_test` binary might not run due to package version mismatches (e.g., glibc).

### Step 2: Testing with the SIG image

After creating your SIG image, run the tests with:

```bash
VM_IMAGE="$(az sig image-definition list --resource-group testgalleryazinitrg --gallery-name testgalleryazinit | jq -r .[].id)" make e2e-test
```

### Cleanup After SIG Testing

When testing is done, clean up the SIG resource group:

```bash
az group delete --resource-group testgalleryazinitrg
```

## Comparison of Testing Approaches

| Feature | Direct VM Testing | SIG Image Testing |
|---------|------------------|-------------------|
| **Speed** | Faster (5-10 minutes) | Slower (30+ minutes) |
| **Completeness** | Tests functionality only | Tests full provisioning flow |
| **Resources Created** | Single VM | Multiple VMs + SIG resources |
| **Costs** | Lower Azure costs | Higher Azure costs |
| **Complexity** | Simple, one command | Multiple steps |
| **Tests azure-init Integration** | No (uses existing VM) | Yes (tests VM with azure-init) |

## How the E2E Testing Works

1. **Build Process**:
   - The `functional_tests` binary is built in your chosen environment
   - This binary must be compatible with the target Azure VM environment

2. **VM Creation**:
   - The `functional_tests.sh` script creates a resource group and VM in Azure
   - It uses standard Azure CLI commands to provision the VM
   - The VM is created with your SSH public key for authentication

3. **Test Execution**:
   - The compatible binary is copied to the VM using SCP
   - The script runs the binary on the VM with sudo permissions
   - The binary tests core azure-init provisioning functionality like user creation and SSH key setup

4. **Cleanup**:
   - The script automatically deletes the resource group to clean up all resources

## What the Tests Actually Validate

The functional tests verify that azure-init correctly:

1. Processes Azure VM metadata from the IMDS endpoint
2. Sets up user accounts as specified in the provisioning data
3. Configures SSH keys for secure access
4. Sets the hostname according to Azure VM specifications
5. Handles password configuration properly

## Troubleshooting

### Common Issues

1. **Azure CLI not authenticated**
   - Run `az login` before running tests

2. **Missing SUBSCRIPTION_ID**
   - Export your subscription ID: `export SUBSCRIPTION_ID=$(az account show --query id -o tsv)`

3. **SSH Key Issues**
   - If you don't have SSH keys, the script will generate them
   - To use custom keys, set `PATH_TO_PUBLIC_SSH_KEY` and `PATH_TO_PRIVATE_SSH_KEY`

4. **VM Creation Failures**
   - Check your quota limits in the Azure region you're using
   - Verify you have permissions to create VMs with the specified size

5. **Binary Compatibility Errors**
   - If you see `version GLIBC_X.XX not found` errors, it means your build environment has a newer glibc than the target VM
   - Options to resolve:
     - Build on a matching OS version (same as the target VM)
     - Use Docker to build in a compatible environment
     - Configure static linking in your Rust build

6. **Docker Not Running**
   - Ensure Docker daemon is running before executing `make e2e-test`
   - If Docker is not available, you may need to install it or start the service

7. **Image Creation Failures**
   - Check the resource group (default: `testgalleryazinitrg`) for error details
   - If the image fails to create, the resource group is preserved for debugging

### Advanced Configuration

For more granular control over testing, you can set these environment variables:

- `SUBSCRIPTION_ID`: Your Azure subscription ID
- `RG`: Resource group name (default: `e2etest-azinit-<timestamp>`)
- `LOCATION`: Azure region (default: `eastus`)
- `VM_NAME`: Base name for test VMs
- `VM_SIZE`: VM size (default: `Standard_D2lds_v5`)
- `VM_ADMIN_USERNAME`: Admin username (default: `azureuser`)
- `VM_IMAGE`: Image to use (URN or image ID)
- `VM_SECURITY_TYPE`: Security type (default: `TrustedLaunch`)