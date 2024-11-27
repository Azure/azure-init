# Azure-init End-to-end Testing

End-to-end tests validate the integration of the entire system. These tests require additional setup, such as setting a subscription ID.

## Quickstart

To run e2e tests, use the following command from the repository root:

```
make e2e-test
```

This command will:

1. Create a test user and associated SSH directory.
2. Place mock SSH keys for testing.
3. Run the tests and then clean up any test artifacts generated during the process.

## Details

End-to-end testing of azure-init consists of 2 steps: preparation of SIG(Shared Image Gallery) image, and the actual testing.

### Preparation of Azure SIG image

To create an Azure SIG image to be used for end-to-end testing, run `image_creation.sh`.
That will create a resource group, a storage account, a virtual machine, generate a SIG image, and publish the SIG image.

```
demo/image_creation.sh
```

If you want to run the script with custom variables for resource group, VM location, VM size, base image URN, etc., then specify corresponding environment variables. For example:

```
RG="mytest-azinit" LOCATION="westeurope" VM_SIZE="Standard_D2ds_v5" BASE_IMAGE="Canonical:0001-com-ubuntu-server-jammy:22_04-lts-gen2:latest" demo/image_creation.sh
```

The current limitation is, however, that the `BASE_IMAGE` should be one of Debian-derivatives like Ubuntu. When the build host OS is different from the target host OS, the `functional_test` binary might not be able to run due to mismatch of package versions such as glibc.

### Running end-to-end testing

To run end-to-end testing, use `make e2e-test`, which will create a test user, ssh directory, place mock ssh keys, and
then clean up the test artifacts afterwards.

`VM_IMAGE` should be specified to pick the correct SIG image created in the previous step.

```
VM_IMAGE="$(az sig image-definition list --resource-group testgalleryazinitrg --gallery-name testgalleryazinit | jq -r .[].id)" make e2e-test
```

It is also possible to pass custom environment variables. For example:

```
RG="mytest-azinit" LOCATION="westeurope" VM_SIZE="Standard_D2ds_v5" VM_IMAGE="$(az sig image-definition list --resource-group testgalleryazinitrg --gallery-name testgalleryazinit | jq -r .[].id)" make e2e-test
```

When testing is done, it is recommended to clean up resource group for SIG images.

```
az group delete --resource-group testgalleryazinitrg
```
