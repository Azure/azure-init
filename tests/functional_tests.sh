SUBSCRIPTION_ID="0a2c89a7-a44e-4cd0-b6ec-868432ad1d13"
EPOCH=$(date +%s)
RG=cade-test-azprovagent-$EPOCH
LOCATION=eastus
PATH_TO_PUBLIC_SSH_KEY="$HOME/.ssh/id_rsa.pub"
PATH_TO_PRIVATE_SSH_KEY="$HOME/.ssh/id_rsa"
VM_NAME="AzProvAgentFunctionalTest"
VM_IMAGE="Canonical:0001-com-ubuntu-server-jammy:22_04-lts:latest"
VM_SIZE="Standard_D2lds_v5"
VM_ADMIN_USERNAME="azureuser"
AZURE_SSH_KEY_NAME="azure-ssh-key"
VM_NAME_WITH_TIMESTAMP=$VM_NAME-$EPOCH

set -e

echo "Starting script"

if [ ! -f "$PATH_TO_PUBLIC_SSH_KEY" ]; then
    ssh-keygen -t rsa -b 4096 -f $PATH_TO_PUBLIC_SSH_KEY -N ""
    echo "SSH key created."
else
    echo "SSH key already exists."
fi

# Log into Azure (this will open a browser window prompting you to log in)
echo "Logging you into Azure"
az login

# Ensure Azure is logged in
az account get-access-token -o none

# Set the subscription you want to use
az account set --subscription $SUBSCRIPTION_ID

# Create resource group
az group create -g $RG -l $LOCATION

echo "Creating VM..."
az vm create -n $VM_NAME_WITH_TIMESTAMP \
-g $RG \
--image $VM_IMAGE \
--size $VM_SIZE \
--admin-username $VM_ADMIN_USERNAME \
--ssh-key-value $PATH_TO_PUBLIC_SSH_KEY \
--public-ip-sku Standard
echo "VM successfully created"

echo "Sleeping to ensure SSH access set up"
sleep 15

echo "Getting VM Public IP Address..."
PUBLIC_IP=$(az vm show -d -g $RG -n $VM_NAME_WITH_TIMESTAMP --query publicIps -o tsv)
echo $PUBLIC_IP

scp -o StrictHostKeyChecking=no -i $PATH_TO_PRIVATE_SSH_KEY ./target/debug/functional_tests $VM_ADMIN_USERNAME@$PUBLIC_IP:~

echo "Logging into VM..."
ssh -o StrictHostKeyChecking=no -i $PATH_TO_PRIVATE_SSH_KEY $VM_ADMIN_USERNAME@$PUBLIC_IP 'sudo ./functional_tests test_user' 

# Delete the resource group
az group delete -g $RG --yes --no-wait
