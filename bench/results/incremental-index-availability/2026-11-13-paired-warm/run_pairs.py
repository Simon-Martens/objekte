#!/usr/bin/env python3
"""Alternate fresh-process Musenalm construction runs on one immutable corpus root."""
import argparse
import json
import statistics
import subprocess
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('--baseline-bin', type=Path, required=True)
p.add_argument('--candidate-bin', type=Path, required=True)
p.add_argument('--root', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--pairs', type=int, default=7)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=False)
root = a.root.resolve(strict=True)
records = []
for pair in range(a.pairs):
    order = ('baseline', 'candidate') if pair % 2 == 0 else ('candidate', 'baseline')
    for position, mode in enumerate(order):
        binary = (a.baseline_bin if mode == 'baseline' else a.candidate_bin).resolve(strict=True)
        result_root = a.output / f'{pair + 1:02d}-{position + 1}-{mode}'
        cmd = [str(binary), '--root', str(root), '--result-dir', str(result_root),
               '--alphabet', 'ascii', '--workers', '32', '--profile', '--snapshot', '--check-queries']
        if mode == 'candidate':
            cmd.append('--incremental')
        run = subprocess.run(cmd, cwd=binary.parents[3], capture_output=True, text=True)
        if run.returncode:
            (a.output / 'failure.json').write_text(json.dumps({'pair': pair + 1, 'mode': mode,
                'command': cmd, 'returncode': run.returncode, 'stdout': run.stdout,
                'stderr': run.stderr}, indent=2))
            raise SystemExit(f'failed pair {pair + 1} {mode}: {run.stderr[-1000:]}')
        output = json.loads(run.stdout.strip().splitlines()[-1])
        artifact = Path(output['result_dir'])
        sample = json.loads((artifact / 'sample.json').read_text())
        manifest = json.loads((artifact / 'run-manifest.json').read_text())
        row = {'pair': pair + 1, 'position': position + 1, 'mode': mode,
               'command': cmd, 'artifact': str(artifact), 'sample': sample,
               'manifest': manifest}
        records.append(row)
        (a.output / 'runs.json').write_text(json.dumps(records, indent=2))
        # The measured byte count remains in sample.json; retain compact raw
        # measurements while avoiding fourteen 100+ MB cache payloads.
        (artifact / 'index.snapshot').unlink(missing_ok=True)
        print(f"pair {pair + 1} {mode}: {sample['build_ns']/1e6:.3f} ms, "
              f"HWM {sample['after_build_hwm']/1e6:.3f} MB, "
              f"live {sample['estimated_index_bytes']/1e6:.3f} MB", flush=True)

def summary(values):
    q1, _, q3 = statistics.quantiles(values, n=4, method='inclusive')
    return {'median': statistics.median(values), 'q1': q1, 'q3': q3, 'iqr': q3-q1}
metrics = ('build_ns', 'discovery_ns', 'after_build_hwm', 'after_build_rss',
           'estimated_index_bytes', 'build_parses')
report = {}
for metric in metrics:
    report[metric] = {mode: summary([r['sample'][metric] for r in records if r['mode'] == mode])
                      for mode in ('baseline', 'candidate')}
    report[metric]['candidate_over_baseline'] = (
        report[metric]['candidate']['median'] / report[metric]['baseline']['median'] - 1)
(a.output / 'summary.json').write_text(json.dumps(report, indent=2))
print(json.dumps(report, indent=2))
