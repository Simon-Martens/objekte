#!/usr/bin/env python3
"""Paired fresh-process construction comparison; candidate minus control."""
import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path


def revision(root):
    return subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()


def run(role, pair, warmup, args, directory):
    binary = args.control_bin if role == 'control' else args.candidate_bin
    cwd = args.control_repo if role == 'control' else args.candidate_repo
    trial_root = directory / ('warmup' if warmup else f'pair-{pair:02}') / role
    trial_root.mkdir(parents=True, exist_ok=False)
    command = [str(binary), '--root', str(args.root), '--alphabet', args.alphabet,
               '--result-dir', str(trial_root)]
    if args.workers != 'auto':
        command += ['--workers', args.workers]
    if args.profile:
        command.append('--profile')
    completed = subprocess.run(command, cwd=cwd, text=True, capture_output=True, timeout=600)
    (trial_root / 'process.stdout').write_text(completed.stdout)
    (trial_root / 'process.stderr').write_text(completed.stderr)
    if completed.returncode:
        raise RuntimeError(f'{role} pair {pair}: {completed.returncode}: {completed.stderr}')
    samples = list(trial_root.glob('trial-*/sample.json'))
    if len(samples) != 1:
        raise RuntimeError(f'expected one sample in {trial_root}, got {samples}')
    sample = json.loads(samples[0].read_text())
    manifest = json.loads((samples[0].parent / 'run-manifest.json').read_text())
    if manifest['code_revision'] != revision(cwd):
        raise RuntimeError(f'wrong revision in {samples[0]}')
    if manifest['dirty_status']:
        raise RuntimeError(f'dirty measured checkout {cwd}: {manifest["dirty_status"]}')
    if sample['failed_parses']:
        raise RuntimeError(f'failed parses in {samples[0]}')
    return {'role': role, 'pair': pair, 'warmup': warmup,
            'order': None, 'sample': str(samples[0]), 'build_ns': sample['build_ns'],
            'after_build_hwm': sample['after_build_hwm'],
            'after_build_rss': sample['after_build_rss'],
            'discovery_ns': sample['discovery_ns'],
            'extraction_index_ns': sample['extraction_index_ns'],
            'finalization_ns': sample['finalization_ns'],
            'build_admission_ns': sample['build_admission_ns'],
            'profile': sample['profile'],
            'identity': {key: sample[key] for key in
                         ['build_parses', 'postings', 'sort_fields', 'relation_edges',
                          'bigram_stores', 'distinct_gram_keys', 'estimated_index_bytes']},
            'corpus_fingerprint': manifest['corpus_fingerprint'],
            'schema_fingerprint': manifest['schema_fingerprint'],
            'revision': manifest['code_revision'], 'resolved_workers': manifest['resolved_workers']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--control-repo', type=Path, required=True)
    parser.add_argument('--candidate-repo', type=Path, required=True)
    parser.add_argument('--control-bin', type=Path, required=True)
    parser.add_argument('--candidate-bin', type=Path, required=True)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--alphabet', choices=['ascii', 'unicode'], required=True)
    parser.add_argument('--workers', default='auto')
    pair_mode = parser.add_mutually_exclusive_group(required=True)
    pair_mode.add_argument('--pairs', type=int, help='pairs in a new comparison')
    pair_mode.add_argument('--append-pairs', type=int, help='additional pairs in an existing comparison')
    parser.add_argument('--profile', action='store_true')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    for name in ['control_repo', 'candidate_repo', 'control_bin', 'candidate_bin', 'root', 'out']:
        setattr(args, name, getattr(args, name).resolve())
    appending = args.append_pairs is not None
    if (args.append_pairs if appending else args.pairs) < 1:
        parser.error('pair count must be positive')
    if not appending:
        args.out.mkdir(parents=True, exist_ok=False)
    config = {name: str(getattr(args, name)) for name in
              ['control_repo', 'candidate_repo', 'control_bin', 'candidate_bin', 'root']}
    config.update(alphabet=args.alphabet, workers=args.workers,
                  profile=args.profile, control_revision=revision(args.control_repo),
                  candidate_revision=revision(args.candidate_repo),
                  control_sha256=hashlib.file_digest(args.control_bin.open('rb'), 'sha256').hexdigest(),
                  candidate_sha256=hashlib.file_digest(args.candidate_bin.open('rb'), 'sha256').hexdigest())
    if appending:
        previous = json.loads((args.out / 'config.json').read_text())
        if {key: value for key, value in previous.items() if key != 'pairs'} != config:
            raise RuntimeError('append parameters, revisions, or binary hashes changed')
        start_pair = previous['pairs']
        total_pairs = start_pair + args.append_pairs
        rows = [json.loads(line) for line in (args.out / 'runs.jsonl').read_text().splitlines()]
        expected = {(-1, role, True) for role in ('control', 'candidate')}
        expected.update((pair, role, False) for pair in range(start_pair)
                        for role in ('control', 'candidate'))
        actual = [(row['pair'], row['role'], row['warmup']) for row in rows]
        if len(actual) != len(expected) or set(actual) != expected:
            raise RuntimeError('existing runs are incomplete or duplicated; refusing append')
        if not (args.out / 'summary.json').exists():
            raise RuntimeError('existing comparison has no completed summary')
    else:
        start_pair = 0
        total_pairs = args.pairs
        rows = []
    with (args.out / 'runs.jsonl').open('a' if appending else 'w') as output:
        if not appending:
            config['pairs'] = total_pairs
            (args.out / 'config.json').write_text(json.dumps(config, indent=2) + '\n')
            for role in ['control', 'candidate']:
                row = run(role, -1, True, args, args.out)
                rows.append(row)
                output.write(json.dumps(row) + '\n'); output.flush()
                print(f'warmup {role}: {row["build_ns"]/1e6:.3f} ms', flush=True)
        for pair in range(start_pair, total_pairs):
            order = ['control', 'candidate'] if pair % 2 == 0 else ['candidate', 'control']
            for role in order:
                row = run(role, pair, False, args, args.out)
                row['order'] = order
                rows.append(row)
                output.write(json.dumps(row) + '\n'); output.flush()
                print(f'pair {pair+1}/{total_pairs} {role}: {row["build_ns"]/1e6:.3f} ms', flush=True)
    config['pairs'] = total_pairs
    measured = [row for row in rows if not row['warmup']]
    identity = [(row['identity'], row['corpus_fingerprint'], row['schema_fingerprint'],
                 row['resolved_workers']) for row in measured]
    if any(value != identity[0] for value in identity):
        raise RuntimeError('build corpus/index counters differ between measurements')
    controls = [next(row for row in measured if row['pair'] == pair and row['role'] == 'control')
                for pair in range(total_pairs)]
    candidates = [next(row for row in measured if row['pair'] == pair and row['role'] == 'candidate')
                  for pair in range(total_pairs)]
    deltas = [b['build_ns'] - a['build_ns'] for a,b in zip(controls,candidates)]
    def summarize(seq):
        vals = list(seq)
        q = statistics.quantiles(vals, n=4, method='inclusive') if len(vals) >= 2 else [vals[0]]*3
        return {'median': statistics.median(vals), 'min': min(vals), 'max': max(vals),
                'iqr': q[2]-q[0]}
    summary = {'config': config, 'control_build_ns': summarize(r['build_ns'] for r in controls),
               'candidate_build_ns': summarize(r['build_ns'] for r in candidates),
               'paired_delta_ns': summarize(deltas),
               'paired_delta_percent': summarize(100*(b['build_ns']/a['build_ns']-1)
                                                   for a,b in zip(controls,candidates)),
               'paired_deltas_ns': deltas,
               'identity': measured[0]['identity'],
               'corpus_fingerprint': measured[0]['corpus_fingerprint'],
               'schema_fingerprint': measured[0]['schema_fingerprint'],
               'resolved_workers': measured[0]['resolved_workers']}
    (args.out / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    (args.out / 'config.json').write_text(json.dumps(config, indent=2) + '\n')
    print('paired median candidate-control:', f'{summary["paired_delta_ns"]["median"]/1e6:+.3f} ms',
          f'({summary["paired_delta_percent"]["median"]:+.3f}%)', flush=True)


if __name__ == '__main__':
    main()
