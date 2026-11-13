#!/usr/bin/env python3
"""Three paired Unicode restores and one shortcut profile."""

from run_campaign import BASE, run

snapshot = next((BASE / "unicode-fixture").glob("trial-*/index.snapshot"))
for round_number in range(1, 4):
    for variant in (("b0", "b3") if round_number % 2 else ("b3", "b0")):
        run("restore", variant, round_number, alphabet="unicode", snapshot=snapshot)
run("restore", "b3", 1, profiled=True, alphabet="unicode", snapshot=snapshot)
