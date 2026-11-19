#!/usr/bin/env python3
"""Check the maintained OpenAPI path/method inventory against daemon routes."""
import json
import re
from pathlib import Path

root = Path(__file__).resolve().parent.parent
source = (root / "crates/argus-cli/src/main.rs").read_text()
actual = set()
for match in re.finditer(r'\.route\(\s*"(/api[^"\n]*)"\s*,', source):
    position, depth = match.end(), 1
    while position < len(source) and depth:
        depth += (source[position] == "(") - (source[position] == ")")
        position += 1
    for method in re.findall(r'\b(get|post|put|patch|delete)\(', source[match.end():position - 1]):
        actual.add((match[1][4:], method))

contract = json.loads((root / "openapi/argus.json").read_text())
declared = {(path, method) for path, methods in contract["paths"].items()
            for method in methods if method in {"get", "post", "put", "patch", "delete"}}
if not actual:
    raise SystemExit("No API routes found; update the inventory checker for the new router structure")
if actual != declared:
    raise SystemExit(f"OpenAPI route drift: missing={sorted(actual - declared)}, stale={sorted(declared - actual)}")
print(f"OpenAPI inventory matches all {len(actual)} daemon API operations")
