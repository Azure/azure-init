#!/usr/bin/env python3
"""
Testing server for provisioning agent that mocks Azure IMDS and WireServer endpoints.
This server creates dummy network interfaces and serves fake metadata responses.
"""

import subprocess
import json
import threading
import time
import signal
import sys
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs
import logging

# Configure logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Network configuration
IMDS_IP = "169.254.169.254"
WIRESERVER_IP = "168.63.129.16"
DUMMY_IFACE = "dummy0"
IMDS_PORT = 80
WIRESERVER_PORT = 80

# Mock metadata responses
MOCK_INSTANCE_METADATA = {
    "compute": {
        "azEnvironment": "AzurePublicCloud",
        "customData": "",
        "isHostCompatibilityLayerVm": "false",
        "licenseType": "",
        "location": "eastus",
        "name": "test-vm",
        "offer": "UbuntuServer",
        "osProfile": {
            "adminUsername": "userforazure",
            "computerName": "test-vm",
            "disablePasswordAuthentication": "true"
        },
        "osType": "Linux",
        "placementGroupId": "",
        "plan": {
            "name": "",
            "product": "",
            "publisher": ""
        },
        "platformFaultDomain": "0",
        "platformUpdateDomain": "0",
        "provider": "Microsoft.Compute",
        "publicKeys": [],
        "publisher": "Canonical",
        "resourceGroupName": "test-rg",
        "resourceId": "/subscriptions/12345678-90ab-cdef-1234-567890abcdef/resourceGroups/test-rg/providers/Microsoft.Compute/virtualMachines/test-vm",
        "securityProfile": {
            "secureBootEnabled": "false",
            "virtualTpmEnabled": "false"
        },
        "sku": "18.04-LTS",
        "storageProfile": {
            "dataDisks": [],
            "imageReference": {
                "offer": "UbuntuServer",
                "publisher": "Canonical",
                "sku": "18.04-LTS",
                "version": "latest"
            },
            "osDisk": {
                "caching": "ReadWrite",
                "createOption": "FromImage",
                "diskSizeGB": "30",
                "image": {
                    "uri": ""
                },
                "managedDisk": {
                    "id": "/subscriptions/12345678-90ab-cdef-1234-567890abcdef/resourceGroups/test-rg/providers/Microsoft.Compute/disks/test-vm_OsDisk_1",
                    "storageAccountType": "Premium_LRS"
                },
                "name": "test-vm_OsDisk_1",
                "osType": "Linux",
                "vhd": {
                    "uri": ""
                },
                "writeAcceleratorEnabled": "false"
            }
        },
        "subscriptionId": "12345678-90ab-cdef-1234-567890abcdef",
        "tags": "Environment:Test;Project:ProvisioningAgent",
        "version": "18.04.202109080",
        "vmId": "12345678-90ab-cdef-1234-567890abcdef",
        "vmScaleSetName": "",
        "vmSize": "Standard_B2s",
        "zone": "1"
    },
    "network": {
        "interface": [
            {
                "ipv4": {
                    "ipAddress": [
                        {
                            "privateIpAddress": "10.0.0.4",
                            "publicIpAddress": "52.168.1.1"
                        }
                    ],
                    "subnet": [
                        {
                            "address": "10.0.0.0",
                            "prefix": "24"
                        }
                    ]
                },
                "ipv6": {
                    "ipAddress": []
                },
                "macAddress": "000D3A123456"
            }
        ]
    }
}

MOCK_WIRESERVER_CONFIG = {
    "version": "2012-11-30",
    "goalStateIncarnation": "1",
    "machine": {
        "expectedState": "Started",
        "configurationStatus": "Ready"
    },
    "container": {
        "containerId": "12345678-90ab-cdef-1234-567890abcdef",
        "roleInstanceList": [
            {
                "instanceId": "test-vm",
                "state": "ReadyRole",
                "configurationName": "test-config"
            }
        ]
    }
}


def run_cmd(cmd):
    """Run a shell command and raise if it fails."""
    logger.info(f"Running command: {cmd}")
    try:
        result = subprocess.run(cmd, shell=True, check=True, capture_output=True, text=True)
        if result.stdout:
            logger.debug(f"Command output: {result.stdout}")
        return result
    except subprocess.CalledProcessError as e:
        logger.error(f"Command failed: {e}")
        logger.error(f"Error output: {e.stderr}")
        raise


class IMDSHandler(BaseHTTPRequestHandler):
    """HTTP handler for Azure Instance Metadata Service requests."""
    
    def log_message(self, format, *args):
        """Override to use our logger."""
        logger.info(f"IMDS: {format % args}")
    
    def do_GET(self):
        """Handle GET requests to IMDS endpoints."""
        parsed_url = urlparse(self.path)
        query_params = parse_qs(parsed_url.query)
        
        logger.info(f"IMDS GET request: {self.path}")
        logger.info(f"Headers: {dict(self.headers)}")
        
        # Check for required Metadata header
        if self.headers.get('Metadata') != 'true':
            self.send_response(400)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            error_response = {"error": "Bad Request", "message": "Metadata header not found"}
            self.wfile.write(json.dumps(error_response).encode())
            return
        
        # Handle different IMDS endpoints
        if parsed_url.path == '/metadata/instance':
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(MOCK_INSTANCE_METADATA).encode())
        
        elif parsed_url.path == '/metadata/identity/oauth2/token':
            # Mock managed identity token endpoint
            mock_token = {
                "access_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiIsIng1dCI6IjdkRC1nZWNOZ1gxWmY3R0xrT3ZwT0IyZGNWQSIsImtpZCI6IjdkRC1nZWNOZ1gxWmY3R0xrT3ZwT0IyZGNWQSJ9.eyJhdWQiOiJodHRwczovL21hbmFnZW1lbnQuYXp1cmUuY29tLyIsImlzcyI6Imh0dHBzOi8vc3RzLndpbmRvd3MubmV0LzEyMzQ1Njc4LTkwYWItY2RlZi0xMjM0LTU2Nzg5MGFiY2RlZi8iLCJpYXQiOjE2MjQ5NzQwMDAsIm5iZiI6MTYyNDk3NDAwMCwiZXhwIjoxNjI0OTc3NjAwLCJvaWQiOiIxMjM0NTY3OC05MGFiLWNkZWYtMTIzNC01Njc4OTBhYmNkZWYiLCJzdWIiOiIxMjM0NTY3OC05MGFiLWNkZWYtMTIzNC01Njc4OTBhYmNkZWYiLCJ0aWQiOiIxMjM0NTY3OC05MGFiLWNkZWYtMTIzNC01Njc4OTBhYmNkZWYifQ.fake_signature",
                "client_id": "12345678-90ab-cdef-1234-567890abcdef",
                "expires_in": "3600",
                "expires_on": "1624977600",
                "ext_expires_in": "3600",
                "not_before": "1624974000",
                "resource": "https://management.azure.com/",
                "token_type": "Bearer"
            }
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(mock_token).encode())
        
        else:
            self.send_response(404)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            error_response = {"error": "Not Found", "message": f"Endpoint {parsed_url.path} not found"}
            self.wfile.write(json.dumps(error_response).encode())


class WireServerHandler(BaseHTTPRequestHandler):
    """HTTP handler for Azure WireServer requests."""
    
    def log_message(self, format, *args):
        """Override to use our logger."""
        logger.info(f"WireServer: {format % args}")
    
    def do_GET(self):
        """Handle GET requests to WireServer endpoints."""
        logger.info(f"WireServer GET request: {self.path}")
        logger.info(f"Headers: {dict(self.headers)}")
        
        if self.path.startswith('/machine'):
            # Mock machine configuration endpoint
            self.send_response(200)
            self.send_header('Content-Type', 'application/xml')
            self.end_headers()
            xml_response = '''<?xml version="1.0" encoding="utf-8"?>
<GoalState xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="goalstate10.xsd">
  <Version>2012-11-30</Version>
  <Incarnation>1</Incarnation>
  <Machine>
    <ExpectedState>Started</ExpectedState>
    <StopRolesDeadlineHint>300000</StopRolesDeadlineHint>
    <LBProbePorts>
      <Port>16001</Port>
    </LBProbePorts>
  </Machine>
  <Container>
    <ContainerId>12345678-90ab-cdef-1234-567890abcdef</ContainerId>
    <RoleInstanceList>
      <RoleInstance>
        <InstanceId>test-vm</InstanceId>
        <State>ReadyRole</State>
        <Configuration>
          <HostingEnvironmentConfig>test-config</HostingEnvironmentConfig>
        </Configuration>
      </RoleInstance>
    </RoleInstanceList>
  </Container>
</GoalState>'''
            self.wfile.write(xml_response.encode())
        else:
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(MOCK_WIRESERVER_CONFIG).encode())
    
    def do_POST(self):
        """Handle POST requests to WireServer endpoints."""
        content_length = int(self.headers.get('Content-Length', 0))
        post_data = self.rfile.read(content_length)
        
        logger.info(f"WireServer POST request: {self.path}")
        logger.info(f"POST data: {post_data.decode('utf-8', errors='ignore')}")
        
        # Mock successful response for any POST
        self.send_response(200)
        self.send_header('Content-Type', 'application/xml')
        self.end_headers()
        response = '<?xml version="1.0" encoding="utf-8"?><Response>OK</Response>'
        self.wfile.write(response.encode())


class TestServer:
    """Main test server class that manages network setup and HTTP servers."""
    
    def __init__(self):
        self.imds_server = None
        self.wireserver_server = None
        self.imds_thread = None
        self.wireserver_thread = None
        self.running = False
    
    def setup_network_interface(self):
        """Set up dummy network interface with required IP addresses."""
        logger.info("Setting up dummy network interface...")
        
        try:
            # Create dummy interface
            run_cmd(f"sudo ip link add {DUMMY_IFACE} type dummy")
            run_cmd(f"sudo ip link set {DUMMY_IFACE} up")
            
            # Assign IP addresses
            run_cmd(f"sudo ip addr add {IMDS_IP}/32 dev {DUMMY_IFACE}")
            run_cmd(f"sudo ip addr add {WIRESERVER_IP}/32 dev {DUMMY_IFACE}")
            
            # Add routes to ensure traffic goes to our dummy interface
            run_cmd(f"sudo ip route add {IMDS_IP} dev {DUMMY_IFACE}")
            run_cmd(f"sudo ip route add {WIRESERVER_IP} dev {DUMMY_IFACE}")
            
            logger.info("Network interface setup completed successfully")
            
        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to set up network interface: {e}")
            raise
    
    def cleanup_network_interface(self):
        """Clean up dummy network interface."""
        logger.info("Cleaning up network interface...")
        
        try:
            # Remove routes first
            run_cmd(f"sudo ip route del {IMDS_IP} dev {DUMMY_IFACE} 2>/dev/null || true")
            run_cmd(f"sudo ip route del {WIRESERVER_IP} dev {DUMMY_IFACE} 2>/dev/null || true")
            
            # Remove the dummy interface
            run_cmd(f"sudo ip link delete {DUMMY_IFACE} 2>/dev/null || true")
            
            logger.info("Network interface cleanup completed")
            
        except subprocess.CalledProcessError as e:
            logger.warning(f"Error during cleanup (this might be expected): {e}")
    
    def start_imds_server(self):
        """Start the IMDS HTTP server."""
        logger.info(f"Starting IMDS server on {IMDS_IP}:{IMDS_PORT}")
        self.imds_server = HTTPServer((IMDS_IP, IMDS_PORT), IMDSHandler)
        self.imds_server.serve_forever()
    
    def start_wireserver_server(self):
        """Start the WireServer HTTP server."""
        logger.info(f"Starting WireServer on {WIRESERVER_IP}:{WIRESERVER_PORT}")
        self.wireserver_server = HTTPServer((WIRESERVER_IP, WIRESERVER_PORT), WireServerHandler)
        self.wireserver_server.serve_forever()
    
    def start(self):
        """Start the test server."""
        logger.info("Starting provisioning agent test server...")
        
        try:
            # Set up network interface
            self.setup_network_interface()
            
            # Start HTTP servers in separate threads
            self.imds_thread = threading.Thread(target=self.start_imds_server, daemon=True)
            self.wireserver_thread = threading.Thread(target=self.start_wireserver_server, daemon=True)
            
            self.imds_thread.start()
            self.wireserver_thread.start()
            
            self.running = True
            logger.info("Test server started successfully!")
            logger.info(f"IMDS endpoint: http://{IMDS_IP}:{IMDS_PORT}/metadata/instance")
            logger.info(f"WireServer endpoint: http://{WIRESERVER_IP}:{WIRESERVER_PORT}/machine")
            
            # Keep the main thread alive
            try:
                while self.running:
                    time.sleep(1)
            except KeyboardInterrupt:
                logger.info("Received interrupt signal")
                self.stop()
        
        except Exception as e:
            logger.error(f"Failed to start test server: {e}")
            self.cleanup_network_interface()
            raise
    
    def stop(self):
        """Stop the test server and clean up."""
        logger.info("Stopping test server...")
        self.running = False
        
        if self.imds_server:
            self.imds_server.shutdown()
        if self.wireserver_server:
            self.wireserver_server.shutdown()
        
        self.cleanup_network_interface()
        logger.info("Test server stopped")


def signal_handler(sig, frame):
    """Handle SIGINT (Ctrl+C) gracefully."""
    logger.info("Received SIGINT, shutting down...")
    sys.exit(0)


if __name__ == "__main__":
    # Set up signal handler for graceful shutdown
    signal.signal(signal.SIGINT, signal_handler)
    
    # Create and start the test server
    server = TestServer()
    try:
        server.start()
    except KeyboardInterrupt:
        pass
    finally:
        server.stop()
