#!/usr/bin/env python3
"""Checks that our own crates were compiled from the current sources.

Reads cargo's JSON messages on stdin, e.g.
    cargo test --no-run --message-format=json-render-diagnostics | scripts/check-fresh.py

Fails if any artifact of a workspace crate was reused ("fresh": true): our
code must never come from a cache. Rebuilt third-party dependencies are only
reported (a speed issue, not a correctness one); with --expect-deps-cached
they become a GitHub warning.
"""

import argparse
import json
import subprocess
import sys


def workspace_ids():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked", "--offline"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return set(json.loads(out)["workspace_members"])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--expect-deps-cached", action="store_true")
    args = parser.parse_args()

    ours = workspace_ids()
    reused_ours, rebuilt_deps, built_ours = [], [], set()
    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("reason") != "compiler-artifact":
            continue
        pkg = msg["package_id"]
        name = msg["target"]["name"]
        if pkg in ours:
            built_ours.add(pkg)
            if msg["fresh"]:
                reused_ours.append(f"{name} ({', '.join(msg['target']['kind'])})")
        elif not msg["fresh"]:
            rebuilt_deps.append(name)

    if not built_ours:
        print("error: no artifacts of our crates in the input", file=sys.stderr)
        return 1
    if reused_ours:
        print("error: our crates were taken from a cache, not compiled:", file=sys.stderr)
        for r in sorted(set(reused_ours)):
            print(f"  {r}", file=sys.stderr)
        return 1
    print(f"ok: {len(built_ours)} of our crates compiled from current sources")
    deps = sorted(set(rebuilt_deps))
    if deps:
        text = f"{len(deps)} dependencies were rebuilt: {', '.join(deps)}"
        if args.expect_deps_cached:
            print(f"::warning::dependency cache hit, but {text}")
        else:
            print(f"info: {text}")
    else:
        print("info: all dependencies came from the cache")
    return 0


if __name__ == "__main__":
    sys.exit(main())
