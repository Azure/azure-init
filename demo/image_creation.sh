#!/bin/bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

set -e -u -o pipefail

USAGE="Usage: $0 [options]
It is possible to pass in custom parameters like:

RG=\"myrg\" LOCATION=\"westeurope\" VM_SIZE=\"Standard_D2ds_v5\" \\
    BASE_IMAGE=\"Debian:debian-11:11-backports-gen2:latest\" \\
    $0

Options:
    -v|--verbose    Print out debug message
    -h|--help       This help message
"

while [[ $# -gt 0 ]] ; do
    case "$1" in
        -h|--help)
            echo "$USAGE"
            exit 0
            ;;
        -v|--verbose)
            set -x
	    shift
            ;;
        *)
            break
    esac
done

EPOCH=$(date +%s)
TEMP_DIR=/tmp/staging.$EPOCH
STAGING_DIR=$TEMP_DIR/install
echo "*********************************************************************"
echo "Building the agent"
echo "*********************************************************************"

ROOT_DIR=$(git rev-parse --show-toplevel)
echo "*********************************************************************"
echo "Staging artifacts to $STAGING_DIR"
echo "*********************************************************************"
make install DESTDIR="$STAGING_DIR"
cp "$ROOT_DIR"/demo/customdata_template.yml "$TEMP_DIR"/customdata.yml
echo "Done"

echo "*********************************************************************"
echo "Creating azure-init.tgz package for upload"
echo "*********************************************************************"
rm -f ./azure-init.tgz
tar cvfz azure-init.tgz -C "$STAGING_DIR" .
echo "Done"

RG="${RG:-testagent-$EPOCH}"
STORAGE_ACCOUNT="${STORAGE_ACCOUNT:-azinitsa$EPOCH}"
STORAGE_CONTAINER="${STORAGE_CONTAINER:-azinitcontainer}"
LOCATION="${LOCATION:-eastus}"

echo "*********************************************************************"
echo "Creating resource group $RG"
echo "*********************************************************************"
az group create -g "$RG" -l "$LOCATION"
echo "Done"

echo "*********************************************************************"
echo "Creating storage account $STORAGE_ACCOUNT"
echo "*********************************************************************"
az storage account create -g "$RG" -l "$LOCATION" -n "$STORAGE_ACCOUNT" --sku Standard_LRS --allow-shared-key-access false -o none
echo "Done"

echo "*********************************************************************"
echo "Creating storage container $STORAGE_CONTAINER with account $STORAGE_ACCOUNT"
echo "*********************************************************************"
az storage container create --name "$STORAGE_CONTAINER" --account-name "$STORAGE_ACCOUNT" --auth-mode login
echo "Done"

echo "*********************************************************************"
echo "Generating a SAS for azure-init-$EPOCH.tgz"
echo "*********************************************************************"
EXPIRY=$(date -u -d '10 days' '+%Y-%m-%dT%H:%MZ')
SASURL=$(az storage blob generate-sas \
  --account-name "$STORAGE_ACCOUNT" \
  --container-name "$STORAGE_CONTAINER" \
  --name azure-init-"$EPOCH".tgz \
  --permissions r \
  --expiry "$EXPIRY" \
  --https-only \
  --auth-mode login \
  --as-user \
  --full-uri)
echo "Done"

echo "*********************************************************************"
echo "Generating customdata"
echo "*********************************************************************"
sed -i "s __SASURL__ ${SASURL//&/\\&} g" "$TEMP_DIR"/customdata.yml
echo "Done"

VM_NAME="${VM_NAME:-testvm-$EPOCH}"
VM_SIZE="${VM_SIZE:-Standard_D2ds_v5}"
SSH_KEY_PATH=~/.ssh/id_rsa.pub
BASE_IMAGE="${BASE_IMAGE:-canonical:0001-com-ubuntu-server-jammy:22_04-lts-gen2:latest}"
ADMIN_USERNAME="${ADMIN_USERNAME:-testuser-$EPOCH}"

# Both "az vm create" and "az sig image-definition" should use the same security type.
SECURITY_TYPE="${SECURITY_TYPE:-TrustedLaunch}"

echo "*********************************************************************"
echo "Uploading package as azure-init-$EPOCH.tgz"
echo "*********************************************************************"
az storage blob upload --account-name "$STORAGE_ACCOUNT" --container-name "$STORAGE_CONTAINER" --file azure-init.tgz --name azure-init-"$EPOCH".tgz --auth-mode login
echo "Done"

echo "*********************************************************************"
echo "Creating vm $VM_NAME with user $ADMIN_USERNAME in $LOCATION"
echo "*********************************************************************"
az vm create -g "$RG" -n "$VM_NAME" --image "$BASE_IMAGE" --admin-username "$ADMIN_USERNAME" --ssh-key-value @${SSH_KEY_PATH} --boot-diagnostics-storage "$STORAGE_ACCOUNT" --size "${VM_SIZE}" --accelerated-network true --nic-delete-option Delete --os-disk-delete-option Delete --custom-data "$TEMP_DIR"/customdata.yml --security-type "$SECURITY_TYPE"
echo "vm finished deployment, waiting for image configuration to finish"

DEADLINE=$(date -u -d '15 minutes' '+%s')
FOUND=0
while [[ $(date '+%s') < $DEADLINE ]]
do
    POWER_STATE=$(az vm get-instance-view -g "$RG" -n "$VM_NAME" | jq '.instanceView.statuses[] | select(.code | contains("PowerState")) | .code' -r)
    if [[ "$POWER_STATE" == *"stopped"* ]]
    then
        LOG=$(az vm boot-diagnostics get-boot-log -g "$RG" -n "$VM_NAME")
        if [[ -n "$LOG" && "$LOG" == *"SIGTOOL_END"* ]]
        then
            echo "vm configured successfully"
            FOUND=1
            break
        fi
    fi
    sleep 15
done

if [[ $FOUND -eq 0 ]]
then
    echo "vm failed to configure in 15 minutes - abort"
    exit 1
fi

IMAGE_NAME=image-$EPOCH
echo "*********************************************************************"
echo "Capturing OsDisk snapshot of vm $VM_NAME with image $IMAGE_NAME"
echo "*********************************************************************"
TARGET_DISK=$(az disk show --ids "$(az vm show -g "$RG" -n "$VM_NAME" | jq .storageProfile.osDisk.managedDisk.id -r)" | jq .name -r)
az snapshot create -g "$RG" -n "$VM_NAME"-snapshot --source "$TARGET_DISK"

IMAGE_VERSION=$(date '+%Y.%m%d.%H%M%S')
GALLERY_NAME=testgalleryazinit
GALLERY_DEFINITION=testgalleryazinitdef
GALLERY_RG=testgalleryazinitrg
SNAPSHOT_ID=$(az snapshot show -n "$VM_NAME"-snapshot -g "$RG" --query id --output tsv)

# For example, if BASE_IMAGE is set to "canonical:0001-com-ubuntu-server-jammy:22_04-lts-gen2:latest",
# then GALLERY_PUBLISHER becomes "canonical", GALLERY_OFFER becomes
# "0001-com-ubuntu-server-jammy", GALLERY_SKU becomes "22_04-lts-gen2".
GALLERY_PUBLISHER="$(echo "$BASE_IMAGE" | cut -f1 -d:)"
GALLERY_OFFER="$(echo "$BASE_IMAGE" | cut -f2 -d:)"
GALLERY_SKU="$(echo "$BASE_IMAGE" | cut -f3 -d:)"

az group create --location "$LOCATION" --resource-group "$GALLERY_RG"

az sig create --resource-group "$GALLERY_RG" --gallery-name "$GALLERY_NAME"

echo "*********************************************************************"
echo "Creating image definition of $BASE_IMAGE"
echo "*********************************************************************"
az sig image-definition create --resource-group "$GALLERY_RG" --gallery-name "$GALLERY_NAME" --gallery-image-definition "$GALLERY_DEFINITION" --offer "$GALLERY_OFFER" --publisher "$GALLERY_PUBLISHER" --sku "$GALLERY_SKU" --os-type linux --os-state Generalized --features SecurityType="$SECURITY_TYPE"

echo "*********************************************************************"
echo "Publishing image version $IMAGE_VERSION to $GALLERY_NAME/$GALLERY_DEFINITION"
echo "*********************************************************************"
if az sig image-version create -g $GALLERY_RG --gallery-name $GALLERY_NAME --gallery-image-definition $GALLERY_DEFINITION --gallery-image-version "$IMAGE_VERSION" --os-snapshot "$SNAPSHOT_ID" --target-regions "$LOCATION" --replica-count 1;
then
    echo "Image publishing finished"
    echo "Deleting staging resource group"
    az group delete -g "$RG" --yes --no-wait
    echo "Done"
else
    echo "Image publishing failed"
    echo "Resource group $RG is kept for debugging"
    exit 1
fi
