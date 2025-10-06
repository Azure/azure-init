# Azure-Init - Testinit Project

A containerized Azure provisioning agent that simulates Azure Instance Metadata Service (IMDS) and WireServer for testing and development environments.

> **Warning**
> Running the ./start-all.sh script will modify your local machine due to how systemd in Docker works!
> Whenever possible, do not run this system on your personal machine as it completes the full provisioning run found in azure-init.
> This may affect your local hostname, ssh keys, users, or more.
> Exercise caution running this system locally!

## Overview

This project consists of two main components:
- **Provisioning Agent**: A systemd-based service running `azure-init` binary in a container
- **Testing Server**: A mock Azure service providing IMDS and WireServer endpoints

## Architecture

The setup creates two Docker networks with Azure-like IP addresses:
- `imds-network` (169.254.0.0/16) - For IMDS communication
- `wireserver-network` (168.63.0.0/16) - For WireServer communication

## Prerequisites

- Docker and Docker Compose
- WSL2 or Linux environment
- The `azure-init` binary and service file

## Quick Start

### Starting Services

Run the start script to launch both services:

```bash
./start-all.sh
```

This will:
1. Start the testing server container first (creates networks with Azure IP addresses)
2. Wait for the testing server to be ready
3. Start the provisioning agent

### Stopping Services

Stop all services and clean up:

```bash
./stop-all.sh
```

This will:
1. Stop the provisioning agent container
2. Stop the testing server container
3. Remove orphaned containers
4. Clean up Docker networks

## Service Details

### Provisioning Agent

- **Container**: `azureinit-provisioning-agent`
- **Image**: Built from local Dockerfile
- **Service**: systemd-based `azure-init.service`
- **Privileges**: Runs with `privileged: true` for systemd support
- **Binary Location**: In the container, located at `/usr/local/bin/azure-init`

### Testing Server

- **Container**: `azure-testing-server`
- **Endpoints**:
  - IMDS: `http://169.254.169.254/metadata/instance`
  - WireServer: `http://168.63.129.16/machine`
- **Port**: 80 (mapped to host)

## Monitoring and Debugging

### View Logs

**Provisioning Agent:**
```bash
docker compose logs -f provisioning-agent
```

**Testing Server:**
```bash
cd testing-server
docker compose logs -f testing-server
```

## Development

### Building

The Dockerfile expects:
- `azure-init/target/debug/azure-init` - The main binary (compiled via `cargo build`)
- `azure-init/config/azure-init.service` - The systemd service file
