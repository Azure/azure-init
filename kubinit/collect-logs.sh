#!/bin/bash

# Local log collection script that mimics GitHub Actions workflow
# Usage: ./collect-logs.sh [duration_in_seconds]

set -e

# Configuration
DURATION=${1:-120}  # Default 2 minutes
LOG_DIR="./local-logs-$(date +%Y%m%d-%H%M%S)"
COLORS=true

# Colors for output
if [ "$COLORS" = true ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[1;33m'
    BLUE='\033[0;34m'
    PURPLE='\033[0;35m'
    CYAN='\033[0;36m'
    NC='\033[0m' # No Color
else
    RED=''
    GREEN=''
    YELLOW=''
    BLUE=''
    PURPLE=''
    CYAN=''
    NC=''
fi

print_status() {
    echo -e "${GREEN}[$(date '+%H:%M:%S')]${NC} $1"
}

print_info() {
    echo -e "${BLUE}[$(date '+%H:%M:%S')]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[$(date '+%H:%M:%S')]${NC} $1"
}

print_error() {
    echo -e "${RED}[$(date '+%H:%M:%S')]${NC} $1"
}

print_header() {
    echo -e "${CYAN}$1${NC}"
}

# Cleanup function
cleanup() {
    print_warning "Cleaning up..."
    
    # Kill background processes
    jobs -p | xargs -r kill 2>/dev/null || true
    
    # Stop services
    ./stop-all.sh 2>/dev/null || true
    
    print_status "Cleanup completed"
}

# Set up signal handlers
trap cleanup SIGINT SIGTERM

# Create log directory
mkdir -p "$LOG_DIR"

print_header "╔════════════════════════════════════════════════════════════════╗"
print_header "║              Azure Init Local Log Collection                   ║"
print_header "║                                                                ║"
print_header "║  This script collects logs similar to GitHub Actions CI       ║"
print_header "║  Duration: ${DURATION} seconds                                        ║"
print_header "║  Log directory: $LOG_DIR                            ║"
print_header "╚════════════════════════════════════════════════════════════════╝"

print_info "Starting log collection process..."

# Make scripts executable
chmod +x ./start-all.sh ./stop-all.sh ./testing-server/start_server_docker.sh

# Start services
print_status "Starting Azure Init services..."
./start-all.sh > "$LOG_DIR/startup.log" 2>&1 &
START_PID=$!

# Wait for initial startup
print_info "Waiting for services to start..."
sleep 30

# Check if containers are running
print_info "Checking container status..."
docker ps -a > "$LOG_DIR/container-status.log"
cat "$LOG_DIR/container-status.log"

# Start continuous log collection
print_status "Starting continuous log collection for $DURATION seconds..."

{
    echo "=== Azure Init Provisioning Agent Logs ==="
    docker compose logs -f provisioning-agent 2>&1 &
    AGENT_PID=$!
    
    echo "=== Testing Server Logs ==="
    (cd testing-server && docker compose logs -f testing-server 2>&1) &
    SERVER_PID=$!
    
    # Let it run for specified duration
    sleep "$DURATION"
    
    # Stop log collection
    kill $AGENT_PID $SERVER_PID 2>/dev/null || true
    
} | tee "$LOG_DIR/live-logs.log"

print_status "Collecting detailed logs..."

# Collect Azure Init specific logs
print_info "Getting Azure Init service status..."
{
    echo "=== Azure Init Service Status ==="
    docker exec azureinit-provisioning-agent systemctl status azure-init.service 2>&1 || echo "Service not accessible"
    
    echo ""
    echo "=== Azure Init Journal (last 200 lines) ==="
    docker exec azureinit-provisioning-agent journalctl -u azure-init.service --no-pager -n 200 2>&1 || echo "Journal not accessible"
    
    echo ""
    echo "=== All Journal Logs (last 100 lines) ==="
    docker exec azureinit-provisioning-agent journalctl --no-pager -n 100 2>&1 || echo "Full journal not accessible"
    
} > "$LOG_DIR/azure-init-detailed.log"

# Collect system information
print_info "Getting system information..."
{
    echo "=== Container Process List ==="
    docker exec azureinit-provisioning-agent ps aux 2>&1 || echo "Process list not accessible"
    
    echo ""
    echo "=== Systemd Units ==="
    docker exec azureinit-provisioning-agent systemctl list-units --all 2>&1 || echo "Systemd units not accessible"
    
} > "$LOG_DIR/system-info.log"

# Collect network information
print_info "Getting network information..."
{
    echo "=== Network Configuration ==="
    docker exec azureinit-provisioning-agent ip addr show 2>&1 || echo "Network info not accessible"
    
    echo ""
    echo "=== Routing Table ==="
    docker exec azureinit-provisioning-agent ip route show 2>&1 || echo "Route info not accessible"
    
    echo ""
    echo "=== Docker Networks ==="
    docker network ls
    
    echo ""
    echo "=== Network Inspection ==="
    docker network inspect imds-network wireserver-network 2>&1 || echo "Network inspection failed"
    
} > "$LOG_DIR/network-info.log"

# Collect container details
print_info "Getting container details..."
{
    echo "=== Provisioning Agent Container Details ==="
    docker inspect azureinit-provisioning-agent 2>&1 || echo "Container inspection failed"
    
    echo ""
    echo "=== Testing Server Container Details ==="
    docker inspect azure-testing-server 2>&1 || echo "Container inspection failed"
    
} > "$LOG_DIR/container-details.log"

# Test endpoints
print_info "Testing service endpoints..."
{
    echo "=== IMDS Endpoint Test ==="
    curl -v -H "Metadata: true" "http://169.254.169.254/metadata/instance?api-version=2021-02-01" 2>&1 || echo "IMDS test failed"
    
    echo ""
    echo "=== WireServer Endpoint Test ==="
    curl -v "http://168.63.129.16/machine" 2>&1 || echo "WireServer test failed"
    
} > "$LOG_DIR/endpoint-tests.log"

# Collect environment information
print_info "Getting environment information..."
{
    echo "=== Container Environment ==="
    docker exec azureinit-provisioning-agent env 2>&1 || echo "Environment not accessible"
    
} > "$LOG_DIR/environment.log"

# Create summary
print_status "Creating summary..."
{
    echo "Azure Init Log Collection Summary"
    echo "================================="
    echo "Collection Date: $(date)"
    echo "Duration: $DURATION seconds"
    echo "Log Directory: $LOG_DIR"
    echo ""
    echo "Files Collected:"
    ls -la "$LOG_DIR/"
    echo ""
    echo "Log Sizes:"
    du -sh "$LOG_DIR"/*
    echo ""
    echo "Container Status at Collection Time:"
    docker ps -a
    echo ""
    echo "Last 10 lines of Azure Init Journal:"
    docker exec azureinit-provisioning-agent journalctl -u azure-init.service --no-pager -n 10 2>/dev/null || echo "No logs available"
    
} > "$LOG_DIR/collection-summary.log"

# Display summary
print_header ""
print_header "📊 Log Collection Summary"
print_header "=========================="
cat "$LOG_DIR/collection-summary.log"

print_header ""
print_status "✅ Log collection completed successfully!"
print_info "📁 Logs saved to: $LOG_DIR"
print_info "📋 To view logs:"
print_info "   - Main logs: cat $LOG_DIR/live-logs.log"
print_info "   - Azure Init details: cat $LOG_DIR/azure-init-detailed.log"
print_info "   - System info: cat $LOG_DIR/system-info.log"
print_info "   - Network info: cat $LOG_DIR/network-info.log"
print_info "   - Endpoint tests: cat $LOG_DIR/endpoint-tests.log"

# Create an archive for easy sharing
print_info "Creating archive..."
tar -czf "${LOG_DIR}.tar.gz" "$LOG_DIR/"
print_status "📦 Archive created: ${LOG_DIR}.tar.gz"

# Cleanup
cleanup

print_header ""
print_status "🎉 All done! Check the logs in $LOG_DIR or the archive ${LOG_DIR}.tar.gz"
