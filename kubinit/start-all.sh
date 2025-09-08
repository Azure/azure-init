#!/bin/bash

echo "Starting testing-server first (creates networks with Azure IP addresses)..."
cd testing-server
docker compose up -d --build
echo "cade, ls"
ls
cd ..

echo "cade, ls 2"
ls

echo "Waiting for testing-server to be ready..."
sleep 10

echo "Starting provisioning-agent (connects to existing networks)..."
docker compose up -d --build

echo "Both services should now be running (unless you see bright red errors)"
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
