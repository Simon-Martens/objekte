import json
import sys
from pathlib import Path


data = Path(sys.argv[1]) / "data"
data.mkdir()
with (data / "records.jsonl").open("w", encoding="utf-8") as output:
    for i in range(32):
        record = {
            "manual": "m",
            "blocked": "x",
            "group": ["A", "B"][i % 2],
            "enabled": i % 2 == 0,
            "sequence": i,
        }
        print(json.dumps(record), file=output)
