from http.server import BaseHTTPRequestHandler
import time
import json
import xml.etree.ElementTree as ET

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
        tree = ET.parse(cls._responses_file_path)
        root = tree.getroot()

        cls._responses = []

        for response_elem in root.findall("response"):
            response_dict = {}

            status_code = response_elem.find("status_code")
            if status_code is not None:
                response_dict["status_code"] = int(status_code.text)

            headers_elem = response_elem.find("headers")
            if headers_elem is not None:
                headers = {}
                for header in headers_elem.findall("header"):
                    name = header.get("name")
                    value = header.get("value")
                    if name and value:
                        headers[name] = value
                response_dict["headers"] = headers

            response_body = response_elem.find("response_body")
            if response_body is not None:
                response_dict["response"] = response_body.text

            delay_elem = response_elem.find("delay")
            if delay_elem is not None:
                response_dict["delay"] = int(delay_elem.text)

            cls._responses.append(response_dict)

        logger.info(f"PRINTING RESPONSES {cls._responses}")

    def write_custom_response(self):
        responses_list = self._responses

        if self.__class__._response_position >= len(responses_list):
            self.__class__._response_position = 0

        current_response = responses_list[self.__class__._response_position]

        delay = current_response.get("delay")
        if delay is not None:
            logger.info(f"Adding custom WireServer delay of {delay} seconds")
        #            time.sleep(delay)

        self.send_response(current_response["status_code"])

        headers = current_response.get("headers", {})
        for header_name, header_value in headers.items():
            self.send_header(header_name, header_value)
        self.end_headers()

        response_body = ""

        self.wfile.write(response_body.encode())

        logger.info(
            f"Returning response from position: {self.__class__._response_position}"
        )
        logger.info(f"Response details:\n{json.dumps(current_response, indent=2)}")
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

        if self._responses is not None:
            self.write_custom_response()
            return

        else:
            self.write_default_response()
            return
