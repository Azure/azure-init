#!/bin/bash
set -e -u -x -o pipefail

epoch=$(date +%s)
temp_dir=/tmp/staging.$epoch
target_dir=azure-provisioning-agent
staging_dir=$temp_dir/$target_dir
echo "*********************************************************************"
echo "Building the agent"
echo "*********************************************************************"

cargo build

root_dir=$(git rev-parse --show-toplevel)
echo "*********************************************************************"
echo "Staging artifacts to $staging_dir"
echo "*********************************************************************"
mkdir -p $staging_dir
cp $root_dir/target/debug/azure-provisioning-agent $staging_dir/
cp $root_dir/config/azure-provisioning-agent.service $staging_dir/
cp $root_dir/tests/customdata_template.yml $temp_dir/customdata.yml
echo "Done"

echo "*********************************************************************"
echo "Creating azure-provisioning-agent.tgz package for upload"
echo "*********************************************************************"
rm -f ./azure-provisioning-agent.tgz
tar cvfz azure-provisioning-agent.tgz -C $temp_dir $target_dir
echo "Done"

echo "*********************************************************************"
echo "Uploading package as azure-provisioning-agent-$epoch.tgz"
echo "*********************************************************************"
az storage blob upload --account-name aztuxprovisioningtest -c minagent --file azure-provisioning-agent.tgz --name azure-provisioning-agent-$epoch.tgz
echo "Done"

echo "*********************************************************************"
echo "Generating a SAS for azure-provisioning-agent-$epoch.tgz"
echo "*********************************************************************"
end=$(date -u -d '10 days' '+%Y-%m-%dT%H:%MZ')
sasurl=$(az storage blob generate-sas --account-name aztuxprovisioningtest -c minagent -n azure-provisioning-agent-$epoch.tgz --permissions r --expiry $end --https-only --full-uri)
echo "Done"

echo "*********************************************************************"
echo "Generating customdata"
echo "*********************************************************************"
sed -i "s __SASURL__ ${sasurl//&/\\&} g" $temp_dir/customdata.yml
echo "Done"

rg=testagent-$epoch
storage=testagent$epoch
location=eastus
vm=testvm-$epoch
ssh_key_path=~/.ssh/id_rsa.pub
base_image=canonical:0001-com-ubuntu-server-jammy:22_04-lts-gen2:latest
admin_username=testuser-$epoch

echo "*********************************************************************"
echo "Creating resource group $rg"
echo "*********************************************************************"
az group create -g $rg -l $location
echo "Done"

echo "*********************************************************************"
echo "Creating storage account $storage"
echo "*********************************************************************"
az storage account create -g $rg -l $location -n $storage --sku Standard_LRS -o none
echo "Done"

echo "*********************************************************************"
echo "Creating vm $vm with user $admin_username in $location"
echo "*********************************************************************"
az vm create -g $rg -n $vm --image $base_image --admin-username $admin_username --ssh-key-value @${ssh_key_path} --boot-diagnostics-storage $storage --size Standard_D2ds_v5 --accelerated-network true --nic-delete-option Delete --os-disk-delete-option Delete --custom-data $temp_dir/customdata.yml
echo "vm finished deployment, waiting for image configuration to finish"

deadline=$(date -u -d '15 minutes' '+%s')
found=0
while [[ $(date '+%s') < $deadline ]]
do
    power_state=$(az vm get-instance-view -g $rg -n $vm | jq '.instanceView.statuses[] | select(.code | contains("PowerState")) | .code' -r)
    if [[ "$power_state" == *"stopped"* ]]
    then
        log=$(az vm boot-diagnostics get-boot-log -g $rg -n $vm)
        if [[ -n "$log" && "$log" == *"SIGTOOL_END"* ]]
        then
            echo "vm configured successfully"
            found=1
            break
        fi
    fi
    sleep 15
done

if [[ $found -eq 0 ]]
then
    echo "vm failed to configure in 15 minutes - abort"
    exit 1
fi

image=image-$epoch
echo "*********************************************************************"
echo "Deallocating and generalizing vm to capture image $image"
echo "*********************************************************************"
az vm deallocate -g $rg -n $vm
az vm generalize -g $rg -n $vm
az image create -g $rg -n $image --source $vm --hyper-v-generation V2
echo "Done"

version=$(date '+%Y.%m%d.%H%M%S')
gallery=testgalleryagent
definition=testgallery
gallery_rg=temp-rg-rust-agent-testing
image_id=$(az image show -g $rg -n $image | jq .id -r)
echo "*********************************************************************"
echo "Publishing image version $version to $gallery/$definition"
echo "*********************************************************************"
az sig image-version create -g $gallery_rg --gallery-name $gallery --gallery-image-definition $definition --gallery-image-version $version --target-regions "eastus" --replica-count 1 --managed-image $image_id

if [[ $? -eq 0 ]]
then
    echo "Image publishing finished - /SharedGalleries/0a2c89a7-a44e-4cd0-b6ec-868432ad1d13-TESTGALLERYAGENT/Images/testgallery/Versions/latest"
    echo "Deleting staging resource group"
    az group delete -g $rg --yes --no-wait
    echo "Done"
fi
