#!/bin/bash

# Docker-optimized script to start the provisioning agent test server
# This version is specifically designed to run in a Docker container

set -e

# Colors for better output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Function to print colored output
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

# Check if running in Docker
if [[ "$RUNNING_IN_DOCKER" == "true" ]]; then
    print_info "Running in Docker container mode"
    USE_SUDO=""
else
    print_info "Running in host mode"
    USE_SUDO="sudo"
fi

# Check for required permissions
check_permissions() {
    if [[ $EUID -eq 0 ]]; then
        print_status "Running as root"
    elif [[ "$RUNNING_IN_DOCKER" == "true" ]]; then
        print_status "Running in Docker with appropriate privileges"
    else
        print_error "This script requires root privileges"
        echo "Please run with: sudo $0"
        exit 1
    fi
}

# Check if dummy module is available
check_dummy_support() {
    print_info "Checking dummy interface support..."
    
    if ${USE_SUDO} ip link add test_check_dummy type dummy 2>/dev/null; then
        ${USE_SUDO} ip link delete test_check_dummy 2>/dev/null || true
        print_status "Dummy interface support confirmed"
    else
        print_warning "Dummy interface creation failed. Attempting to load dummy module..."
        ${USE_SUDO} modprobe dummy 2>/dev/null || {
            print_error "Failed to load dummy kernel module"
            if [[ "$RUNNING_IN_DOCKER" == "true" ]]; then
                print_warning "Note: Some Docker environments don't support kernel modules"
                print_warning "The server will attempt to continue but may fail when creating interfaces"
            else
                print_error "Please ensure the dummy module is available"
                exit 1
            fi
        }
        print_status "Dummy module loaded"
    fi
}

# Check if Python dependencies are installed
check_dependencies() {
    print_info "Checking Python dependencies..."
    
    # Check each dependency individually for better error reporting
    local missing_deps=()
    
    python3 -c "import requests" 2>/dev/null || missing_deps+=("requests")
    python3 -c "import subprocess" 2>/dev/null || missing_deps+=("subprocess")
    python3 -c "import json" 2>/dev/null || missing_deps+=("json")
    python3 -c "import threading" 2>/dev/null || missing_deps+=("threading")
    python3 -c "import time" 2>/dev/null || missing_deps+=("time")
    python3 -c "import signal" 2>/dev/null || missing_deps+=("signal")
    python3 -c "import sys" 2>/dev/null || missing_deps+=("sys")
    
    if [ ${#missing_deps[@]} -ne 0 ]; then
        print_error "Missing Python dependencies: ${missing_deps[*]}"
        if [[ "$RUNNING_IN_DOCKER" == "true" ]]; then
            print_info "Attempting to install missing dependencies..."
            pip3 install requests || {
                print_error "Failed to install dependencies in Docker container"
                print_error "This suggests an issue with the Docker build process"
                exit 1
            }
            print_status "Dependencies installed successfully"
        else
            print_error "Please install missing dependencies with: pip3 install -r requirements.txt"
            exit 1
        fi
    else
        print_status "Python dependencies available"
    fi
}

# Check if test_server.py exists
check_server_file() {
    if [[ ! -f "test_server.py" ]]; then
        print_error "test_server.py not found in current directory"
        exit 1
    fi
    print_status "test_server.py found"
}

# Cleanup function
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
    ${USE_SUDO} ip route del 169.254.169.254 dev dummy0 2>/dev/null || true
    ${USE_SUDO} ip route del 168.63.129.16 dev dummy0 2>/dev/null || true
    ${USE_SUDO} ip link delete dummy0 2>/dev/null || true
    
    print_status "Cleanup completed"
    exit 0
}

# Set up signal handlers
trap cleanup SIGINT SIGTERM

# Print banner
echo -e "${CYAN}"
cat << "EOF"
╔══════════════════════════════════════════════════════════════╗
║                    Azure-Init Test Server                    ║
║                        (Docker Mode)                         ║
║                                                              ║
║  This server mocks Azure IMDS and WireServer endpoints       ║
║  Press Ctrl+C to stop the server and clean up                ║
╚══════════════════════════════════════════════════════════════╝
EOF
echo -e "${NC}"

# Run pre-flight checks
print_info "Running pre-flight checks..."
check_permissions
check_server_file
check_dummy_support
check_dependencies

echo ""
print_status "Starting Azure provisioning agent test server..."
print_info "IMDS endpoint: http://169.254.169.254/metadata/instance"
print_info "WireServer endpoint: http://168.63.129.16/machine"
print_info "All requests will be logged below"
print_info "Press Ctrl+C to stop the server"
echo ""

# Start the server and capture its PID
python3 test_server.py &
SERVER_PID=$!

# Give the server a moment to start
sleep 2

# Check if server is still running
if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    print_error "✗ Server failed to start"
    exit 1
fi

print_status "Test server started successfully (PID: $SERVER_PID)"
print_info "Server is ready to receive requests..."
print_info "You can test the endpoints from outside the container:"
print_info "   curl -H 'Metadata: true' 'http://169.254.169.254/metadata/instance?api-version=2021-02-01'"
print_info "   curl 'http://168.63.129.16/machine'"

# Wait for the server process to finish
wait $SERVER_PID 2>/dev/null

# If we get here, the server exited normally
print_status "Server shut down normally"
cleanup
