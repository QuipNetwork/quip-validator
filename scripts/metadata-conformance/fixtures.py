#!/usr/bin/env python3
"""Capture or verify finalized metadata-conformance fixtures (stdlib only).

Capture writes a NEW directory. Verification never changes a fixture.
Every metadata query uses the block's parent state, including upgrade blocks.
"""

import argparse
import datetime
import hashlib
import json
from pathlib import Path
import urllib.request

EVENTS_KEY = "0x26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7"
CODE_KEY = "0x3a636f6465"
# SDK decl_runtime_apis identifies each trait by its Blake2b-64 name digest.
BENCHMARK_API_ID = "0x" + hashlib.blake2b(b"Benchmark", digest_size=8).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def rpc(url, method, params):
    request = urllib.request.Request(
        url,
        json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params)).encode(),
        {"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        result = json.load(response)
    require("error" not in result, f"{method}: {result.get('error')}")
    require(result.get("result") is not None, f"{method}: missing/pruned result")
    return result["result"]


def render(value, encoding):
    if encoding == "scale":
        require(isinstance(value, str) and value.startswith("0x"), "expected RPC hex")
        return bytes.fromhex(value[2:])
    require(encoding == "json", f"unknown encoding: {encoding}")
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def compact(data):
    mode = data[0] & 3
    width = (1, 2, 4, (data[0] >> 2) + 5)[mode]
    require(len(data) >= width, "truncated compact integer")
    value = int.from_bytes(data[1:width] if mode == 3 else data[:width], "little")
    return (value if mode == 3 else value >> 2), width


def wire_expectations(extrinsics):
    """Independent wire facts; no metadata decoder or invented signature bytes."""
    result = []
    for index, encoded in enumerate(extrinsics):
        raw = bytes.fromhex(encoded.removeprefix("0x"))
        size, prefix = compact(raw)
        body = raw[prefix:]
        require(len(body) == size, "extrinsic length mismatch")
        version = body[0]
        require(version in (5, 132), f"unexpected extrinsic format {version}")
        row = dict(index=index, version=version, signed=version == 132,
                   hash="0x" + hashlib.blake2b(raw, digest_size=32).hexdigest())
        call_offset = 1
        if row["signed"]:
            require(body[1] == 0 and len(body) >= 1698, "expected H4 MultiAddress::Id")
            envelope = body[34:1694]
            row.update(accountId="0x" + body[2:34].hex(),
                       publicKeySha256=hashlib.sha256(envelope[:929]).hexdigest(),
                       signatureFieldSha256=hashlib.sha256(envelope[929:]).hexdigest(),
                       envelopeSha256=hashlib.sha256(envelope).hexdigest())
            call_offset = 1694
            era_width = 1 if body[call_offset] == 0 else 2
            row["eraHex"] = "0x" + body[call_offset:call_offset + era_width].hex()
            call_offset += era_width
            for name in ("nonce", "tip"):
                value, width = compact(body[call_offset:])
                row[name] = str(value)
                call_offset += width
            row["metadataHashMode"] = body[call_offset]
            require(row["metadataHashMode"] == 0, "fixture expects disabled metadata hash")
            call_offset += 1
            row["extensionHex"] = "0x" + body[1694:call_offset].hex()
        row["callIndexHex"] = "0x" + body[call_offset:call_offset + 2].hex()
        row["callSha256"] = hashlib.sha256(body[call_offset:]).hexdigest()
        require(len(body[call_offset:]) >= 2, "missing pallet/call indexes")
        # Stable pallet/call indexes in these pinned runtimes. These wire
        # expectations are deliberately independent of consumer type registries.
        call = body[call_offset:]
        if call[:2] == b"\x01\x00":
            moment, width = compact(call[2:])
            require(width + 2 == len(call), "timestamp has trailing bytes")
            row.update(pallet="Timestamp", call="set", now=str(moment))
        elif call[:2] == b"\x04\x03":
            require(call[2] == 0, "expected transfer destination MultiAddress::Id")
            amount, width = compact(call[35:])
            require(width + 35 == len(call), "transfer has trailing bytes")
            row.update(pallet="Balances", call="transfer_keep_alive",
                       destination="0x" + call[3:35].hex(), amount=str(amount))
        result.append(row)
    return result


def capture(args):
    require(not args.directory.exists(), "capture destination must not exist")
    if args.kind == "local-non-benchmark":
        require(args.source_commit and args.wasm and args.build_features is not None,
                "local capture requires source commit, build features and Wasm")
        require("runtime-benchmarks" not in args.build_features, "benchmark build is not accepted")
    finalized = rpc(args.rpc, "chain_getFinalizedHead", [])
    height = int(rpc(args.rpc, "chain_getHeader", [finalized])["number"], 16)
    require(all(0 < n <= height for n in args.blocks), "only finalized non-genesis blocks")
    manifest = dict(schemaVersion=1, capturedAt=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    rpc=args.rpc, kind=args.kind, genesisHash=rpc(args.rpc, "chain_getBlockHash", [0]),
                    finalizedAtCapture=dict(hash=finalized, number=height),
                    sourceCommit=args.source_commit, buildFeatures=args.build_features,
                    files={}, blocks=[])
    if args.wasm:
        manifest["buildArtifact"] = dict(name=args.wasm.name,
                                        sha256=hashlib.sha256(args.wasm.read_bytes()).hexdigest())
    args.directory.mkdir(parents=True)

    def save(name, method, params, encoding="json"):
        value = rpc(args.rpc, method, params)
        data = render(value, encoding)
        (args.directory / name).write_bytes(data)
        manifest["files"][name] = dict(method=method, params=params, encoding=encoding,
                                       bytes=len(data), sha256=hashlib.sha256(data).hexdigest())
        return value

    for number in args.blocks:
        block_hash = rpc(args.rpc, "chain_getBlockHash", [number])
        block_file = f"block-{number}.json"
        block = save(block_file, "chain_getBlock", [block_hash])["block"]
        require(int(block["header"]["number"], 16) == number, "block number mismatch")
        parent = block["header"]["parentHash"]
        parent_version = save(f"runtime-parent-{number}.json", "state_getRuntimeVersion", [parent])
        if args.kind == "local-non-benchmark":
            require(all(api[0] != BENCHMARK_API_ID for api in parent_version["apis"]), "runtime advertises Benchmark API")
        post_version = save(f"runtime-post-{number}.json", "state_getRuntimeVersion", [block_hash])
        metadata_file = f"metadata-{number}.scale"
        metadata = save(metadata_file, "state_getMetadata", [parent], "scale")
        require(bytes.fromhex(metadata[2:])[:5] == b"meta\x10", "expected V16 metadata")
        code_hash = rpc(args.rpc, "state_getStorageHash", [CODE_KEY, parent])
        code = rpc(args.rpc, "state_getStorage", [CODE_KEY, parent])
        code_sha = hashlib.sha256(bytes.fromhex(code[2:])).hexdigest()
        if args.wasm:
            require(code_sha == manifest["buildArtifact"]["sha256"], "node runtime differs from supplied Wasm artifact")
        events_file = f"events-{number}.scale"
        save(events_file, "state_getStorage", [EVENTS_KEY, block_hash], "scale")
        manifest["blocks"].append(dict(number=number, hash=block_hash, parentHash=parent,
            executionSpecVersion=parent_version["specVersion"], postStateSpecVersion=post_version["specVersion"],
            transactionVersion=parent_version["transactionVersion"], metadataVersion=16,
            metadataStateHash=parent, metadataFile=metadata_file, blockFile=block_file, eventsFile=events_file,
            executionCodeStorageHash=code_hash, executionCodeSha256=code_sha,
            extrinsics=wire_expectations(block["extrinsics"])))
        print(f"captured #{number}: execution spec {parent_version['specVersion']}, post-state {post_version['specVersion']}")
    (args.directory / "manifest.json").write_bytes(render(manifest, "json"))


def verify(args):
    manifest = json.loads((args.directory / "manifest.json").read_text())
    if args.rpc:
        require(rpc(args.rpc, "chain_getBlockHash", [0]) == manifest["genesisHash"], "wrong genesis")
    for name, entry in manifest["files"].items():
        data = (args.directory / name).read_bytes()
        require(len(data) == entry["bytes"] and hashlib.sha256(data).hexdigest() == entry["sha256"], f"file drift: {name}")
        if args.rpc:
            observed = render(rpc(args.rpc, entry["method"], entry["params"]), entry["encoding"])
            require(observed == data, f"RPC mismatch: {name}")
    for row in manifest["blocks"]:
        block = json.loads((args.directory / row["blockFile"]).read_text())["block"]
        require(block["header"]["parentHash"] == row["parentHash"] == row["metadataStateHash"], "wrong execution metadata state")
        require(manifest["files"][row["metadataFile"]]["params"] == [row["parentHash"]], "metadata query must use the parent state")
        parent_version = json.loads((args.directory / f"runtime-parent-{row['number']}.json").read_text())
        post_version = json.loads((args.directory / f"runtime-post-{row['number']}.json").read_text())
        require((row["executionSpecVersion"], row["transactionVersion"]) == (parent_version["specVersion"], parent_version["transactionVersion"]), "execution version mismatch")
        require(row["postStateSpecVersion"] == post_version["specVersion"], "post-state version mismatch")
        if manifest["kind"] == "local-non-benchmark":
            require(all(api[0] != BENCHMARK_API_ID for api in parent_version["apis"]), "runtime advertises Benchmark API")
            require(manifest["buildArtifact"]["sha256"] == row["executionCodeSha256"], "runtime/build artifact mismatch")
        require(wire_expectations(block["extrinsics"]) == row["extrinsics"], "wire expectations drifted")
        if args.rpc:
            require(rpc(args.rpc, "chain_getBlockHash", [row["number"]]) == row["hash"], "wrong canonical block")
            require(rpc(args.rpc, "state_getStorageHash", [CODE_KEY, row["parentHash"]]) == row["executionCodeStorageHash"], "runtime code mismatch")
    print(f"verified {len(manifest['blocks'])} blocks and {len(manifest['files'])} files" + (" against RPC" if args.rpc else " offline"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ("capture", "verify"):
        p = sub.add_parser(command)
        p.add_argument("directory", type=Path)
        p.add_argument("--rpc", required=command == "capture")
        if command == "capture":
            p.add_argument("--blocks", nargs="+", type=int, required=True)
            p.add_argument("--kind", choices=("archived-live", "local-non-benchmark"), required=True)
            p.add_argument("--source-commit")
            p.add_argument("--build-features", nargs="*")
            p.add_argument("--wasm", type=Path)
    args = parser.parse_args()
    (capture if args.command == "capture" else verify)(args)


if __name__ == "__main__":
    main()
