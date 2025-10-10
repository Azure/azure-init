from http.server import BaseHTTPRequestHandler

from utils import logger


class WireServerHandler(BaseHTTPRequestHandler):
    """HTTP handler for Azure WireServer requests."""
    
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
