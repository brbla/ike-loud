#!/usr/bin/env python3
"""Ověří závislosti crates ve workspace proti dependency-policy.toml.

Spouští se z kořene repozitáře: `python3 scripts/check-deps.py`.
Návratový kód 0 = vše v pořádku, 1 = porušení pravidel (vypíše je).
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
        errors.append(f"{missing}: chybí v {POLICY_FILE.name}")
    for extra in sorted(crates.keys() - names):
        errors.append(f"{extra}: je v {POLICY_FILE.name}, ale ne ve workspace")

    # Pravidlo jednoho obalu: externí crate smí mít povolený jen jeden člen.
    owners = defaultdict(list)
    for crate, rules in crates.items():
        for dep in rules.get("external", []):
            owners[dep].append(crate)
    for dep, who in sorted(owners.items()):
        if len(who) > 1:
            errors.append(f"{dep}: smí ho volat jen jeden obal, povolen v {', '.join(sorted(who))}")

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
            if name not in allowed and dep["name"] not in allowed:
                kind = dep["kind"] or "normal"
                errors.append(f"{pkg['name']}: nepovolená závislost {dep['name']} ({kind})")
    return errors


def main():
    with POLICY_FILE.open("rb") as f:
        policy = tomllib.load(f)
    errors = check(load_members(), policy)
    for e in errors:
        print(f"chyba: {e}", file=sys.stderr)
    if errors:
        return 1
    print("závislosti odpovídají dependency-policy.toml")
    return 0


if __name__ == "__main__":
    sys.exit(main())
