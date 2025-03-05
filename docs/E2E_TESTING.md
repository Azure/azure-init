# Azure-init End-to-end Testing

End-to-end tests validate the integration of azure-init in a real Azure VM environment. These tests verify that azure-init can properly provision Linux VMs according to Azure metadata.

## Prerequisites

Before running end-to-end tests, ensure you have:

1. **Azure CLI** installed and configured
2. An active **Azure subscription** (set as `SUBSCRIPTION_ID` environment variable)
3. **SSH keys** set up (default: `~/.ssh/id_rsa` and `~/.ssh/id_rsa.pub`)
4. **jq** installed for JSON parsing
5. Appropriate **Azure permissions** to create resource groups, VMs, and Shared Image Gallery resources

## Quickstart

To run e2e tests, use the following command from the repository root:

```bash
SUBSCRIPTION_ID=<your-subscription-id> make e2e-test
```

This command will:

1. Create a test user and associated SSH directory
2. Place mock SSH keys for testing
3. Run the tests and clean up any test artifacts generated during the process

## Details

End-to-end testing of azure-init consists of 2 steps: preparation of a SIG (Shared Image Gallery) image, and the actual testing.

### What is a Shared Image Gallery (SIG)?

Azure Shared Image Gallery is a service that helps you build structure and organization around your custom VM images. It provides:

- Global replication of images
- Versioning and grouping of images
- Highly available images for scaling deployments

In our testing, we use SIG to create a custom VM image with azure-init pre-installed, allowing us to test the provisioning process in a controlled environment.

### Preparation of Azure SIG image

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

### Running end-to-end testing

After creating your SIG image, run the tests with:

```bash
VM_IMAGE="$(az sig image-definition list --resource-group testgalleryazinitrg --gallery-name testgalleryazinit | jq -r .[].id)" make e2e-test
```

This process:

1. Creates a new VM using your SIG image
2. Copies the `functional_tests` binary to the VM
3. Runs tests to verify azure-init's provisioning behavior
4. Validates that user accounts, SSH keys, and hostname are correctly set up
5. Cleans up resources after testing completes

Custom environment variables can be passed:

```bash
RG="mytest-azinit" LOCATION="westeurope" VM_SIZE="Standard_D2ds_v5" VM_IMAGE="$(az sig image-definition list --resource-group testgalleryazinitrg --gallery-name testgalleryazinit | jq -r .[].id)" make e2e-test
```

### What the tests actually validate

The functional tests verify that azure-init correctly:

1. Processes Azure VM metadata from the IMDS endpoint
2. Sets up user accounts as specified in the provisioning data
3. Configures SSH keys for secure access
4. Sets the hostname according to Azure VM specifications
5. Handles password configuration properly

### Cleanup

When testing is done, clean up the SIG resource group:

```bash
az group delete --resource-group testgalleryazinitrg
```

## Troubleshooting

### Common Issues

1. **Azure CLI not authenticated**
   - Run `az login` before running tests

2. **Missing SUBSCRIPTION_ID**
   - Export your subscription ID: `export SUBSCRIPTION_ID=<your-subscription-id>`

3. **SSH Key Issues**
   - If you don't have SSH keys, the script will generate them
   - To use custom keys, set `PATH_TO_PUBLIC_SSH_KEY` and `PATH_TO_PRIVATE_SSH_KEY`

4. **VM Creation Failures**
   - Check your quota limits in the Azure region you're using
   - Verify you have permissions to create VMs with the specified size

5. **Image Creation Failures**
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
