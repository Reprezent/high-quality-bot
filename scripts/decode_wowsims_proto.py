#!/usr/bin/env python3
import argparse
import glob
import importlib
import json
import os
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from typing import Dict, List


def run(cmd: List[str]) -> None:
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(
            f"Command failed ({proc.returncode}): {' '.join(cmd)}\n"
            f"stdout:\n{proc.stdout}\n"
            f"stderr:\n{proc.stderr}"
        )


def generate_python_protos(proto_root: str, out_dir: str, protoc: str) -> None:
    proto_files = sorted(glob.glob(os.path.join(proto_root, "*.proto")))
    if not proto_files:
        raise RuntimeError(f"No .proto files found under {proto_root}")

    cmd = [
        protoc,
        f"-I={proto_root}",
        "-I=/usr/include",
        f"--python_out={out_dir}",
        *proto_files,
    ]
    run(cmd)


@dataclass
class DecodeHit:
    message_name: str
    offset: int
    consumed: int
    json_text: str


def candidate_offsets(data: bytes, max_scan: int) -> List[int]:
    offsets = {0}

    # Common log prefix separators when binary payload is printed after metadata.
    for sep in (b"\n", b"\r\n", b": "):
        idx = data.find(sep)
        if idx != -1 and idx + len(sep) < len(data):
            offsets.add(idx + len(sep))

    # Brute-force early offsets for prefixed payloads.
    for i in range(1, min(max_scan, len(data))):
        offsets.add(i)

    return sorted(offsets)


def payload_variants(data: bytes) -> List[bytes]:
    variants = [data]

    # Known worker prefix patterns like: raidSimAsync-<id>
    pattern = re.compile(rb"(?:raidSimAsync|statWeightsAsync)-[0-9a-fA-F-]+")
    match = pattern.search(data)
    if match:
        start = match.end()
        while start < len(data) and data[start] in (9, 10, 13, 32):
            start += 1
        if start < len(data):
            variants.append(data[start:])

    return variants


def decode_attempts(data: bytes, message_types: Dict[str, type], max_scan: int) -> List[DecodeHit]:
    from google.protobuf.json_format import MessageToJson
    from google.protobuf.message import DecodeError

    hits: List[DecodeHit] = []

    for variant in payload_variants(data):
        variant_base_offset = len(data) - len(variant)
        for offset in candidate_offsets(variant, max_scan):
            chunk = variant[offset:]
            if not chunk:
                continue

            for name, cls in message_types.items():
                msg = cls()
                try:
                    consumed = msg.MergeFromString(chunk)
                except DecodeError:
                    continue

                if consumed <= 0:
                    continue

                json_text = MessageToJson(msg, preserving_proto_field_name=True)
                if json_text.strip() in ("{}", "{\n}"):
                    continue

                hits.append(DecodeHit(name, variant_base_offset + offset, consumed, json_text))

    return hits


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Decode captured wowsims protobuf payloads into JSON."
    )
    parser.add_argument("payload", help="Path to file containing raw protobuf bytes")
    parser.add_argument(
        "--proto-root",
        default="vendor/wowsims-mop/proto",
        help="Path to wowsims proto directory",
    )
    parser.add_argument(
        "--message",
        choices=["auto", "RaidSimRequest", "ProgressMetrics", "AsyncAPIResult"],
        default="auto",
        help="Message type to decode",
    )
    parser.add_argument(
        "--protoc",
        default="protoc",
        help="Path to protoc binary",
    )
    parser.add_argument(
        "--max-scan",
        type=int,
        default=128,
        help="Max leading bytes to skip while searching for payload start",
    )
    parser.add_argument(
        "--all",
        action="store_true",
        help="Print all successful decode candidates",
    )
    args = parser.parse_args()

    if not os.path.exists(args.payload):
        print(f"Payload file not found: {args.payload}", file=sys.stderr)
        return 2

    with open(args.payload, "rb") as f:
        raw = f.read()

    if not raw:
        print("Payload file is empty", file=sys.stderr)
        return 2

    with tempfile.TemporaryDirectory(prefix="wowsims_pyproto_") as tmp:
        generate_python_protos(args.proto_root, tmp, args.protoc)
        sys.path.insert(0, tmp)
        api_pb2 = importlib.import_module("api_pb2")

        all_types: Dict[str, type] = {
            "RaidSimRequest": api_pb2.RaidSimRequest,
            "ProgressMetrics": api_pb2.ProgressMetrics,
            "AsyncAPIResult": api_pb2.AsyncAPIResult,
        }

        if args.message == "auto":
            target_types = all_types
        else:
            target_types = {args.message: all_types[args.message]}

        hits = decode_attempts(raw, target_types, args.max_scan)
        if not hits:
            print(
                "No decode candidates found. If your payload has a large text prefix, increase --max-scan.",
                file=sys.stderr,
            )
            return 1

        # Prefer smallest offset and the most bytes consumed; for ties prefer richer JSON.
        hits.sort(key=lambda h: (h.offset, -h.consumed, -len(h.json_text)))

        if args.all:
            for i, hit in enumerate(hits, start=1):
                print(
                    f"=== Candidate #{i}: {hit.message_name} "
                    f"(offset={hit.offset}, consumed={hit.consumed}) ==="
                )
                print(hit.json_text)
            return 0

        best = hits[0]
        print(
            json.dumps(
                {
                    "message": best.message_name,
                    "offset": best.offset,
                    "consumed": best.consumed,
                },
                indent=2,
            )
        )
        print(best.json_text)
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
