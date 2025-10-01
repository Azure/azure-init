import subprocess
import threading
import time
import signal
import sys
from http.server import HTTPServer

from config import WIRESERVER_IP, WIRESERVER_PORT, IMDS_IP, IMDS_PORT, DUMMY_IFACE
from utils import logger, run_cmd

from wireserver_handler import WireServerHandler
from imds_handler import IMDSHandler

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
    
    server = TestServer()
    try:
        server.start()
    except KeyboardInterrupt:
        pass
    finally:
        server.stop()
