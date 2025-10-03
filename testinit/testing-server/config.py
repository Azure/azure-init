import os

# Network configuration
IMDS_IP = "169.254.169.254"
WIRESERVER_IP = "168.63.129.16"
DUMMY_IFACE = "dummy0"
IMDS_PORT = 80
WIRESERVER_PORT = 80

# Server configuration
IMDS_GET_DELAY = int(os.getenv("IMDS_GET_DELAY", "0"))  # In seconds
WIRESERVER_GET_DELAY = int(os.getenv("WIRESERVER_GET_DELAY", "0"))  # In seconds
IMDS_GET_TIMEOUT = True if os.getenv("IMDS_GET_TIMEOUT", "False").lower() == "true" else False
WIRESERVER_GET_TIMEOUT = True if os.getenv("WIRESERVER_GET_TIMEOUT", "False").lower() == "true" else False
