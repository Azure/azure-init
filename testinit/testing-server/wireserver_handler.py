from http.server import BaseHTTPRequestHandler
import json
import time

from config import WIRESERVER_GET_DELAY, WIRESERVER_GET_TIMEOUT
from utils import logger

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


class WireServerHandler(BaseHTTPRequestHandler):
    """HTTP handler for Azure WireServer requests."""
    
    def do_GET(self):
        """Handle GET requests to WireServer endpoints."""
        logger.info(f"WireServer GET request: {self.path}")
        logger.info(f"Headers: {dict(self.headers)}")

        if WIRESERVER_GET_TIMEOUT:
            logger.info(f"Adding wireserver GET timeout from ENV variable")
            return

        if WIRESERVER_GET_DELAY != 0:
            logger.info(f"Adding wireserver GET request delay of {WIRESERVER_GET_DELAY} seconds")
            time.sleep(WIRESERVER_GET_DELAY)
        
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
