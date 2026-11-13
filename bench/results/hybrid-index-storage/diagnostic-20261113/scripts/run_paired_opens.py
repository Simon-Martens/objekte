import hashlib
import json
import os
import statistics
import subprocess
from pathlib import Path

root = Path.cwd()
out = root / 'target/hybrid-index-storage/paired-opens-final-20261113'
out.mkdir(parents=True, exist_ok=True)
base_program = root / 'target/hybrid-index-storage/baseline/bench_index_construction-da8fde6847'
new_program = root / 'target/release/examples/bench_musenalm'
snapshot = root / 'target/hybrid-index-storage/baseline/fixture/trial-1794575018344972131-413663/index.snapshot'
store_base = root / 'target/hybrid-index-storage/stores-smoke'
records = []
for round_number in range(7):
    modes = ['baseline', 'ondemand', 'resident']
    order = modes[round_number % 3:] + modes[:round_number % 3]
    for mode in order:
        result_dir = out / f'{round_number + 1}-{mode}'
        if mode == 'baseline':
            command = [str(base_program), '--root', 'examples/musenalm', '--result-dir', str(result_dir), '--alphabet', 'ascii', '--workers', '32', '--incremental', '--restore-snapshot', str(snapshot)]
        else:
            command = [str(new_program), '--root', 'examples/musenalm', '--result-dir', str(result_dir), '--extended', '--incremental', '--storage', 'custom', str(store_base), '--profiles', 'ascii-off', '--cases', 'Q001', '--verified-cache-bytes', '0', '--repetitions', '1']
            if mode == 'resident':
                command.append('--fully-resident')
        run = subprocess.run(command, text=True, capture_output=True)
        result_dir.mkdir(exist_ok=True)
        (result_dir / 'stdout.log').write_text(run.stdout)
        (result_dir / 'stderr.log').write_text(run.stderr)
        if run.returncode:
            raise RuntimeError(f'{mode} round {round_number + 1}: exit {run.returncode}: {run.stderr[-1000:]}')
        if mode == 'baseline':
            samples = list(result_dir.glob('trial-*/sample.json'))
            if len(samples) != 1:
                raise RuntimeError(f'baseline round {round_number + 1}: expected one sample, got {samples}')
            sample = json.loads(samples[0].read_text())
            open_ns = sample['load_ns']
            rss = sample['after_load_rss']
        else:
            sample = json.loads((result_dir / 'index-results.json').read_text())[0]
            open_ns = sample['open_ns']
            rss = sample['resident_bytes']
            if sample['source_reads'] != 0 or sample['build_parses'] != 0:
                raise RuntimeError(f'{mode} round {round_number + 1}: performed source build')
        records.append({'round': round_number + 1, 'mode': mode, 'command': command, 'open_ns': open_ns, 'rss': rss})
        print(f'{round_number + 1} {mode}: {open_ns/1e6:.2f} ms', flush=True)
manifest = {'records': records, 'affinity': sorted(os.sched_getaffinity(0)), 'binary_sha256': {'baseline': hashlib.sha256(base_program.read_bytes()).hexdigest(), 'candidate': hashlib.sha256(new_program.read_bytes()).hexdigest()}, 'store_base': str(store_base), 'baseline_snapshot': str(snapshot)}
(out / 'trials.json').write_text(json.dumps(manifest, indent=2) + '\n')
for mode in ['baseline', 'ondemand', 'resident']:
    values = [r['open_ns']/1e6 for r in records if r['mode'] == mode]
    print(f'{mode} median={statistics.median(values):.2f} ms samples={values}', flush=True)
