#!/usr/bin/env python3
"""Convert an artifact manifest into a canonical Ed25519-signed update manifest."""

import argparse
import base64
import hashlib
import json
import subprocess
import tempfile
from pathlib import Path


FIELDS = (
    "schema_version",
    "product",
    "version",
    "platform",
    "architecture",
    "artifact",
    "bytes",
    "sha256",
    "signing",
    "protocol_minimum",
    "protocol_maximum",
    "key_id",
)


def run(*arguments: str, stdin: bytes | None = None) -> bytes:
    return subprocess.run(
        arguments,
        input=stdin,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=True,
    ).stdout


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--private-key", required=True, type=Path)
    args = parser.parse_args()

    manifest = json.loads(args.input.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 1 or manifest.get("signing") not in {
        "developer-id",
        "package",
        "android-release",
    }:
        raise SystemExit("only platform-signed release manifests can become update manifests")
    public_der = run(
        "openssl",
        "pkey",
        "-in",
        str(args.private_key),
        "-pubout",
        "-outform",
        "DER",
    )
    if len(public_der) < 32:
        raise SystemExit("invalid Ed25519 public key")
    public_key = public_der[-32:]
    manifest["schema_version"] = 2
    manifest["key_id"] = hashlib.sha256(public_key).hexdigest()[:16]
    canonical = {field: manifest[field] for field in FIELDS}
    payload = json.dumps(canonical, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    with tempfile.TemporaryDirectory(prefix="homebot-update-sign-") as temporary:
        payload_path = Path(temporary) / "manifest.payload"
        signature_path = Path(temporary) / "manifest.signature"
        payload_path.write_bytes(payload)
        run(
            "openssl",
            "pkeyutl",
            "-sign",
            "-rawin",
            "-inkey",
            str(args.private_key),
            "-in",
            str(payload_path),
            "-out",
            str(signature_path),
        )
        canonical["signature"] = base64.b64encode(signature_path.read_bytes()).decode("ascii")
    args.output.write_text(
        json.dumps(canonical, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
