FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive \
    container=docker

VOLUME [ "/sys/fs/cgroup" ]

# Install required packages
RUN apt-get update && apt-get install -y \
    systemd \
    dbus \
    curl \
    ca-certificates \
    util-linux \
    libpam-modules \
    libnss3 \
    libnss-systemd \
    passwd \
    adduser \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

# Disable systemd's unnecessary units for containers
RUN systemctl mask dev-hugepages.mount sys-fs-fuse-connections.mount \
    systemd-remount-fs.service \
    && systemctl set-default multi-user.target

# Create Hyper-V KVP directory and file for provisioning agent
RUN mkdir -p /var/lib/hyperv && \
    touch /var/lib/hyperv/.kvp_pool_1

# Copy your binary and service file
COPY target/debug/azure-init /usr/bin/azure-init
RUN chmod +x /usr/bin/azure-init

COPY config/azure-init.service /etc/systemd/system/azure-init.service
RUN systemctl enable azure-init.service

STOPSIGNAL SIGRTMIN+3

CMD ["/usr/lib/systemd/systemd"]
