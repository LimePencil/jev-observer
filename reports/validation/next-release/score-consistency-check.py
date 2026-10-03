#!/usr/bin/env python3
"""Check the captured Score arithmetic offline; this does not invoke Observer.

Usage: python3 score-consistency-check.py [openrouter-live.json]
Reads the existing capture report and writes JSON to stdout. No network calls.
"""

import hashlib
import json
import pathlib
import sys
from decimal import Decimal, ROUND_HALF_UP


path = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path(__file__).with_name("openrouter-live.json")
content = path.read_bytes()
report = json.loads(content, parse_float=Decimal, parse_int=Decimal)
cases = []
successful_answer_count = 0
for call in report["calls"]:
    if call["status"] != 200:
        continue
    successful_answer_count += len(call["answers"])
    for key, answer in call["answers"].items():
        if answer["type"] != "score":
            continue
        weighted = sum(Decimal(level) * probability for level, probability in answer["probabilities"].items())
        difference = abs(answer["score"] - weighted)
        cases.append({
            "call": call["name"],
            "answer_key": key,
            "captured_answer": answer,
            "displayed_probability_sum": sum(answer["probabilities"].values()),
            "displayed_weighted_score": weighted,
            "absolute_difference": difference,
            "matches_current_weighted_check": difference <= Decimal("0.001"),
        })

# A hypothetical rounding witness: no claim that the provider used these values.
latent = [Decimal("0.1375"), Decimal("0.858"), Decimal("0.0045")]
latent_score = sum(Decimal(index) * probability for index, probability in enumerate(latent))
rounded = lambda value: value.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)
assert sum(latent) == 1
assert list(map(rounded, latent)) == [Decimal("0.14"), Decimal("0.86"), Decimal("0.00")]
assert rounded(latent_score) == Decimal("0.87")

print(json.dumps({
    "scope": "Offline exact-decimal check of captured values; does not execute the production validator",
    "source_report": path.name,
    "source_sha256": hashlib.sha256(content).hexdigest(),
    "source_binary_sha256": report["binary_sha256"],
    "provider_calls": 0,
    "successful_answer_count": successful_answer_count,
    "score_answer_count": len(cases),
    "weighted_check_mismatches": sum(not case["matches_current_weighted_check"] for case in cases),
    "cases": cases,
    "hypothetical_rounding_witness": {
        "status": "Synthetic possibility only; provider rounding is not established",
        "assumption": "All probabilities and the expected score are independently rounded to the nearest hundredth",
        "latent_probabilities": latent,
        "latent_expected_score": latent_score,
        "rounded_probabilities": list(map(rounded, latent)),
        "rounded_score": rounded(latent_score),
    },
    "number_encoding": "Decimal strings preserve the arithmetic exactly",
}, default=str, indent=2))
