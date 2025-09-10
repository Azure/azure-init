FROM ubuntu:24.04

# Install required packages
RUN apt-get update && apt-get install -y \
    python3 \
    python3-pip \
    python3-venv \
    iproute2 \
    curl \
    jq \
    sudo \
    kmod \
    && rm -rf /var/lib/apt/lists/*

# Create and activate virtual environment, then install Python dependencies
COPY requirements.txt /app/requirements.txt
RUN python3 -m venv /app/venv && \
    /app/venv/bin/pip install --upgrade pip && \
    /app/venv/bin/pip install -r /app/requirements.txt

# Create app directory
WORKDIR /app

# Copy all application files
COPY . /app/

# Make scripts executable
RUN chmod +x start_server_docker.sh 

# Load dummy kernel module
RUN modprobe dummy 2>/dev/null || echo "Dummy module not available - will be handled by script"

# Expose the ports used by IMDS and WireServer
EXPOSE 80

# Set environment variable to indicate we're running in Docker
ENV RUNNING_IN_DOCKER=true

# Add virtual environment to PATH
ENV PATH="/app/venv/bin:$PATH"

# Use the Docker-optimized script as the entrypoint
CMD ["./start_server_docker.sh"]
