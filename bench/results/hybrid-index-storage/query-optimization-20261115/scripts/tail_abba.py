#!/usr/bin/env python3
"""Run fixed query tails in fresh baseline/candidate/candidate/baseline processes."""
import argparse
import csv
import gzip
import hashlib
import json
import statistics
import subprocess
from pathlib import Path

CASES = ('Q006','Q007','Q025','Q026','Q167','Q174','Q175','Q177','Q187','Q202','Q203','Q204')
PROFILES = ('ascii-off','unicode-off')
REPETITIONS = 30


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser()
    for name in ('baseline','candidate','root','output'):
        parser.add_argument('--'+name, type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    runs = [('A',args.baseline),('B',args.candidate),('B',args.candidate),('A',args.baseline)]
    manifest = {'order': 'ABBA', 'cases': CASES, 'profiles': PROFILES,
                'repetitions_per_run': REPETITIONS,
                'binary_sha256': {'A':sha256(args.baseline),'B':sha256(args.candidate)},
                'fixture_root':str(args.root), 'runs':[]}
    all_samples = {}
    reference_digest = None
    for position, (label, program) in enumerate(runs):
        folder = args.output / f'{position}-{label}'
        command = [str(program),'--root',str(args.root),'--extended','--incremental',
                   '--profiles',','.join(PROFILES),'--cases',','.join(CASES),
                   '--repetitions',str(REPETITIONS),'--verified-cache-bytes','0',
                   '--prepared-cache-bytes','16777216','--query-workers','32',
                   '--result-dir',str(folder)]
        print(f'Running {position}-{label}',flush=True)
        result = subprocess.run(command,capture_output=True,text=True)
        folder.mkdir(exist_ok=True)
        (folder/'command.json').write_text(json.dumps(command,indent=2)+'\n')
        (folder/'stdout.log').write_text(result.stdout)
        (folder/'stderr.log').write_text(result.stderr)
        if result.returncode:
            raise RuntimeError(f'{position}-{label}: {result.stderr[-2000:]}')
        digests = json.loads((folder/'result-digests.json').read_text())
        if reference_digest is None:
            reference_digest = digests
        elif digests != reference_digest:
            raise RuntimeError(f'{position}-{label}: result digest mismatch')
        samples = [json.loads(line) for line in (folder/'measurements.jsonl').read_text().splitlines()]
        if len(samples) != len(CASES)*len(PROFILES)*REPETITIONS:
            raise RuntimeError(f'{position}-{label}: wrong sample count')
        if any(row['status'] != 'ok' or row['counters']['verified_result_hit'] for row in samples):
            raise RuntimeError(f'{position}-{label}: failed query or verified cache hit')
        all_samples[position] = samples
        with gzip.open(folder/'measurements.jsonl.gz','wt') as zipped:
            for sample in samples:
                zipped.write(json.dumps(sample,separators=(',',':'))+'\n')
        (folder/'measurements.jsonl').unlink()
        manifest['runs'].append({'label':label,'position':position,'command':command,
                                 'manifest':json.loads((folder/'run-manifest.json').read_text())})
        print(f'Completed {position}-{label}: {len(samples)} samples',flush=True)
    summary = []
    for case in CASES:
        for profile in PROFILES:
            data = {}
            for label, positions in [('A',(0,3)),('B',(1,2))]:
                times = [row['total_ns'] for position in positions for row in all_samples[position]
                         if row['case_id']==case and row['profile']['alphabet']==profile.split('-')[0]]
                data[label] = statistics.median(times)
            summary.append({'case':case,'profile':profile,'baseline_median_ns':data['A'],
                            'candidate_median_ns':data['B'],'ratio':data['B']/data['A']})
    manifest['digest_parity'] = True
    manifest['summary'] = summary
    (args.output/'summary.json').write_text(json.dumps(manifest,indent=2)+'\n')
    print('All ABBA digests match',flush=True)

if __name__ == '__main__':
    main()
