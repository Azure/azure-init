#!/bin/bash

echo "Starting testing-server first (creates networks with Azure IP addresses)..."
pushd testing-server
docker compose up -d --build

popd

echo "Starting azureinit-provisioning-agent (connects to existing networks)..."
docker compose up -d --build

while true; do
  if docker exec azureinit-provisioning-agent journalctl -u azure-init.service --no-pager | grep -q "Finished azure-init.service"; then
    echo "azure-init.service has finished"
    break
  fi
  if  docker exec azureinit-provisioning-agent journalctl -u azure-init.service --no-pager | grep -i 'azure-init' | grep -Eiq 'failed|failure'; then
    echo "azure-init.service has failed."
    break
  fi
  echo "Waiting for azure-init.service to finish..."
  sleep 10
done

echo "Testing-server is available at the Azure service endpoints:"
echo "  IMDS: http://169.254.169.254/metadata/instance"
echo "  WireServer: http://168.63.129.16/machine"
echo ""
echo "To view logs:"
echo "  docker compose logs -f azureinit-provisioning-agent"
echo "  cd testing-server && docker compose logs -f testing-server"
echo ""
echo "To stop all services (or if there were errors):"
echo "  ./stop-all.sh"
