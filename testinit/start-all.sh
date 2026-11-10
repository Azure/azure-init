#!/bin/bash

set -e

# Accept base image as first argument, default to ubuntu:24.04 if not provided
BASE_IMAGE=${1:-ubuntu:24.04}

if [[ $# -gt 2 || ( $# -eq 2 && $2 != --no-build ) ]]; then
  echo "Usage: $0 [base-image] [--no-build]" >&2
  exit 1
fi

BUILD_ARGS=(--build)
if [[ ${2:-} == --no-build ]]; then
  BUILD_ARGS=(--no-build --pull never)
fi

mkdir -p /tmp/testinit

echo "Using base image: $BASE_IMAGE"
echo "Starting testing-server first (creates networks with Azure IP addresses)..."
pushd testing-server
docker compose up -d "${BUILD_ARGS[@]}"

popd

echo "Starting azureinit-provisioning-agent (connects to existing networks)..."
BASE_IMAGE=$BASE_IMAGE docker compose up -d "${BUILD_ARGS[@]}"

TIMEOUT=1200  # 20 minutes in seconds
ELAPSED=0

while true; do
  # Check journal logs for either a success or a failure to ensure the completion of the service
  SERVICE_LOGS=$(docker exec azureinit-provisioning-agent journalctl -u azure-init.service --no-pager)
  if grep -q "Finished azure-init.service" <<< "$SERVICE_LOGS"; then
    echo "azure-init.service has finished"
    break
  fi
  if grep -i 'azure-init' <<< "$SERVICE_LOGS" | grep -Eiq 'failed|failure'; then
    echo "azure-init.service has failed." >&2
    exit 1
  fi
  if [ "$ELAPSED" -ge "$TIMEOUT" ]; then
    echo "ERROR: Timed out after 20 minutes waiting for azure-init.service to complete."
    exit 1
  fi
  echo "Waiting for azure-init.service to finish... (${ELAPSED}s/${TIMEOUT}s)"
  sleep 10
  ELAPSED=$((ELAPSED + 10))
done

echo "Testing-server is available at the Azure service endpoints:"
echo "  IMDS: http://169.254.169.254/metadata/instance"
echo "  WireServer: http://168.63.129.16"
echo ""
echo "To view logs:"
echo "  docker compose logs -f azureinit-provisioning-agent"
echo "  cd testing-server && docker compose logs -f testing-server"
echo ""
echo "To stop all services (or if there were errors):"
echo "  ./stop-all.sh"
