import argparse
import subprocess
import threading
import time
import json
import signal
import os
import sys
from http.server import HTTPServer

from config import WIRESERVER_IP, WIRESERVER_PORT, IMDS_IP, IMDS_PORT, DUMMY_IFACE
from utils import logger, run_cmd

from wireserver_handler import WireServerHandler
from imds_handler import IMDSHandler


class TestServer:
    """Main test server class that manages network setup and HTTP servers."""

    def __init__(self, imds_responses_file=None):
        self.imds_server = None
        self.wireserver_server = None
        self.imds_thread = None
        self.wireserver_thread = None
        self.running = False
        self.imds_responses_file = imds_responses_file

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
            run_cmd(
                f"sudo ip route del {IMDS_IP} dev {DUMMY_IFACE} 2>/dev/null || true"
            )
            run_cmd(
                f"sudo ip route del {WIRESERVER_IP} dev {DUMMY_IFACE} 2>/dev/null || true"
            )

            # Remove the dummy interface
            run_cmd(f"sudo ip link delete {DUMMY_IFACE} 2>/dev/null || true")

            logger.info("Network interface cleanup completed")

        except subprocess.CalledProcessError as e:
            logger.warning(f"Error during cleanup (this might be expected): {e}")

    def start_imds_server(self):
        """Start the IMDS HTTP server."""
        if self.imds_responses_file:
            IMDSHandler.set_response_file_path(self.imds_responses_file)
            IMDSHandler.load_responses()
            logger.info(
                f"IMDS handler will load responses from: {self.imds_responses_file}"
            )

        logger.info(f"Starting IMDS server on {IMDS_IP}:{IMDS_PORT}")
        self.imds_server = HTTPServer((IMDS_IP, IMDS_PORT), IMDSHandler)
        self.imds_server.serve_forever()

    def start_wireserver_server(self):
        """Start the WireServer HTTP server."""
        logger.info(f"Starting WireServer on {WIRESERVER_IP}:{WIRESERVER_PORT}")
        self.wireserver_server = HTTPServer(
            (WIRESERVER_IP, WIRESERVER_PORT), WireServerHandler
        )
        self.wireserver_server.serve_forever()

    def start(self):
        """Start the test server."""
        logger.info("Starting provisioning agent test server...")

        try:
            self.setup_network_interface()

            # Start HTTP servers in separate threads
            self.imds_thread = threading.Thread(
                target=self.start_imds_server, daemon=True
            )
            self.wireserver_thread = threading.Thread(
                target=self.start_wireserver_server, daemon=True
            )

            self.imds_thread.start()
            self.wireserver_thread.start()

            self.running = True
            logger.info("Test server started successfully!")
            logger.info(
                f"IMDS endpoint: http://{IMDS_IP}:{IMDS_PORT}/metadata/instance"
            )
            logger.info(
                f"WireServer endpoint: http://{WIRESERVER_IP}:{WIRESERVER_PORT}"
            )

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

        self.imds_server.shutdown()
        self.wireserver_server.shutdown()

        self.cleanup_network_interface()
        logger.info("Test server stopped")


def signal_handler(sig, frame):
    """Handle SIGINT (Ctrl+C) gracefully."""
    logger.info("Received SIGINT, shutting down...")
    sys.exit(0)


def parse_arguments():
    """Parse command line arguments."""
    parser = argparse.ArgumentParser(
        description="Azure provisioning agent test server",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )

    parser.add_argument(
        "--imds-responses",
        type=str,
        metavar="JSON_FILE",
        help="Path to JSON file containing custom IMDS responses",
    )

    parser.add_argument(
        "--wireserver-responses",
        type=str,
        metavar="XML_FILE",
        help="Path to XML file containing custom wireserver responses",
    )

    return parser.parse_args()


def validate_json_file(file_path):
    """Validate that the file exists and is valid JSON."""
    if not os.path.exists(file_path):
        logger.error(f"JSON file not found: {file_path}")
        sys.exit(1)

    try:
        with open(file_path, "r") as f:
            json.load(f)
        logger.info(f"Validated JSON file: {file_path}")
    except json.JSONDecodeError as e:
        logger.error(f"Invalid JSON in file: {e}")
        sys.exit(1)
    except Exception as e:
        logger.error(f"Error reading file: {e}")
        sys.exit(1)


if __name__ == "__main__":
    args = parse_arguments()

    if args.imds_responses:
        validate_json_file(args.imds_responses)
    # if args.wireserver_responses:
    #     validate_json_file(args.wireserver_responses)

    signal.signal(signal.SIGINT, signal_handler)

    server = TestServer(args.imds_responses, args.wireserver_responses)
    try:
        server.start()
    except KeyboardInterrupt:
        pass
    finally:
        server.stop()
