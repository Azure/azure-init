# Azure-Init - Testinit Project

A containerized Azure provisioning agent that simulates Azure Instance Metadata Service (IMDS) and WireServer for testing and development environments.

> [!WARNING]
>
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
- The repository source and service file; the image builds both binaries

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
4. Wait for its systemd exit result

#### Selecting Images

The `start-all.sh` script also accepts an optional base image parameter for the Azure-init container.

```bash
./start-all.sh debian:12
```

Without a base container image argument, `start-all` defaults to Ubuntu 24.04.
The list of currently tested images can be found in the [e2e testing workflow](/.github/workflows/e2e-testing.yml).

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
- **Binaries**: `/usr/bin/azure-init` and `/usr/bin/libazureinit-kvp`
- **KVP Pool**: A private container tmpfs at `/var/lib/hyperv`, recreated for each run

### Testing Server

- **Container**: `azure-testing-server`
- **Endpoints**:
  - IMDS: `http://169.254.169.254/metadata/instance`
  - WireServer: `http://168.63.129.16`
- **Port**: 80 (mapped to host)
- **Readiness**: TCP connections to both configured Azure endpoint addresses.
  The health check sends no HTTP requests, preserving the scripted failures and
  delays for provisioning retry tests.

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

The Dockerfile builds the agent and KVP CLI from the workspace source and
installs the systemd service. Python is installed only for the test verifier.

### KVP Checks

CI invokes `verify_kvp.py` in a separate step after starting the testinit
environment. It checks all fourteen CLI commands,
including append/upsert behavior, batch input from files and stdin, parsed
diagnostics, report replacement, output modes, pool selection, safe/unsafe
limits, stale cleanup, and exit codes. All CLI checks use temporary directories
selected with `--dir`, never the agent's diagnostic pool. The verifier does not
launch `azure-init`, modify its configuration, or rerun provisioning.

To run the same CLI checks manually in an already-built container:

```bash
docker exec azureinit-provisioning-agent python3 /build/testinit/verify_kvp.py
```

CI collects the CLI output, agent logs, raw pool and parsed telemetry before
cleanup. These tests validate the local CLI and pool behavior, not Hyper-V or
Azure host retrieval. The privileged environment must still be run only on
disposable test hosts.
