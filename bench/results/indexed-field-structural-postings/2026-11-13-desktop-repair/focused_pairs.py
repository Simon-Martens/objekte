#!/usr/bin/env python3
"""Alternating exact-starting-commit query latency recheck."""
import argparse
import csv
import json
import statistics
import subprocess
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('before', type=Path)
p.add_argument('after', type=Path)
p.add_argument('root', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
cases = 'Q001,Q052,Q069,Q112,Q116,Q117,Q167,Q168,Q174,Q175,Q177,Q189,Q195,Q196,Q214'
runs = []
for pair in range(3):
    for label in (('before','after') if pair % 2 == 0 else ('after','before')):
        binary = a.before if label == 'before' else a.after
        directory = a.output / f'{pair+1:02}-{label}'
        cmd = [str(binary), '--root', str(a.root), '--extended', '--cases', cases,
               '--profiles', 'ascii-off,unicode-off', '--repetitions', '7',
               '--query-workers', '4', '--verified-cache-bytes', '67108864',
               '--result-dir', str(directory)]
        result = subprocess.run(cmd, text=True, capture_output=True)
        directory.mkdir(parents=True, exist_ok=True)
        (directory / 'process.log').write_text(result.stdout + result.stderr)
        if result.returncode:
            raise SystemExit(f'{label} pair {pair+1} failed: {result.returncode}')
        with (directory / 'query-results.csv').open() as stream:
            for row in csv.DictReader(stream):
                runs.append({'pair':pair+1,'revision':label,'case':row['case_id'],'profile':row['profile'],
                             'median_ns':int(row['median_ns']),'status':row['status']})
        print(pair+1,label,'complete',flush=True)
(a.output / 'measurements.json').write_text(json.dumps(runs, indent=2) + '\n')
summary = []
for case in cases.split(','):
    for profile in ('ascii-off','unicode-off'):
        before = [r['median_ns'] for r in runs if r['case']==case and r['profile']==profile and r['revision']=='before']
        after = [r['median_ns'] for r in runs if r['case']==case and r['profile']==profile and r['revision']=='after']
        old,new = statistics.median(before),statistics.median(after)
        summary.append({'case':case,'profile':profile,'before_ms':old/1e6,'after_ms':new/1e6,
                        'delta_ms':(new-old)/1e6,'delta_percent':(new/old-1)*100 if old else None,
                        'material_regression':new-old>500000 and new>old*1.1})
(a.output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print('material regressions',[(s['case'],s['profile']) for s in summary if s['material_regression']])
