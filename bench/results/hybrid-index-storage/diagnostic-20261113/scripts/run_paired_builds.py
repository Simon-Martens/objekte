import hashlib
import json
import os
import statistics
import subprocess
from pathlib import Path

root = Path.cwd()
out = root / 'target/hybrid-index-storage/paired-builds-final-20261113'
out.mkdir(parents=True, exist_ok=True)
programs = {
    'baseline': root / 'target/hybrid-index-storage/baseline/bench_index_construction-da8fde6847',
    'candidate': root / 'target/release/examples/bench_index_construction',
}
records = []
for round_number in range(7):
    order = ['baseline', 'candidate'] if round_number % 2 == 0 else ['candidate', 'baseline']
    for label in order:
        result_dir = out / f'{round_number + 1}-{label}'
        command = [str(programs[label]), '--root', 'examples/musenalm', '--result-dir', str(result_dir), '--alphabet', 'ascii', '--workers', '32', '--incremental']
        run = subprocess.run(command, text=True, capture_output=True)
        result_dir.mkdir(exist_ok=True)
        (result_dir / 'stdout.log').write_text(run.stdout)
        (result_dir / 'stderr.log').write_text(run.stderr)
        if run.returncode:
            raise RuntimeError(f'{label} round {round_number + 1}: exit {run.returncode}: {run.stderr[-1000:]}')
        samples = list(result_dir.glob('trial-*/sample.json'))
        if len(samples) != 1:
            raise RuntimeError(f'{label} round {round_number + 1}: expected one sample, got {samples}')
        sample = json.loads(samples[0].read_text())
        record = {'round': round_number + 1, 'label': label, 'command': command, 'sample_path': str(samples[0].relative_to(root)), 'build_ns': sample['build_ns'], 'completion_ns': sample['completion_ns'], 'first_batch_ns': sample['first_batch_ns'], 'after_build_rss': sample['after_build_rss'], 'after_build_hwm': sample['after_build_hwm']}
        records.append(record)
        print(f"{round_number + 1} {label}: {sample['build_ns']/1e6:.2f} ms", flush=True)
manifest = {'records': records, 'affinity': sorted(os.sched_getaffinity(0)), 'binary_sha256': {label: hashlib.sha256(path.read_bytes()).hexdigest() for label, path in programs.items()}}
(out / 'trials.json').write_text(json.dumps(manifest, indent=2) + '\n')
for label in programs:
    values = [r['build_ns']/1e6 for r in records if r['label'] == label]
    print(f'{label} median={statistics.median(values):.2f} ms samples={values}', flush=True)
