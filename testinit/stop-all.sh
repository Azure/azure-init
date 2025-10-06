#!/bin/bash

echo "Stopping provisioning-agent..."
docker compose down

echo "Stopping testing-server and cleaning up networks..."
pushd testing-server
docker compose down
popd

echo "Removing any orphaned containers..."
docker container prune -f

echo "Removing Docker networks..."
docker network rm imds-network wireserver-network 2>/dev/null || true

echo "All services stopped and networks cleaned up!"
echo ""
echo "Network status:"
docker network ls | grep -E "(imds|wireserver|agent)" || echo "No related networks found (good!)"
sleep 2

echo "All services stopped!"
