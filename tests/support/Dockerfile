ARG BASE_IMAGE=ubuntu:24.04
FROM ${BASE_IMAGE}

RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 libudev1 \
    && rm -rf /var/lib/apt/lists \
    && mkdir -p /test /work /var/lib/hyperv /etc/sudoers.d \
    && rm -f /etc/mtab \
    && touch /etc/mtab /test/isolated-provisioning

COPY container_provisioning.py /test/container_provisioning.py

ENTRYPOINT ["python3", "/test/container_provisioning.py", "--inside"]
