ARG BASE_IMAGE=ubuntu:24.04
FROM ${BASE_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive \
    container=docker

VOLUME [ "/sys/fs/cgroup" ]

RUN if command -v apt-get >/dev/null 2>&1; then \
    # Debian/Ubuntu (apt-based) \
    apt-get -o Acquire::Retries=3 update \
    && apt-get -o Acquire::Retries=3 install -y \
    systemd \
    dbus \
    curl \
    ca-certificates \
    util-linux \
    openssh-server \
    python3 \
    build-essential \
    pkg-config \
    libudev-dev \
    && apt-get clean && rm -rf /var/lib/apt/lists/*; \
    elif command -v tdnf >/dev/null 2>&1; then \
    # Azure Linux (tdnf-based) \
    tdnf install -y \
    libseccomp \
    systemd \
    dbus \
    curl \
    ca-certificates \
    util-linux \
    openssh-server \
    python3 \
    gcc \
    gcc-c++ \
    glibc-devel \
    binutils \
    make \
    pkg-config \
    libudev-devel \
    && tdnf clean all; \
    else \
    echo "No supported package manager found" && exit 1; \
    fi

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Disable systemd's unnecessary units for containers
RUN systemctl mask dev-hugepages.mount sys-fs-fuse-connections.mount \
    systemd-remount-fs.service \
    && systemctl set-default multi-user.target

# Create Hyper-V KVP directory and file for provisioning agent
RUN mkdir -p /var/lib/hyperv && \
    touch /var/lib/hyperv/.kvp_pool_1

# Create the sudoers directory
RUN mkdir -p /etc/sudoers.d

# Create the SSHD directory
RUN mkdir -p /etc/ssh

# Build the agent and the standalone KVP CLI from the same source.
COPY . /build
WORKDIR /build
RUN cargo build --verbose -p azure-init --bin azure-init \
    -p libazureinit-kvp --bin libazureinit-kvp
RUN install -m 0755 target/debug/azure-init target/debug/libazureinit-kvp /usr/bin/

WORKDIR /

COPY config/azure-init.service /etc/systemd/system/azure-init.service
RUN systemctl enable azure-init.service

STOPSIGNAL SIGRTMIN+3

CMD ["/usr/lib/systemd/systemd"]
