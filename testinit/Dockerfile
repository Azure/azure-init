ARG BASE_IMAGE=ubuntu:24.04
FROM ${BASE_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive \
    container=docker

VOLUME [ "/sys/fs/cgroup" ]

# Conditional package installation for apt-based, rpm-based, and Azure Linux systems
RUN if command -v apt-get >/dev/null 2>&1; then \
    # Debian/Ubuntu (apt-based) \
    apt-get update && apt-get install -y \
    systemd \
    dbus \
    curl \
    ca-certificates \
    util-linux \
    openssh-server \
    build-essential \
    pkg-config \
    libudev-dev \
    && apt-get clean && rm -rf /var/lib/apt/lists/*; \
    elif command -v tdnf >/dev/null 2>&1; then \
    # Azure Linux (tdnf-based) \
    tdnf install -y \
    systemd \
    dbus \
    curl \
    ca-certificates \
    util-linux \
    openssh-server \
    gcc \
    gcc-c++ \
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

# Copy source code and build azure-init
COPY . /build
WORKDIR /build
RUN cargo build --verbose
RUN cp target/debug/azure-init /usr/bin/azure-init && chmod +x /usr/bin/azure-init

WORKDIR /

COPY config/azure-init.service /etc/systemd/system/azure-init.service
RUN systemctl enable azure-init.service

STOPSIGNAL SIGRTMIN+3

CMD ["/usr/lib/systemd/systemd"]
