"""Check distinct PRD line and branch targets; do not confuse the two."""

import json
import sys


def main() -> None:
    data = json.load(open(sys.argv[1]))
    line = data["totals"]["percent_statements_covered"]
    summaries = [
        value["summary"]
        for name, value in data["files"].items()
        if "/core/" in name or "/protocol/" in name
    ]
    branches = sum(value["covered_branches"] for value in summaries)
    total = sum(value["num_branches"] for value in summaries)
    branch = branches / total * 100
    print(f"Production line coverage: {line:.2f}% (target 85%).")
    print(f"Protocol/core branch coverage: {branch:.2f}% (target 90%).")
    assert line >= 85 and branch >= 90, "Coverage requirements not satisfied"


if __name__ == "__main__":
    main()
