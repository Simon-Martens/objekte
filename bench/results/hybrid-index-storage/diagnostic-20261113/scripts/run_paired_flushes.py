import hashlib
import json
import os
import shutil
import statistics
import subprocess
from pathlib import Path

root = Path.cwd()
out = root / 'target/hybrid-index-storage/paired-flushes-final-20261113'
out.mkdir(parents=True, exist_ok=True)
programs = {
    'baseline': root / 'target/hybrid-index-storage/baseline/bench_index_construction-da8fde6847',
    'candidate': root / 'target/release/examples/bench_managed_flush',
}
records = []
for round_number in range(7):
    order = ['baseline', 'candidate'] if round_number % 2 == 0 else ['candidate', 'baseline']
    for mode in order:
        folder = out / f'{round_number + 1}-{mode}'
        folder.mkdir(exist_ok=True)
        if mode == 'baseline':
            cmd = [str(programs[mode]), '--root', 'examples/musenalm', '--result-dir', str(folder), '--alphabet', 'ascii', '--workers', '32', '--incremental', '--snapshot']
        else:
            store_base = folder / 'store'
            cmd = [str(programs[mode]), '--root', 'examples/musenalm', '--output', str(store_base), '--alphabet', 'ascii']
        run = subprocess.run(cmd, text=True, capture_output=True)
        (folder / 'stdout.log').write_text(run.stdout)
        (folder / 'stderr.log').write_text(run.stderr)
        if run.returncode:
            raise RuntimeError(f'{mode} round {round_number + 1}: {run.stderr[-1500:]}')
        if mode == 'baseline':
            samples = list(folder.glob('trial-*/sample.json'))
            if len(samples) != 1:
                raise RuntimeError(f'expected one baseline sample, got {samples}')
            sample = json.loads(samples[0].read_text())
            stats = sample['snapshot_result']
            elapsed = stats['save_call_ns']
            size = stats['bytes']
            (samples[0].parent / 'index.snapshot').unlink()
        else:
            sample = json.loads(run.stdout)
            elapsed = sample['flush_ns']
            managed = Path(sample['managed_directory'])
            size = sum(f.stat().st_size for f in managed.rglob('*') if f.is_file())
            if sample['durable_generation'] != sample['revision']:
                raise RuntimeError('durable generation mismatch')
            shutil.rmtree(store_base)
        records.append({'round': round_number + 1, 'mode': mode, 'command': cmd, 'flush_ns': elapsed, 'stored_bytes': size})
        print(f'{round_number + 1} {mode}: {elapsed/1e6:.2f} ms, {size/1e6:.2f} MB', flush=True)
manifest = {'records': records, 'affinity': sorted(os.sched_getaffinity(0)), 'binary_sha256': {label: hashlib.sha256(path.read_bytes()).hexdigest() for label, path in programs.items()}}
(out / 'trials.json').write_text(json.dumps(manifest, indent=2) + '\n')
for mode in programs:
    values = [r['flush_ns']/1e6 for r in records if r['mode'] == mode]
    print(f'{mode} median={statistics.median(values):.2f} ms samples={values}', flush=True)
