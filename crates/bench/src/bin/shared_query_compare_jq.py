"""Fresh-process jq comparison for the shared NDJSON workload.

Each jq invocation includes process startup, parsing and query execution. This
is deliberately reported separately from the resident-engine warm timings.
"""
import argparse
import json
import statistics
import subprocess
import tempfile
import time


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--input', required=True)
    p.add_argument('--number-field', required=True)
    p.add_argument('--text-field', required=True)
    p.add_argument('--lower', type=int, required=True)
    p.add_argument('--upper', type=int, required=True)
    p.add_argument('--needle', required=True)
    p.add_argument('--runs', type=int, default=3)
    a = p.parse_args()
    n = '.data[' + json.dumps(a.number_field) + ']'
    t = '.data[' + json.dumps(a.text_field) + ']'
    first = '.[0].data[' + json.dumps(a.number_field) + ']'
    lo, hi = a.lower, a.upper
    cond = f'({n} | type == "number") and ({n} >= {lo} and {n} <= {hi})'
    programs = {
        'range_count': f'[.[] | select({cond})] | length',
        'range_page': f'[.[] | select({cond})] | sort_by({n},.id) | .[:50] | map([{n},.id])',
        'number_group': f'[.[] | select({n} | type == "number")] | group_by({n}) | map([{first},length])',
        'text_count': f'[.[] | select(({t} // "") | ascii_downcase | contains({json.dumps(a.needle.lower())}))] | length',
    }
    for name, program in programs.items():
        samples = []
        result = None
        with tempfile.NamedTemporaryFile() as rss_file:
            for _ in range(a.runs):
                start = time.perf_counter()
                run = subprocess.run(
                    ['/usr/bin/time', '-f', '%M', '-o', rss_file.name,
                     'jq', '--slurp', '--compact-output', program, a.input],
                    check=True, capture_output=True, text=True,
                )
                seconds = time.perf_counter() - start
                samples.append((seconds, int(open(rss_file.name).read().strip()) / 1024))
                result = json.loads(run.stdout)
        print(json.dumps({'metric': name, 'seconds': statistics.median(x[0] for x in samples),
                          'result': result, 'peak_rss_mib': max(x[1] for x in samples)}))


if __name__ == '__main__':
    main()
