#!/usr/bin/env python3
"""Create a deterministic machine-readable HomeBot release artifact manifest."""

import argparse
import hashlib
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--artifact", required=True, type=Path)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--architecture", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument(
        "--signing",
        required=True,
        choices=("unsigned", "adhoc", "developer-id", "package", "android-release", "ci-ephemeral"),
    )
    parser.add_argument("--protocol-minimum", type=int, default=1)
    parser.add_argument("--protocol-maximum", type=int, default=1)
    args = parser.parse_args()

    payload = args.artifact.read_bytes()
    manifest = {
        "schema_version": 1,
        "product": "HomeBot",
        "version": args.version,
        "platform": args.platform,
        "architecture": args.architecture,
        "artifact": args.artifact.name,
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
        "signing": args.signing,
        "protocol_minimum": args.protocol_minimum,
        "protocol_maximum": args.protocol_maximum,
    }
    args.output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
