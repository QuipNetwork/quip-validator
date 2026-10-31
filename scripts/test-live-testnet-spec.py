#!/usr/bin/env python3
"""Exercise integrity and smoke guards; --node adds a real loopback dial mutant."""

import argparse
import json
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import types
from unittest import mock


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def load_smoke(script, code=None):
    # Execute with optimization even when the fixture runner itself uses normal
    # Python: production guards must survive the equivalent of python -O.
    module = types.ModuleType("smoke_fixture")
    module.__file__ = str(script)
    exec(compile(code or script.read_text(), str(script), "exec", optimize=2), module.__dict__)
    return module


def check_smoke_prelaunch_guard(source, root):
    smoke_source = (source / "scripts/smoke-live-testnet-spec.py").read_text()
    clearing = '    smoke["bootNodes"] = []\n'
    require(smoke_source.count(clearing) == 1, "missing clearing mutation anchor")
    canonical = json.loads((root / "chain-specs/aglais-network.json").read_text())
    require(canonical["bootNodes"], "negative fixture needs shipped bootnodes")

    class LaunchReached(Exception):
        pass

    # Exercise main's actual preparation and run_node path. Intercept Popen so
    # even a broken invariant cannot launch a node or contact a shipped peer.
    for label, code, should_reach_launch in [
        ("isolated smoke spec", smoke_source, True),
        ("omitted bootnode clearing", smoke_source.replace(clearing, ""), False),
    ]:
        script = root / "scripts/smoke-live-testnet-spec.py"
        script.write_text(code)
        module = load_smoke(script, code)
        argv = [str(script), sys.executable, "--log-dir", str(root / "logs")]
        with mock.patch.object(sys, "argv", argv), mock.patch.object(
            module.subprocess, "Popen", side_effect=LaunchReached
        ) as launch:
            try:
                module.main()
            except LaunchReached:
                require(should_reach_launch, "unsafe smoke spec reached Popen")
            except RuntimeError as error:
                require(not should_reach_launch, str(error))
                require("bootNodes" in str(error), str(error))
            else:
                raise AssertionError("fixture unexpectedly completed")
            require(launch.call_count == int(should_reach_launch), "wrong process-launch count")
        print(f"{label}: pre-launch gate verified; no process started")


def check_observation_guards(source, root):
    # Synthetic clock/log fixtures exercise the full run_node path without a
    # node. An actual loopback-peer injection is available with --node below.
    for label, marker, delayed_dial, expected_error in [
        ("observed clean window", True, False, None),
        ("missing discovery marker", False, False, "discovery observation timed out"),
        ("late dial during window", True, True, "outbound dial"),
    ]:
        module = load_smoke(source / "scripts/smoke-live-testnet-spec.py")
        now = [0.0]
        log_path = root / (label.replace(" ", "-") + ".log")
        process = mock.Mock(returncode=0)
        process.poll.return_value = None
        process.send_signal.side_effect = lambda _: setattr(process.poll, "return_value", 0)

        def launch(*args, **kwargs):
            kwargs["stdout"].write("Running litep2p network backend\nlistening on: loopback\n")
            kwargs["stdout"].flush()
            return process

        def sleep(seconds):
            previous = now[0]
            now[0] += seconds
            with log_path.open("a") as log:
                if marker and previous < 5 <= now[0]:
                    log.write("TRACE sub-libp2p::discovery: start next kademlia query for fixture\n")
                if delayed_dial and previous < 9 <= now[0]:
                    log.write("DEBUG litep2p: dialing loopback fixture\n")

        def rpc(port, method, params):
            return {"chain_getBlockHash": module.GENESIS_HASH,
                    "chain_getHeader": {"number": "0x0"}, "system_health": {"peers": 0}}[method]

        with mock.patch.object(module.subprocess, "Popen", side_effect=launch), \
             mock.patch.object(module.time, "monotonic", side_effect=lambda: now[0]), \
             mock.patch.object(module.time, "sleep", side_effect=sleep), \
             mock.patch.object(module, "rpc", side_effect=rpc):
            try:
                module.run_node(Path("fixture-node"), root / "empty.json", root / "db", log_path, 12)
            except RuntimeError as error:
                require(expected_error and expected_error in str(error), str(error))
            else:
                require(expected_error is None, f"{label} unexpectedly passed")
        require(now[0] >= 10, f"{label}: observation window was shortened")
        require(process.send_signal.call_count == 1, "node was not shut down")
        print(f"{label}: optimized smoke guard verified")


def check_loopback_dial(source, binary, log_dir):
    module = load_smoke(source / "scripts/smoke-live-testnet-spec.py")
    canonical = json.loads((source / "chain-specs/aglais-network.json").read_text())
    # Only reuse the public peer ID syntax; never pass its published address.
    peer_id = canonical["bootNodes"][0].rsplit("/p2p/", 1)[1]
    real_launch = subprocess.Popen
    accepted = threading.Event()
    stop = threading.Event()
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(0.2)
        address = f"/ip4/127.0.0.1/tcp/{listener.getsockname()[1]}/p2p/{peer_id}"

        def accept():
            while not stop.is_set():
                try:
                    connection, _ = listener.accept()
                except socket.timeout:
                    continue
                with connection:
                    accepted.set()

        thread = threading.Thread(target=accept)
        thread.start()

        def inject(command, **kwargs):
            spec_path = next(arg.split("=", 1)[1] for arg in command if arg.startswith("--chain="))
            require(json.loads(Path(spec_path).read_text())["bootNodes"] == [],
                    "B1 fixture must satisfy the independent F2 gate")
            return real_launch(command + [f"--reserved-nodes={address}"], **kwargs)

        log_dir.mkdir(parents=True, exist_ok=True)
        argv = [str(source / "scripts/smoke-live-testnet-spec.py"), str(binary), "--log-dir", str(log_dir)]
        try:
            with mock.patch.object(sys, "argv", argv), mock.patch.object(module.subprocess, "Popen", side_effect=inject):
                try:
                    module.main()
                except RuntimeError as error:
                    require("outbound dial:" in str(error), f"wrong B1 failure: {error}")
                else:
                    raise RuntimeError("loopback reserved-peer mutant unexpectedly passed")
            require(accepted.is_set(), "loopback listener never received the injected dial")
            print("loopback reserved-peer mutant: empty spec bootNodes; real connection; rejected by dial guard")
        finally:
            stop.set()
            thread.join(timeout=2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node", type=Path, help="also run the real loopback dial mutant")
    parser.add_argument("--log-dir", type=Path, help="retain real loopback mutant logs")
    args = parser.parse_args()
    require(not args.node or args.log_dir, "--node requires --log-dir")
    source = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="live-testnet-spec-") as directory:
        root = Path(directory)
        (root / "scripts").mkdir()
        (root / "chain-specs").mkdir()
        checker = root / "scripts/check-live-testnet-spec.py"
        shutil.copyfile(source / "scripts/check-live-testnet-spec.py", checker)
        spec = root / "chain-specs/aglais-network.json"
        original = (source / spec.relative_to(root)).read_bytes()

        def check(label, expected_error=None):
            result = subprocess.run([sys.executable, str(checker)], capture_output=True, text=True)
            output = result.stdout + result.stderr
            if expected_error is None:
                require(result.returncode == 0, output)
            else:
                require(result.returncode != 0 and expected_error in output, output)
            print(f"{label}: exit {result.returncode}\n{output}", end="")

        spec.write_bytes(original)
        check("original")
        spec.write_bytes(original + b" ")
        check("one byte appended", "SHA-256 mismatch")
        equivalent = json.dumps(json.loads(original), separators=(",", ":")).encode()
        require(equivalent != original and json.loads(equivalent) == json.loads(original), "bad reserialization fixture")
        spec.write_bytes(equivalent)
        check("semantic no-op reserialization", "SHA-256 mismatch")
        spec.unlink()
        check("missing file", "cannot read artifact")
        spec.write_bytes(original)
        check("restored original")
        check_smoke_prelaunch_guard(source, root)
        check_observation_guards(source, root)
    if args.node:
        check_loopback_dial(source, args.node.resolve(strict=True), args.log_dir)


if __name__ == "__main__":
    main()
