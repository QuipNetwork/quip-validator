#!/usr/bin/env python3
"""Start/reopen a genesis-only DB. Strip ONLY temporary bootnodes: never dial production.

This exercises the shipped genesis, not its shipped peer list, live sync,
consensus, validator keys, or a long-lived production database. Only the
litep2p backend is exercised; libp2p coverage is outside this smoke test.
"""

import argparse
import copy
import json
from pathlib import Path
import re
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


GENESIS_HASH = "0x542a9915acf840b8940d35ca6de242f669a0c6e3b40f9a8d8c14b9bf7b8ebecb"
# SDK 845eb99 discovery.rs:68 sets a five-second query interval. Observe at
# least two intervals after RPC readiness AND require the real discovery log.
KADEMLIA_OBSERVATION_SECONDS = 10
DISCOVERY_QUERY = re.compile(r"sub-libp2p::discovery.*start next kademlia query for")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def rpc(port, method, params):
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}",
        json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
        {"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=2) as response:
        result = json.load(response)
    if "error" in result:
        raise RuntimeError(f"{method}: {result['error']}")
    return result["result"]


def run_node(binary, spec, base, log_path, timeout):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    # Isolation relies on empty bootnodes/reserved peers and disabled mDNS.
    # This SDK litep2p backend still runs periodic Kademlia queries; a short
    # no-dial log is not a substitute for the empty-bootnodes pre-launch gate.
    # The pinned transport-manager logs every dial at DEBUG; keep it enabled.
    command = [
        str(binary), f"--chain={spec}", f"--base-path={base}",
        f"--rpc-port={port}", "--rpc-methods=Safe",
        "--listen-addr=/ip4/127.0.0.1/tcp/0", "--network-backend=litep2p",
        "--in-peers=0", "--out-peers=0", "--in-peers-light=0", "--reserved-only",
        "--no-mdns", "--no-telemetry", "--no-prometheus", "--offchain-worker=never",
        "--log=sub-libp2p=trace,litep2p=trace", "--disable-log-color",
    ]
    with log_path.open("w") as log:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + timeout
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f"node exited early ({process.returncode}); see {log_path}")
                try:
                    actual = rpc(port, "chain_getBlockHash", [0])
                except (urllib.error.URLError, TimeoutError, ConnectionError):
                    if time.monotonic() >= deadline:
                        raise RuntimeError(f"RPC readiness timed out; see {log_path}")
                    time.sleep(0.2)
                    continue
                require(actual == GENESIS_HASH, f"unexpected genesis: {actual}")
                header = rpc(port, "chain_getHeader", [GENESIS_HASH])
                require(header and int(header["number"], 16) == 0, f"unexpected header: {header}")
                require(rpc(port, "system_health", [])["peers"] == 0, "unexpected peers")
                require(process.poll() is None, "node exited during RPC checks")
                break
            hold_until = time.monotonic() + KADEMLIA_OBSERVATION_SECONDS
            while True:
                require(process.poll() is None, f"node exited during discovery observation: {log_path}")
                now = time.monotonic()
                require(now < deadline, f"discovery observation timed out; need 10s and Kademlia query log: {log_path}")
                if now >= hold_until and DISCOVERY_QUERY.search(log_path.read_text()):
                    break
                time.sleep(min(0.2, deadline - now))
            require(rpc(port, "system_health", [])["peers"] == 0, "unexpected peers after observation")
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                    raise RuntimeError(f"node did not close gracefully; see {log_path}")
    # The known txpool-background ERROR during SIGINT is teardown noise; check process exit.
    require(process.returncode == 0, f"node exit {process.returncode}; see {log_path}")
    output = log_path.read_text()
    require("Running litep2p network backend" in output, "missing network startup log")
    require("listening on:" in output, "network debug logging was not enabled")
    require(DISCOVERY_QUERY.search(output), "missing Kademlia query log; observation is unproven")
    # Catch transport-manager dial messages and network-level failures/events.
    require(not re.search(r"\bdial(?:ing)?\b", output, re.IGNORECASE), f"outbound dial: {log_path}")
    print(f"{log_path.stem}: RPC genesis {GENESIS_HASH}; zero peers; observed Kademlia query and >=10s after RPC; no outbound dial logged")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("node", type=Path)
    parser.add_argument("--log-dir", type=Path, required=True)
    # Cold compilation of the historical Wasm is substantial in a debug node.
    parser.add_argument("--timeout", type=float, default=300)
    args = parser.parse_args()
    require(args.timeout > 0, "--timeout must be positive")
    binary = args.node.resolve(strict=True)
    args.log_dir.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parent.parent / "chain-specs/aglais-network.json"
    original_bytes = source.read_bytes()
    canonical = json.loads(original_bytes)
    smoke = copy.deepcopy(canonical)
    smoke["bootNodes"] = []
    require(smoke["bootNodes"] == [], "smoke bootNodes must be empty before launch")
    require({k: v for k, v in smoke.items() if k != "bootNodes"} == {
        k: v for k, v in canonical.items() if k != "bootNodes"
    }, "smoke spec changed fields other than bootNodes")
    with tempfile.TemporaryDirectory(prefix="live-testnet-startup-") as directory:
        root = Path(directory)
        spec = root / "no-bootnodes.json"
        spec.write_text(json.dumps(smoke))
        base = root / "db"
        require(not base.exists(), "fresh DB path already exists")
        run_node(binary, spec, base, args.log_dir / "fresh.log", args.timeout)
        require(base.is_dir() and any(base.rglob("*")), "fresh run did not create DB files")
        run_node(binary, spec, base, args.log_dir / "restart.log", args.timeout)
    require(source.read_bytes() == original_bytes, "canonical artifact changed")
    print("PASS: fresh genesis DB and same-DB restart; temporary bootnodes removed")


if __name__ == "__main__":
    main()
