"""Check mock listeners without consuming scripted HTTP responses."""

import socket

from config import IMDS_IP, IMDS_PORT, WIRESERVER_IP, WIRESERVER_PORT


def check_readiness():
    for address in ((IMDS_IP, IMDS_PORT), (WIRESERVER_IP, WIRESERVER_PORT)):
        with socket.create_connection(address, timeout=2):
            pass


if __name__ == "__main__":
    check_readiness()
