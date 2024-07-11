#!/bin/bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT License.

SUBSCRIPTION_ID="${SUBSCRIPTION_ID:-}"
EPOCH=$(date +%s)
RG="${RG:-e2etest-azinit-$EPOCH}"
LOCATION="${LOCATION:-eastus}"
PATH_TO_PUBLIC_SSH_KEY="$HOME/.ssh/id_rsa.pub"
PATH_TO_PRIVATE_SSH_KEY="$HOME/.ssh/id_rsa"
VM_NAME="${VM_NAME:-AzInitFunctionalTest}"
VM_IMAGE="${VM_IMAGE:-Canonical:0001-com-ubuntu-server-jammy:22_04-lts:latest}"
VM_SIZE="${VM_SIZE:-Standard_D2lds_v5}"
VM_ADMIN_USERNAME="${VM_ADMIN_USERNAME:-azureuser}"
AZURE_SSH_KEY_NAME="${AZURE_SSH_KEY_NAME:-azure-ssh-key}"
VM_NAME_WITH_TIMESTAMP=$VM_NAME-$EPOCH
VM_SECURITY_TYPE="${VM_SECURITY_TYPE:-TrustedLaunch}"

set -e

echo "Starting script"

if [ -z "${SUBSCRIPTION_ID}" ] ; then
    echo "SUBSCRIPTION_ID missing. Either set environment variable or edit $0 to set a subscription ID."
    exit 1
fi

if [ ! -f "$PATH_TO_PUBLIC_SSH_KEY" ]; then
    ssh-keygen -t rsa -b 4096 -f "$PATH_TO_PRIVATE_SSH_KEY" -N ""
    echo "SSH key created."
else
    echo "SSH key already exists."
fi

# Log into Azure (this will open a browser window prompting you to log in)
if az account get-access-token -o none; then
    echo "Using existing Azure account"
else
    echo "Logging you into Azure"
    az login
fi

# Set the subscription you want to use
az account set --subscription "$SUBSCRIPTION_ID"

# Create resource group
az group create -g "$RG" -l "$LOCATION"

echo "Creating VM..."
az vm create -n "$VM_NAME_WITH_TIMESTAMP" \
-g "$RG" \
--image "$VM_IMAGE" \
--size "$VM_SIZE" \
--admin-username "$VM_ADMIN_USERNAME" \
--ssh-key-value "$PATH_TO_PUBLIC_SSH_KEY" \
--public-ip-sku Standard \
--security-type "$VM_SECURITY_TYPE"
echo "VM successfully created"

echo "Sleeping to ensure SSH access set up"
sleep 15

echo "Getting VM Public IP Address..."
PUBLIC_IP=$(az vm show -d -g "$RG" -n "$VM_NAME_WITH_TIMESTAMP" --query publicIps -o tsv)
echo "$PUBLIC_IP"

scp -o StrictHostKeyChecking=no -i "$PATH_TO_PRIVATE_SSH_KEY" ./target/debug/functional_tests "$VM_ADMIN_USERNAME"@"$PUBLIC_IP":~

echo "Logging into VM..."
ssh -o StrictHostKeyChecking=no -i "$PATH_TO_PRIVATE_SSH_KEY" "$VM_ADMIN_USERNAME"@"$PUBLIC_IP" 'sudo ./functional_tests test_user'

# Delete the resource group
az group delete -g "$RG" --yes --no-wait
