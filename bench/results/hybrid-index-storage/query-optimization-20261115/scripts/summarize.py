#!/usr/bin/env python3
"""Validate campaign parity and write aggregate and selected-case ratios."""
import csv
import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FULL = ROOT / 'full'
MODES = ('baseline','memory','resident','ondemand')
PROFILES = ('ascii-off','unicode-off')
SELECTED = ('Q006','Q007','Q025','Q026','Q167','Q174','Q175','Q177','Q187','Q202','Q203','Q204')


def read(mode):
    folder = FULL / mode
    return (json.loads((folder/'run-manifest.json').read_text()),
            json.loads((folder/'result-digests.json').read_text()),
            {(row['case_id'],row['profile']):row for row in csv.DictReader((folder/'query-results.csv').open())},
            json.loads((folder/'index-results.json').read_text()))


def geomean(values):
    return math.exp(sum(math.log(value) for value in values)/len(values))


def main():
    data={mode:read(mode) for mode in MODES}
    baseline=data['baseline']
    expected=set(baseline[2])
    if len(expected)!=440 or any(row['status']!='ok' for row in baseline[2].values()):
        raise RuntimeError('baseline case coverage or status differs')
    for mode, (manifest,digests,rows,_) in data.items():
        if set(rows)!=expected or any(row['status']!='ok' for row in rows.values()):
            raise RuntimeError(f'{mode} case coverage or status differs')
        if digests!=baseline[1]:
            raise RuntimeError(f'{mode} result digest mismatch')
        for field in ('schema_fingerprint','workload_fingerprint','corpus_fingerprint','cargo_lock_sha256'):
            if manifest[field]!=baseline[0][field]:
                raise RuntimeError(f'{mode} {field} mismatch')
    summary={'provenance':{mode:{key:data[mode][0][key] for key in ('code_revision','executable_sha256','cargo_lock_sha256','schema_fingerprint','workload_fingerprint','corpus_fingerprint','compiler','command')} for mode in MODES},
             'case_profile_count':len(expected),'result_digest_parity':True,'modes':{},'selected_cases':[]}
    for mode in MODES[1:]:
        rows=data[mode][2]
        summary['modes'][mode]={}
        for profile in PROFILES:
            summary['modes'][mode][profile]={metric+'_geomean':geomean([
                int(rows[key][metric])/int(baseline[2][key][metric]) for key in expected if key[1]==profile
            ]) for metric in ('median_ns','p95_ns')}
        if mode in ('resident','ondemand'):
            if any(row['source_reads']!=0 or row['build_parses']!=0 for row in data[mode][3]):
                raise RuntimeError(f'{mode} rebuilt instead of source-free open')
    for case in SELECTED:
        for profile in PROFILES:
            key=(case,profile)
            row={'case':case,'profile':profile,'baseline_median_ns':int(baseline[2][key]['median_ns'])}
            for mode in MODES[1:]:
                candidate=int(data[mode][2][key]['median_ns'])
                row[mode+'_median_ns']=candidate
                row[mode+'_ratio']=candidate/row['baseline_median_ns']
            summary['selected_cases'].append(row)
    summary['index_results']={mode:rows for mode,(_,_,_,rows) in data.items()}
    summary['tail_abba']=json.loads((ROOT/'tail-abba/summary.json').read_text())['summary']
    summary['open_pairs']=json.loads((ROOT/'open-pairs.json').read_text())['summary']
    (ROOT/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print('Validated 440 case/profile rows in four modes and identical result digests')

if __name__=='__main__':
    main()
