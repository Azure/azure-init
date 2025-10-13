#!/bin/bash

set -e

# Colors for better output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

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

check_permissions() {
    if [[ $EUID -eq 0 ]]; then
        print_status "Running as root"
    else
        print_error "This script requires root privileges"
        echo "Please run with: sudo $0"
        exit 1
    fi
}

check_dependencies() {
    print_info "Checking Python dependencies..."
    
    local missing_deps=()
    
    python3 -c "import requests" 2>/dev/null || missing_deps+=("requests")
    
    if [ ${#missing_deps[@]} -ne 0 ]; then
        print_error "Missing Python dependencies: ${missing_deps[*]}"
    else
        print_status "Python dependencies available"
    fi
}

check_server_file() {
    if [[ ! -f "test_server.py" ]]; then
        print_error "test_server.py not found in current directory"
        exit 1
    fi
    print_status "test_server.py found"
}

cleanup() {
    print_warning "\nShutdown signal received. Cleaning up..."
    
    # Kill the Python server process if it's running
    if [[ -n "$SERVER_PID" ]]; then
        print_info "Stopping test server (PID: $SERVER_PID)..."
        kill -TERM "$SERVER_PID" 2>/dev/null || kill -KILL "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    
    # Clean up network interfaces manually
    print_info "Cleaning up network interfaces..."
    ip route del 169.254.169.254 dev dummy0 2>/dev/null || true
    ip route del 168.63.129.16 dev dummy0 2>/dev/null || true
    ip link delete dummy0 2>/dev/null || true
    
    print_status "Cleanup completed"
    exit 0
}

trap cleanup SIGINT SIGTERM

echo -e "${CYAN}"
cat << "EOF"
╔══════════════════════════════════════════════════════════════╗
║                    Azure-Init Test Server                    ║
║                                                              ║
║  This server mocks Azure IMDS and WireServer endpoints       ║
║  Press Ctrl+C to stop the server and clean up                ║
╚══════════════════════════════════════════════════════════════╝
EOF
echo -e "${NC}"

check_permissions
check_server_file
check_dependencies

echo ""
print_status "Starting Azure provisioning agent test server..."
print_info "IMDS endpoint: http://169.254.169.254/metadata/instance"
print_info "WireServer endpoint: http://168.63.129.16"
print_info "All requests will be logged below"
print_info "Press Ctrl+C to stop the server"
echo ""

# Start the server and capture its PID
python3 test_server.py --imds-responses ./api-responses/imds/one-bad-then-success.json --wireserver-responses ./api-responses/wireserver/two-bad-then-success.xml &
SERVER_PID=$!

# Give the server a moment to start
sleep 2

# Check if server is still running
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    print_error "Server failed to start"
    exit 1
fi

print_status "Test server started successfully (PID: $SERVER_PID)"
print_info "Server is ready to receive requests..."
print_info "You can test the endpoints from outside the container:"
print_info "   curl -H 'Metadata: true' 'http://169.254.169.254/metadata/instance?api-version=2021-02-01'"
print_info "   curl 'http://168.63.129.16'"

# Wait for the server process to finish
wait $SERVER_PID 2>/dev/null

# If we get here, the server exited normally
print_status "Server shut down normally"
cleanup
