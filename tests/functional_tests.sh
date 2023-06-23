#!/bin/bash
# Pre-req: You need to have the azure cli installed
# and run this script from a bash shell
# You will also need to make this script executable with 
# chmod a+x azure-cli-script.sh

# You are expected to already have a resource group created in your Azure subscription
# CLI: https://learn.microsoft.com/en-us/azure/azure-resource-manager/management/manage-resource-groups-cli
# Portal: https://learn.microsoft.com/en-us/azure/azure-resource-manager/management/manage-resource-groups-portal


# You are also expected to already have an Azure ssh key pair created
# with the private key stored in your ~/.ssh directory (or wherever is appropriate)
# And the permissions changed on the private key file to 600
# (You could also modify this script to use auto generated keys)
# chmod 600 ~/.ssh/private_key_file
# CLI: https://learn.microsoft.com/en-us/azure/virtual-machines/ssh-keys-azure-cli
# Portal: https://learn.microsoft.com/en-us/azure/virtual-machines/ssh-keys-portal


# Fill in these variables with the appropriate values 
# for your Azure subscription, resource group, etc.
SUBSCRIPTION_ID="YOUR_SUBSCRIPTION_ID"
RESOURCE_GROUP_NAME="RESOURCE_GROUP_NAME" # e.g. myresourcegroup
REGION="AZURE_REGION" # e.g. westus2
VM_NAME="VM_NAME" # e.g. myvm
VM_IMAGE="VM_IMAGE" # e.g. Ubuntu2204
VM_SIZE="VM_SIZE" # e.g. Standard_D2lds_v5
VM_ADMIN_USERNAME="ADMIN_USERNAME" # e.g. azureuser
AZURE_SSH_KEY_NAME="AZURE_KEY_PAIR_NAME" # e.g. azure-ssh-key
PATH_TO_PRIVATE_SSH_KEY="LOCAL_PATH_TO_PRIVATE_KEY_FILE" # e.g. ~/.ssh/id_rsa

echo "Starting script"
echo "Logging you into Azure"

# Log into Azure (this will open a browser window prompting you to log in)
az login

# Set the subscription you want to use
az account set --subscription $SUBSCRIPTION_ID

# Adds timestamp to VM name to make it unique
timestamp=$(date +%s)
VM_NAME_WITH_TIMESTAMP=$VM_NAME-$timestamp

echo "Creating VM..."
az vm create -n $VM_NAME_WITH_TIMESTAMP \
-g $RESOURCE_GROUP_NAME \
--image $VM_IMAGE \
--size $VM_SIZE \
--admin-username $VM_ADMIN_USERNAME \
--ssh-key-name $AZURE_SSH_KEY_NAME 

echo "Getting VM Public IP Address..."
PUBLIC_IP=$(az vm show -d -g $RESOURCE_GROUP_NAME -n $VM_NAME_WITH_TIMESTAMP --query publicIps -o tsv)
echo $PUBLIC_IP

# Give the VM some time to fully start up before attempting to ssh into it
echo "Sleep for 30 seconds to give the VM time to fully start up"
sleep 30

echo "Logging into VM..."
ssh -i $PATH_TO_PRIVATE_SSH_KEY $VM_ADMIN_USERNAME@$PUBLIC_IP

# Begin preparing SSH environment
apt-get update

apt-get install rustc


# make e2e-test
