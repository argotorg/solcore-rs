#!/usr/bin/env python3
"""Generate the shared Solcore Standard JSON suite for tofu's solc-bench."""

import argparse
import json
from pathlib import Path
import shutil


HERE = Path(__file__).resolve().parent
REPOSITORY = HERE.parents[1]
CASES = {
    "std-free": {
        "main.solc": REPOSITORY
        / "crates/parser/tests/fixtures/corpus/ok/test/examples/cases/SingleFun.solc",
    },
    "dispatch-small": {
        "main.solc": REPOSITORY / "tests/e2e/022add/main.solc",
    },
    "erc20-large": {
        "main.solc": REPOSITORY / "tests/e2e/128minierc20/main.solc",
    },
    "multi-file": {
        "main.solc": REPOSITORY / "tests/e2e/ltimp/main.solc",
        "ltproxy.solc": REPOSITORY / "tests/e2e/ltimp/ltproxy.solc",
    },
}


def standard_json(sources):
    return {
        "language": "Solcore",
        "sources": {
            name: {"content": path.read_text(encoding="utf-8")}
            for name, path in sources.items()
        },
        "settings": {
            "solcore": {
                "entrypoint": "main.solc",
                "stage": "hull",
            },
            "outputSelection": {"*": {"*": []}},
        },
    }


def main():
    parser = argparse.ArgumentParser(
        description="generate a Solcore suite consumable by tofu's solc-bench"
    )
    parser.add_argument("output_dir", type=Path)
    arguments = parser.parse_args()

    arguments.output_dir.mkdir(parents=True, exist_ok=True)
    shutil.copy2(HERE / "benchmarks.toml", arguments.output_dir / "benchmarks.toml")
    for name, sources in CASES.items():
        with (arguments.output_dir / f"{name}.json").open("w", encoding="utf-8") as output:
            json.dump(standard_json(sources), output, indent=2, sort_keys=True)
            output.write("\n")


if __name__ == "__main__":
    main()
