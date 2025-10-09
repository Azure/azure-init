from http.server import BaseHTTPRequestHandler
import json
import time
from urllib.parse import urlparse, parse_qs, ParseResult

from utils import logger


class IMDSHandler(BaseHTTPRequestHandler):
    """HTTP handler for Azure Instance Metadata Service requests."""
    
    _responses_file_path = None 
    _responses = None
    _response_position = 0

    @classmethod
    def set_response_file_path(cls, file_path: str):
        cls._responses_file_path = file_path

    @classmethod
    def load_responses(cls):
        with open(cls._responses_file_path, 'r') as f:
            cls._responses = json.load(f)

        cls._responses = cls._responses['responses']

        logger.info(json.dumps(cls._responses, indent=2))

    def write_custom_response(self):
        responses_list = self._responses
            
        if self.__class__._response_position >= len(responses_list):
            self.__class__._response_position = 0
        
        current_response = responses_list[self.__class__._response_position]
        
        delay = current_response.get("delay")
        if delay is not None:
            logger.info(f"Adding custom delay of {delay} seconds")
            time.sleep(delay)

        self.send_response(current_response['status_code'])
        
        headers = current_response.get('headers', {})
        for header_name, header_value in headers.items():
            self.send_header(header_name, header_value)
        self.end_headers()
        
        response_body = current_response.get('response', {})
        self.wfile.write(json.dumps(response_body).encode())

        logger.info(f"Returning response: {current_response}, from position: {self.__class__._response_position}")
        self.__class__._response_position += 1
        return
    
    def write_default_response(self, parsed_url: ParseResult):
        query_params = parse_qs(parsed_url.query)
        extended = query_params.get('extended', ['false'])[0].lower() == 'true'
        
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        
        if extended:
            self.wfile.write(json.dumps(MOCK_INSTANCE_METADATA_EXTENDED).encode())
        else:
            self.wfile.write(json.dumps(MOCK_INSTANCE_METADATA).encode())
        return

    def do_GET(self):
        """Handle GET requests to IMDS endpoints."""
        parsed_url = urlparse(self.path)
        
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

        if parsed_url.path == '/metadata/instance':
            # If we have custom responses from a file, we should use them.
            if self._responses is not None:
                self.write_custom_response()
                return 
            
            # If we have no custom responses, return the default JSON depending on URL params
            else:
                self.write_default_response(parsed_url)
                return
        
        else:
            self.send_response(404)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            error_response = {"error": "Not Found", "message": f"Endpoint {parsed_url.path} not found"}
            self.wfile.write(json.dumps(error_response).encode())

# Mock metadata responses
MOCK_INSTANCE_METADATA_EXTENDED = {
  "compute": {
    "additionalCapabilities": {
      "hibernationEnabled": "false"
    },
    "azEnvironment": "AzurePublicCloud",
    "customData": "",
    "evictionPolicy": "",
    "extendedLocation": {
      "name": "",
      "type": ""
    },
    "host": {
      "id": ""
    },
    "hostGroup": {
      "id": ""
    },
    "isHostCompatibilityLayerVm": "false",
    "isVmInStandbyPool": "",
    "licenseType": "",
    "location": "eastus2",
    "name": "temp-vm",
    "offer": "ubuntu-24_04-lts",
    "osProfile": {
      "adminUsername": "azureuser",
      "computerName": "temp-vm",
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
    "platformSubFaultDomain": "",
    "platformUpdateDomain": "0",
    "priority": "",
    "provider": "Microsoft.Compute",
    "publicKeys": [
      {
        "keyData": "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQDDzWg4+F2u2geSdS2BV1bt98hsfcxriRBnqrpzbiZACcOBdzw2OeGBdekh2/sKIq3982u/3VmOw0FBKD+ef7ZLNkYQGwKEHy+/NWfgE412iHy5u6Zu2rpQdA3X2suk+jaZRoglRuQ5FeQd0M7HM+KPQhrYhPC77cbTMOjVe23ZbeR3xjVZ7of4U9DD3vshkpFZVJRfJHpRyS4xDA7sTUQETeUfMEQ6YwJHP7yps8+J2c4y0y6m2Od9BOw3Da7W70qVyL8G/1MiTRqa44wzkLzTbwXOvGFQeYR2ISSTjJYYkN7IRfAWyT9TAtKcOGHu3/DQesrDuwi9aq22aata2psXXQCLTlScZEN2vy0aY7Vkejq7X7TtmrQrPijnTqw6eRar+bWSjNP/Vto+FC5EBTx9fuTsnxGDgK/GOlghDf4vq6XzouULoMXtOARMnRkXk0Ev4uAKpf6BZ16liC+HErXq51TAmBWubpVimd5hbH39ZdiQ3ZxYqFFdSRGoHSdPiCXjJO9imboDSK1g9wFm7tAEyo2N/DgC1KlByilX4Ws4Qmm75PyG9547+V2lt8BjroQ4bQmrX2lIeCdwr1462Szz1GdwfkKPE1k5rpOFuFMMEIMTcN9H2KPbH3J9Arl3y29ip/tXcOUaXPpMm6TTF9S3Bjr0pmSABbul8jvxXGarew== azureuser@temp-vm\n",
        "path": "/home/azureuser/.ssh/authorized_keys"
      }
    ],
    "publisher": "canonical",
    "resourceGroupName": "temp-rg",
    "resourceId": "/subscriptions/12345678-90ab-cdef-1234-567890abcdef/resourceGroups/test-rg/providers/Microsoft.Compute/virtualMachines/temp-vm",
    "securityProfile": {
      "encryptionAtHost": "false",
      "secureBootEnabled": "false",
      "securityType": "",
      "virtualTpmEnabled": "false"
    },
    "sku": "server-gen1",
    "storageProfile": {
      "dataDisks": [],
      "imageReference": {
        "communityGalleryImageId": "",
        "exactVersion": "24.04.202510010",
        "id": "",
        "offer": "ubuntu-24_04-lts",
        "publisher": "canonical",
        "sharedGalleryImageId": "",
        "sku": "server-gen1",
        "version": "latest"
      },
      "osDisk": {
        "caching": "ReadWrite",
        "createOption": "FromImage",
        "diffDiskSettings": {
          "option": ""
        },
        "diskSizeGB": "30",
        "encryptionSettings": {
          "diskEncryptionKey": {
            "secretUrl": "",
            "sourceVault": {
              "id": ""
            }
          },
          "enabled": "false",
          "keyEncryptionKey": {
            "keyUrl": "",
            "sourceVault": {
              "id": ""
            }
          }
        },
        "image": {
          "uri": ""
        },
        "managedDisk": {
          "id": "/subscriptions/12345678-90ab-cdef-1234-567890abcdef/resourceGroups/temp-rg/providers/Microsoft.Compute/disks/temp-vm_OsDisk_1",
          "storageAccountType": "Premium_LRS"
        },
        "name": "temp-vm",
        "osType": "Linux",
        "vhd": {
          "uri": ""
        },
        "writeAcceleratorEnabled": "false"
      },
      "resourceDisk": {
        "size": "105728"
      }
    },
    "subscriptionId": "12345678-90ab-cdef-1234-567890abcdef",
    "tags": "deployment_end:2025-10-08T13:52:29.401086Z;deployment_start:2025-10-08T13:51:50.291058Z",
    "tagsList": [
      {
        "name": "deployment_end",
        "value": "2025-10-08T13:52:29.401086Z"
      },
      {
        "name": "deployment_start",
        "value": "2025-10-08T13:51:50.291058Z"
      }
    ],
    "userData": "",
    "version": "24.04.202510010",
    "virtualMachineScaleSet": {
      "id": ""
    },
    "vmId": "12345678-90ab-cdef-1234-567890abcdef",
    "vmScaleSetName": "",
    "vmSize": "Standard_D2ds_v5",
    "zone": ""
  },
  "network": {
    "interface": [
      {
        "ipv4": {
          "ipAddress": [
            {
              "privateIpAddress": "10.0.0.4",
              "publicIpAddress": ""
            }
          ],
          "subnet": [
            {
              "address": "10.0.0.0",
              "prefix": "16"
            }
          ]
        },
        "ipv6": {
          "ipAddress": []
        },
        "macAddress": "000D3A123456"
      }
    ]
  },
  "extended": {
    "compute": {
      "hasCustomData": False,
      "ppsType": "None"
    }
  }
}

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
            "adminUsername": "azureuser",
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
        "publicKeys": [
            {
                "keyData": "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAACAQDDzWg4+F2u2geSdS2BV1bt98hsfcxriRBnqrpzbiZACcOBdzw2OeGBdekh2/sKIq3982u/3VmOw0FBKD+ef7ZLNkYQGwKEHy+/NWfgE412iHy5u6Zu2rpQdA3X2suk+jaZRoglRuQ5FeQd0M7HM+KPQhrYhPC77cbTMOjVe23ZbeR3xjVZ7of4U9DD3vshkpFZVJRfJHpRyS4xDA7sTUQETeUfMEQ6YwJHP7yps8+J2c4y0y6m2Od9BOw3Da7W70qVyL8G/1MiTRqa44wzkLzTbwXOvGFQeYR2ISSTjJYYkN7IRfAWyT9TAtKcOGHu3/DQesrDuwi9aq22aata2psXXQCLTlScZEN2vy0aY7Vkejq7X7TtmrQrPijnTqw6eRar+bWSjNP/Vto+FC5EBTx9fuTsnxGDgK/GOlghDf4vq6XzouULoMXtOARMnRkXk0Ev4uAKpf6BZ16liC+HErXq51TAmBWubpVimd5hbH39ZdiQ3ZxYqFFdSRGoHSdPiCXjJO9imboDSK1g9wFm7tAEyo2N/DgC1KlByilX4Ws4Qmm75PyG9547+V2lt8BjroQ4bQmrX2lIeCdwr1462Szz1GdwfkKPE1k5rpOFuFMMEIMTcN9H2KPbH3J9Arl3y29ip/tXcOUaXPpMm6TTF9S3Bjr0pmSABbul8jvxXGarew== azureuser@temp-vm\n",
                "path": "/home/azureuser/.ssh/authorized_keys"
            }
        ],
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