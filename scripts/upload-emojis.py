#!/usr/bin/env python3
"""Manage the bot's custom application emojis without rebuilding anything.

Usually you don't need this: set EMOJI_AUTO_UPLOAD=1 once and the bot uploads
the bundled PNGs itself at startup, then resolves them by name forever after.

This script is the explicit alternative. It reads `emojis.toml`, uploads every
PNG that has a `file = ...` entry into YOUR application's emoji slots, and
prints the resulting IDs. It is idempotent: emojis that already exist (matched
by name) are skipped. Standard library only — no pip install.

Usage:
    export DISCORD_TOKEN='Bot xxxxx'
    export APPLICATION_ID=123456789012345678     # dev portal → General Information
    python3 scripts/upload-emojis.py             # upload missing icons
    python3 scripts/upload-emojis.py --list      # just show what's in the library
    python3 scripts/upload-emojis.py --dry-run   # show what would be uploaded
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import sys
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOML = ROOT / "emojis.toml"
EMOJI_DIR = ROOT / "assets" / "emojis"
API = "https://discord.com/api/v10"
MAX_BYTES = 256 * 1024  # Discord application emoji limit


def api(method: str, path: str, token: str, payload: dict | None = None):
    body = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(
        f"{API}{path}",
        data=body,
        method=method,
        headers={
            "Authorization": token if token.startswith("Bot ") else f"Bot {token}",
            "Content-Type": "application/json",
            "User-Agent": "nexcord-emoji-manager/1.0",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode(errors="replace")[:400]
        raise SystemExit(f"Discord returned {exc.code} for {method} {path}: {detail}") from exc
    except urllib.error.URLError as exc:
        raise SystemExit(f"Could not reach Discord ({exc.reason}). Check your network.") from exc


def load_slots() -> list[tuple[str, str, Path]]:
    """Return (name, fallback, png path) for every slot that has a bundled icon."""
    with TOML.open("rb") as handle:
        cfg = tomllib.load(handle)
    slots: list[tuple[str, str, Path]] = []
    seen: set[str] = set()
    for key, spec in cfg["emojis"].items():
        filename = spec.get("file")
        name = spec["name"]
        if not filename or name in seen:
            continue  # aliases (play / playing) share one upload
        seen.add(name)
        path = EMOJI_DIR / filename
        if not path.is_file():
            raise SystemExit(f"emojis.toml slot '{key}' references missing file {path}")
        if path.stat().st_size > MAX_BYTES:
            raise SystemExit(f"{path.name} is larger than Discord's {MAX_BYTES // 1024} KB limit")
        slots.append((name, spec["fallback"], path))
    return slots


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="only list existing application emojis")
    parser.add_argument("--dry-run", action="store_true", help="show the plan without uploading")
    parser.add_argument("--app-id", default=os.environ.get("APPLICATION_ID", ""))
    args = parser.parse_args()

    token = os.environ.get("DISCORD_TOKEN", "").strip()
    if not token:
        raise SystemExit("Set DISCORD_TOKEN first (see .env.example).")
    if not args.app_id:
        raise SystemExit("Set APPLICATION_ID (Developer Portal → General Information).")

    existing = api("GET", f"/applications/{args.app_id}/emojis", token)
    by_name = {e["name"]: e["id"] for e in existing.get("items", [])}
    print(f"Application emoji library: {len(by_name)} emoji(s)")

    if args.list:
        for name, eid in sorted(by_name.items()):
            print(f"  {name:24s} {eid}")
        return 0

    slots = load_slots()
    created: list[tuple[str, str]] = []
    for name, fallback, path in slots:
        if name in by_name:
            print(f"  = {name:24s} already present ({by_name[name]})")
            continue
        print(f"  + {name:24s} would upload {path.name} (fallback {fallback})")
        if args.dry_run:
            continue
        encoded = base64.b64encode(path.read_bytes()).decode()
        emoji = api(
            "POST",
            f"/applications/{args.app_id}/emojis",
            token,
            {"name": name, "image": f"data:image/png;base64,{encoded}"},
        )
        created.append((name, emoji["id"]))
        print(f"    -> {emoji['id']}")

    if created and not args.dry_run:
        print("\nOptional: pin these IDs in emojis.toml (normally not needed;")
        print("the bot resolves them by name automatically):")
        for name, eid in created:
            print(f'  # {name}: id = {eid}')
    elif not args.dry_run:
        print("\nNothing to do — every bundled icon already exists.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
