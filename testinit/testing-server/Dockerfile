FROM ubuntu:24.04

RUN apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 \
    -o APT::Update::Error-Mode=any update \
    && apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 install -y \
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

WORKDIR /app

COPY . /app/

RUN chmod +x start_server_docker.sh 

EXPOSE 80

ENV PATH="/app/venv/bin:$PATH"

CMD ["./start_server_docker.sh"]
