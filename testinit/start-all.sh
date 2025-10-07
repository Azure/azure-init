#!/bin/bash

echo "Starting testing-server first (creates networks with Azure IP addresses)..."
pushd testing-server
docker compose up -d --build

popd

echo "Starting provisioning-agent (connects to existing networks)..."
docker compose up -d --build

while true; do
  status=$(docker exec provisioning-agent systemctl is-active azure-init.service)
  echo "azure-init.service status: $status"
  if [[ "$status" == "inactive" || "$status" == "failed" ]]; then
    echo "azure-init.service has completed"
    break
  fi
  sleep 10
done

echo "Testing-server is available at the Azure service endpoints:"
echo "  IMDS: http://169.254.169.254/metadata/instance"
echo "  WireServer: http://168.63.129.16/machine"
echo ""
echo "To view logs:"
echo "  docker compose logs -f provisioning-agent"
echo "  cd testing-server && docker compose logs -f testing-server"
echo ""
echo "To stop all services (or if there were errors):"
echo "  ./stop-all.sh"
