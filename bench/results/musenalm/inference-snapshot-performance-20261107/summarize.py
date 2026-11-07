#!/usr/bin/env python3
"""Recreate compact per-sample CSVs from the untouched JSON measurements."""
import csv
import json
from pathlib import Path

here = Path(__file__).resolve().parent

def read(name):
    return json.loads((here / name).read_text())

def write(name, rows):
    rows = list(rows)
    with (here / name).open('w', newline='') as output:
        writer = csv.DictWriter(output, fieldnames=rows[0], lineterminator='\n')
        writer.writeheader()
        writer.writerows(rows)

snapshot = []
for campaign, labels in (('historical', ('old', 'new')), ('one-pass-codec', ('before', 'after'))):
    prefix = 'save' if campaign == 'historical' else 'codec'
    for label in labels:
        for pair in range(1, 8):
            data = read(f'{prefix}-{label}-{pair}.json')
            snapshot.append(dict(
                campaign=campaign, label=label, pair=pair,
                total_ms=data['save_total_ns'] / 1e6,
                header_ms=data['save_header_ns'] / 1e6,
                inventory_ms=data['save_inventory_ns'] / 1e6,
                serialize_ms=sum(s['serialize_ns'] for s in data['sections']) / 1e6,
                checksum_ms=sum(s['checksum_ns'] for s in data['sections']) / 1e6,
                write_ms=data['save_write_ns'] / 1e6,
                sync_ms=data['save_sync_ns'] / 1e6,
                rename_ms=data['save_rename_ns'] / 1e6,
                bytes=data['save_bytes'], sections=len(data['sections']),
                files=data['files'], records=data['records'], parses=data['build_parses'],
            ))
write('snapshot-samples.csv', snapshot)

inference = []
for campaign in ('policy', 'fusion'):
    for label in ('before', 'after'):
        for mode in ('disabled', 'report', 'auto'):
            for pair in range(1, 8):
                data = read(f'{"inference" if campaign == "policy" else "fusion"}-{label}-{mode}-{pair}.json')
                report = data['inference_report']
                inference.append(dict(
                    campaign=campaign, label=label, mode=mode, pair=pair,
                    open_ms=data['open_ns'] / 1e6,
                    build_ms=data['build_ns'] / 1e6,
                    discovery_ms=data['discovery_ns'] / 1e6,
                    extraction_index_ms=data['extraction_index_ns'] / 1e6,
                    admission_ms=data['admission_ns'] / 1e6,
                    finalization_ms=data['finalization_ns'] / 1e6,
                    peak_rss_mib=data['peak_resident_kib'] / 1024,
                    resident_mib=data['resident_kib'] / 1024,
                    workers=data['workers'], peak_active_jobs=data['peak_active_jobs'],
                    parses=data['build_parses'], records=data['records'],
                    observed_paths=report['observed_paths'] if report else 0,
                    admitted_paths=report['admitted_paths'] if report else 0,
                    complete=report['complete'] if report else '',
                    retained_report_bytes=report['retained_bytes_estimate'] if report else 0,
                ))
write('inference-samples.csv', inference)

musenalm = []
for stage in ('base', 'path', 'distinct', 'policy', 'fusion-musenalm'):
    for mode in ('disabled', 'report', 'auto'):
        data = read(f'{stage}-{mode}.json')
        report = data['inference_report']
        musenalm.append(dict(
            stage=stage, mode=mode, build_ms=data['build_ns'] / 1e6,
            discovery_ms=data['discovery_ns'] / 1e6,
            extraction_index_ms=data['extraction_index_ns'] / 1e6,
            finalization_ms=data['finalization_ns'] / 1e6,
            peak_rss_mib=data['peak_rss_after_open_bytes'] / 1048576,
            workers=data['resolved_build_workers'], parses=data['build_parses'],
            records=data['record_count'], corpus_sha256=data['corpus_sha256'],
            observed_paths=report['observed_paths'] if report else 0,
            admitted_paths=report['admitted_paths'] if report else 0,
            complete=report['complete'] if report else '',
        ))
write('musenalm-stage-samples.csv', musenalm)
