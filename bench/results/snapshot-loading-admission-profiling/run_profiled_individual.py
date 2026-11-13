#!/usr/bin/env python3
"""Three additional checked-load profiles for each individual change."""

from run_campaign import run

for round_number in range(1, 4):
    for variant in (("b1", "b2") if round_number % 2 else ("b2", "b1")):
        run("restore", variant, round_number, profiled=True)
