"""
Tests for scripts/check-e2e-targets.sh: verify that the script distinguishes
a JSON-RPC 404 error ("account not found" - expected, actionable state)
from any other error code (a genuine RPC/server problem).

Run:
    pytest tests/test_check_e2e_targets.py -v

The script is run as a real subprocess against a local mock JSON-RPC
server, so the test exercises the actual bash+python pipeline rather than
a reimplementation of its logic in Python.
"""

import json
import os
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

SCRIPT_PATH = os.path.join(
    os.path.dirname(__file__), "..", "scripts", "check-e2e-targets.sh"
)

VALID_HEX = {
    "E2E_PROGRAM_ID": "53def2dc8516302842b10e356914d2a5f6b33425ba42aec684f706aa1cf64192",
    "ORACLE_PROGRAM_ID": "eee682c27db375bebbc17ed9a76aaa935c8b72bc7de50d736f03e2dfbed84b15",
    "E2E_MARKET": "9a5a237ddb156c367952ea3562ab3d05f3cdaf0e9bf6ba4fb7b76e233e181f53",
    "E2E_AUSD_MINT": "55c6cee38a31732e2dad821ab1c38f902a7c51efaefb3641d51f3485c4617a45",
    "E2E_ABTC_MINT": "1d46e0dd87393236e4e01252439f46dcbaec7c2255d1fd734e61771a00e8f4e9",
}


def _make_handler(rpc_error_code, rpc_error_message):
    """Mock JSON-RPC server that answers every read_account_info call with
    the given JSON-RPC error (HTTP 200, error in the body - matching how
    a real Arch node behaves)."""

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            length = int(self.headers.get("Content-Length", 0))
            self.rfile.read(length)

            body = json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": {"code": rpc_error_code, "message": rpc_error_message},
                }
            ).encode()

            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    return Handler


def _run_script_against_mock(rpc_error_code, rpc_error_message):
    handler_cls = _make_handler(rpc_error_code, rpc_error_message)
    server = HTTPServer(("127.0.0.1", 0), handler_cls)
    port = server.server_address[1]

    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    env = os.environ.copy()
    env.update(VALID_HEX)
    env["E2E_RPC"] = f"http://127.0.0.1:{port}"

    try:
        result = subprocess.run(
            ["bash", SCRIPT_PATH],
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )
    finally:
        server.shutdown()
        thread.join(timeout=5)

    return result


def test_404_is_classified_as_missing_not_problem():
    """Error code 404 ("not found") is an expected state: the account does
    not exist. This is exactly the code arch_sdk=0.6.2 (NOT_FOUND_CODE)
    treats as "not found", so the script must classify it as MISSING."""
    result = _run_script_against_mock(404, "not found")

    assert result.returncode != 0
    assert "MISSING" in result.stderr
    assert "PROBLEM" not in result.stderr, (
        "a 404 error must not be classified as PROBLEM"
    )


def test_500_is_classified_as_problem_not_missing():
    """Error code 500 (internal error) is a genuine RPC/server problem, not
    a missing account. arch_sdk=0.6.2 returns ArchError::RpcRequestFailed
    in this case, not "not found". The script must not misdiagnose this
    as an account being missing."""
    result = _run_script_against_mock(500, "internal error")

    assert result.returncode != 0
    assert "PROBLEM" in result.stderr
    assert "RPC error 500" in result.stderr
    assert "MISSING" not in result.stderr, (
        "a 500 error is an RPC-node problem, not a missing account, and "
        "must not be classified as MISSING"
    )


if __name__ == "__main__":
    sys.exit(subprocess.call([sys.executable, "-m", "pytest", __file__, "-v"]))
