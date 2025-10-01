FROM ubuntu:24.04

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

RUN chmod +x start_server_docker.sh 

EXPOSE 80

ENV PATH="/app/venv/bin:$PATH"

# Add potential delays/timeouts to the mocked servers here with env vars
# ENV WIRESERVER_GET_DELAY=5
# ENV IMDS_GET_DELAY=5
# ENV IMDS_GET_TIMEOUT=True
# ENV WIRESERVER_GET_TIMEOUT=True

CMD ["./start_server_docker.sh"]
