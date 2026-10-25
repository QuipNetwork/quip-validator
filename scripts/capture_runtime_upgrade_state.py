#!/usr/bin/env python3
"""Capture all top-level and default child-trie state at one finalized block.

Read-only RPC; standard library only. The offline runtime_upgrade test checks
the reconstructed trie root against the header, so this capture alone is not
proof of completeness. Keep the large output outside the repository.
"""

import argparse
import datetime
import hashlib
import json
from pathlib import Path
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--at", required=True, help="Finalized block hash")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists; choose a new capture path")

    def rpc(method, params):
        request = urllib.request.Request(
            args.endpoint,
            data=json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params)).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=120) as response:
            body = json.load(response)
        if "error" in body:
            raise RuntimeError(f"{method}: {body['error']}")
        return body["result"]

    header = rpc("chain_getHeader", [args.at])
    number = int(header["number"], 16)
    finalized = rpc("chain_getFinalizedHead", [])
    finalized_header = rpc("chain_getHeader", [finalized])
    if number > int(finalized_header["number"], 16):
        raise RuntimeError("requested block is newer than the finalized head")
    if rpc("chain_getBlockHash", [number]) != args.at:
        raise RuntimeError("requested hash is not canonical at its height")

    def capture(child=None):
        entries, start = [], None
        while True:
            if child is None:
                keys = rpc("state_getKeysPaged", ["0x", 500, start, args.at])
            else:
                keys = rpc("childstate_getKeysPaged", [child, "0x", 500, start, args.at])
            if not keys:
                return entries
            if keys != sorted(set(keys)) or (start is not None and keys[0] <= start):
                raise RuntimeError("non-monotonic or duplicate paged keys")
            if child is None:
                changes = rpc("state_queryStorageAt", [keys, args.at])
                if len(changes) != 1 or changes[0]["block"] != args.at:
                    raise RuntimeError("unexpected state_queryStorageAt block")
                pairs = changes[0]["changes"]
                if len(pairs) != len(keys) or {k for k, _ in pairs} != set(keys):
                    raise RuntimeError("storage response omitted or duplicated keys")
                values = dict(pairs)
                page = [(key, values[key]) for key in keys]
            else:
                values = rpc("childstate_getStorageEntries", [child, keys, args.at])
                if len(values) != len(keys):
                    raise RuntimeError("child storage response length mismatch")
                page = list(zip(keys, values))
            if any(value is None for _, value in page):
                raise RuntimeError("listed key has no value at the pinned block")
            entries.extend(page)
            start = keys[-1]
            print(f"{child or 'top'}: {len(entries)} keys", flush=True)

    top = capture()
    children = []
    for key, _ in top:
        raw = bytes.fromhex(key[2:])
        if raw.startswith(b":child_storage:"):
            if not raw.startswith(b":child_storage:default:"):
                raise RuntimeError("unsupported child trie kind")
            children.append(dict(key=key, entries=capture(key)))
    snapshot = dict(
        captured_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
        endpoint=args.endpoint,
        block_hash=args.at,
        header=header,
        genesis_hash=rpc("chain_getBlockHash", [0]),
        runtime_version=rpc("state_getRuntimeVersion", [args.at]),
        top=top,
        children=children,
    )
    data = (json.dumps(snapshot, separators=(",", ":")) + "\n").encode()
    with args.output.open("xb") as output:
        output.write(data)
    print(f"wrote {len(data)} bytes; sha256={hashlib.sha256(data).hexdigest()}")
    print(f"block={number}; top={len(top)}; children={len(children)}")


if __name__ == "__main__":
    main()
