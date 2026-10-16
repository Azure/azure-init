from contextlib import ExitStack
from http.server import HTTPServer
import unittest
from unittest.mock import MagicMock, call, patch

import config
import healthcheck
from imds_handler import IMDSHandler
from wireserver_handler import WireServerHandler


class HealthCheckTests(unittest.TestCase):
    def test_checks_both_configured_endpoints_and_closes_connections(self):
        connections = [MagicMock(), MagicMock()]
        with patch(
            "healthcheck.socket.create_connection", side_effect=connections
        ) as connect:
            healthcheck.check_readiness()

        self.assertEqual(
            connect.call_args_list,
            [
                call((config.IMDS_IP, config.IMDS_PORT), timeout=2),
                call((config.WIRESERVER_IP, config.WIRESERVER_PORT), timeout=2),
            ],
        )
        for connection in connections:
            connection.__exit__.assert_called_once_with(None, None, None)

    def test_fails_if_either_endpoint_refuses_or_times_out(self):
        for endpoint in range(2):
            for error in (ConnectionRefusedError, TimeoutError):
                with self.subTest(endpoint=endpoint, error=error):
                    connections = [MagicMock() for _ in range(endpoint)]
                    with patch(
                        "healthcheck.socket.create_connection",
                        side_effect=[*connections, error("endpoint unavailable")],
                    ):
                        with self.assertRaises(error):
                            healthcheck.check_readiness()
                    for connection in connections:
                        connection.__exit__.assert_called_once_with(None, None, None)

    def test_repeated_probes_preserve_scripted_failure_responses(self):
        with ExitStack() as stack:
            for handler in (IMDSHandler, WireServerHandler):
                stack.enter_context(
                    patch.object(
                        handler, "_responses", [{"status_code": 500, "response": None}]
                    )
                )
                stack.enter_context(patch.object(handler, "_response_position", 0))

            imds = stack.enter_context(HTTPServer(("127.0.0.1", 0), IMDSHandler))
            wireserver = stack.enter_context(
                HTTPServer(("127.0.0.1", 0), WireServerHandler)
            )
            imds.timeout = wireserver.timeout = 1
            stack.enter_context(
                patch.multiple(
                    healthcheck,
                    IMDS_IP=imds.server_address[0],
                    IMDS_PORT=imds.server_address[1],
                    WIRESERVER_IP=wireserver.server_address[0],
                    WIRESERVER_PORT=wireserver.server_address[1],
                )
            )

            for _ in range(3):
                healthcheck.check_readiness()
                imds.handle_request()
                wireserver.handle_request()
                self.assertEqual(IMDSHandler._response_position, 0)
                self.assertEqual(WireServerHandler._response_position, 0)


if __name__ == "__main__":
    unittest.main()
