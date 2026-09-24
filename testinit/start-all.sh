#!/bin/bash

set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

# Accept base image as first argument, default to ubuntu:24.04 if not provided
BASE_IMAGE=${1:-ubuntu:24.04}

echo "Using base image: $BASE_IMAGE"
echo "Starting testing-server first (creates networks with Azure IP addresses)..."
pushd testing-server
docker compose up -d --build --wait --wait-timeout 180

popd

echo "Starting azureinit-provisioning-agent (connects to existing networks)..."
BASE_IMAGE="$BASE_IMAGE" docker compose up -d --build --force-recreate

DEADLINE=$((SECONDS + 1200))

while true; do
  if PROPERTIES=$(docker exec azureinit-provisioning-agent systemctl show \
      azure-init.service -p ActiveState -p Result -p ExecMainStatus \
      -p ExecMainStartTimestampMonotonic 2>/dev/null); then
    STATE= RESULT= STATUS= STARTED=0
    while IFS='=' read -r PROPERTY VALUE; do
      case "$PROPERTY" in
        ActiveState) STATE=$VALUE ;;
        Result) RESULT=$VALUE ;;
        ExecMainStatus) STATUS=$VALUE ;;
        ExecMainStartTimestampMonotonic) STARTED=$VALUE ;;
      esac
    done <<< "$PROPERTIES"

    if [[ "$STATE" == failed ]] || [[ "$STATE" == inactive && "$STARTED" != 0 ]]; then
      if [[ "$RESULT" != success || "$STATUS" != 0 ]]; then
        docker exec azureinit-provisioning-agent journalctl -u azure-init.service --no-pager
        echo "ERROR: azure-init.service ended with result=$RESULT status=$STATUS"
        exit 1
      fi
      echo "azure-init.service has finished successfully"
      break
    fi
  fi
  if [[ "$SECONDS" -ge "$DEADLINE" ]]; then
    echo "ERROR: Timed out after 20 minutes waiting for azure-init.service to complete."
    exit 1
  fi
  sleep 1
done

echo "Testing-server is available at the Azure service endpoints:"
echo "  IMDS: http://169.254.169.254/metadata/instance"
echo "  WireServer: http://168.63.129.16"
echo ""
echo "To view logs:"
echo "  docker compose logs -f provisioning-agent"
echo "  cd testing-server && docker compose logs -f testing-server"
echo ""
echo "To stop all services (or if there were errors):"
echo "  ./stop-all.sh"
