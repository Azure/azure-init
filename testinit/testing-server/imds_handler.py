from http.server import BaseHTTPRequestHandler
import json
from urllib.parse import urlparse, parse_qs

from config import IMDS_GET_DELAY, IMDS_GET_TIMEOUT
from utils import logger


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
        
        if IMDS_GET_TIMEOUT:
            logger.info(f"Adding IDMS GET timeout from ENV variable")
            return

        if IMDS_GET_DELAY != 0:
            logger.info(f"Adding IMDS GET request delay of {IMDS_GET_DELAY} seconds")
            time.sleep(IMDS_GET_DELAY)

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
        
        else:
            self.send_response(404)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            error_response = {"error": "Not Found", "message": f"Endpoint {parsed_url.path} not found"}
            self.wfile.write(json.dumps(error_response).encode())