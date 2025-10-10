from http.server import BaseHTTPRequestHandler
import time
import json

from utils import logger


class WireServerHandler(BaseHTTPRequestHandler):
    """HTTP handler for Azure WireServer requests."""

    _responses_file_path = None
    _responses = None
    _response_position = 0

    @classmethod
    def set_response_file_path(cls, file_path: str):
        cls._responses_file_path = file_path

    @classmethod
    def load_responses(cls):
        with open(cls._responses_file_path, "r") as f:
            cls._responses = json.load(f)

        cls._responses = cls._responses["responses"]

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

        self.send_response(current_response["status_code"])

        headers = current_response.get("headers", {})
        for header_name, header_value in headers.items():
            self.send_header(header_name, header_value)
        self.end_headers()

        response_body = current_response.get("response", {})
        self.wfile.write(json.dumps(response_body).encode())

        logger.info(
            f"Returning response: {json.dumps(current_response, indent=2)}, from position: {self.__class__._response_position}"
        )
        self.__class__._response_position += 1
        return

    def write_default_response(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/xml")
        self.end_headers()
        response = '<?xml version="1.0" encoding="utf-8"?><Response>OK</Response>'
        self.wfile.write(response.encode())

        return

    def do_POST(self):
        """Handle POST requests to WireServer endpoints."""
        content_length = int(self.headers.get("Content-Length", 0))
        post_data = self.rfile.read(content_length)

        logger.info(f"WireServer POST request: {self.path}")
        logger.info(f"POST data: {post_data.decode('utf-8', errors='ignore')}")

        # Mock successful response for any POST
        self.send_response(200)
        self.send_header("Content-Type", "application/xml")
        self.end_headers()
        response = '<?xml version="1.0" encoding="utf-8"?><Response>OK</Response>'
        self.wfile.write(response.encode())
