#!/usr/bin/env python3
"""Checks the dependencies of workspace crates against dependency-policy.toml.

Run from the repository root: `python3 scripts/check-deps.py`.
Exit code 0 = all good, 1 = rule violations (printed).
"""

import json
import subprocess
import sys
import tomllib
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
POLICY_FILE = ROOT / "dependency-policy.toml"


def load_members():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    meta = json.loads(out)
    members = set(meta["workspace_members"])
    return [p for p in meta["packages"] if p["id"] in members]


def check(members, policy):
    errors = []
    crates = policy.get("crates", {})
    names = {p["name"] for p in members}

    for missing in sorted(names - crates.keys()):
        errors.append(f"{missing}: missing in {POLICY_FILE.name}")
    for extra in sorted(crates.keys() - names):
        errors.append(f"{extra}: in {POLICY_FILE.name} but not in the workspace")

    # Single-wrapper rule: an external crate may be allowed for one member only.
    owners = defaultdict(list)
    for crate, rules in crates.items():
        for dep in rules.get("external", []):
            owners[dep].append(crate)
    for dep, who in sorted(owners.items()):
        if len(who) > 1:
            errors.append(f"{dep}: only one wrapper may call it, allowed in {', '.join(sorted(who))}")

    test_only = {name for name, rules in crates.items() if rules.get("test_only")}

    for pkg in sorted(members, key=lambda p: p["name"]):
        rules = crates.get(pkg["name"])
        if rules is None:
            continue
        internal = set(rules.get("internal", []))
        external = set(rules.get("external", []))
        dev = set(rules.get("dev", []))
        for dep in pkg["dependencies"]:
            name = dep.get("rename") or dep["name"]
            is_internal = dep["name"] in names
            if dep["kind"] == "dev":
                allowed = dev | internal | external
            elif is_internal:
                allowed = internal
            else:
                allowed = external
            kind = dep["kind"] or "normal"
            if name not in allowed and dep["name"] not in allowed:
                errors.append(f"{pkg['name']}: dependency not allowed {dep['name']} ({kind})")
            elif dep["name"] in test_only and dep["kind"] != "dev":
                errors.append(f"{pkg['name']}: {dep['name']} is test-only, but a {kind} dependency")
    return errors


def main():
    with POLICY_FILE.open("rb") as f:
        policy = tomllib.load(f)
    errors = check(load_members(), policy)
    for e in errors:
        print(f"error: {e}", file=sys.stderr)
    if errors:
        return 1
    print("dependencies match dependency-policy.toml")
    return 0


if __name__ == "__main__":
    sys.exit(main())
