#!/usr/bin/env python3
"""Seven alternating source-free opens for both stored residency modes."""
import argparse
import hashlib
import json
import statistics
import subprocess
from pathlib import Path


def sha256(path):
    digest=hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda:source.read(1024*1024),b''):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser=argparse.ArgumentParser()
    for name in ('baseline','candidate','baseline-store','candidate-store','root','scratch','output'):
        parser.add_argument('--'+name,type=Path,required=True)
    args=parser.parse_args()
    args.scratch.mkdir(parents=True,exist_ok=True)
    records=[]
    for mode in ('resident','ondemand'):
        for pair in range(7):
            for label in (('A','B') if pair%2==0 else ('B','A')):
                program=args.baseline if label=='A' else args.candidate
                store=args.baseline_store if label=='A' else args.candidate_store
                folder=args.scratch/mode/f'{pair}-{label}'
                cmd=[str(program),'--root',str(args.root),'--extended','--incremental',
                     '--storage','custom',str(store),'--profiles','ascii-off,unicode-off',
                     '--cases','Q167','--repetitions','1','--verified-cache-bytes','0',
                     '--prepared-cache-bytes','16777216','--query-workers','32',
                     '--result-dir',str(folder)]
                if mode=='resident': cmd.append('--fully-resident')
                print(f'{mode} pair {pair} {label}',flush=True)
                result=subprocess.run(cmd,capture_output=True,text=True)
                if result.returncode: raise RuntimeError(f'{label} {mode}: {result.stderr[-2000:]}')
                results=json.loads((folder/'index-results.json').read_text())
                if any(row['source_reads']!=0 or row['build_parses']!=0 for row in results):
                    raise RuntimeError(f'{label} {mode}: rebuilt from source')
                records.append({'mode':mode,'pair':pair,'label':label,'command':cmd,
                                'index_results':results,
                                'digest':json.loads((folder/'result-digests.json').read_text())})
    digest=records[0]['digest']
    if any(record['digest']!=digest for record in records):
        raise RuntimeError('result digest mismatch')
    summary=[]
    for mode in ('resident','ondemand'):
        for alphabet in ('ascii','unicode'):
            baseline=[];candidate=[]
            for pair in range(7):
                for label,values in (('A',baseline),('B',candidate)):
                    record=next(r for r in records if r['mode']==mode and r['pair']==pair and r['label']==label)
                    values.append(next(x for x in record['index_results'] if x['alphabet']==alphabet)['open_ns'])
            summary.append({'mode':mode,'alphabet':alphabet,'baseline_ns':baseline,
                            'candidate_ns':candidate,
                            'baseline_median_ns':statistics.median(baseline),
                            'candidate_median_ns':statistics.median(candidate),
                            'ratio':statistics.median(candidate)/statistics.median(baseline)})
    output={'binary_sha256':{'A':sha256(args.baseline),'B':sha256(args.candidate)},
            'baseline_store':str(args.baseline_store),'candidate_store':str(args.candidate_store),
            'fixture_root':str(args.root),'source_free':True,'digest_parity':True,
            'summary':summary,'records':records}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(output,indent=2)+'\n')
    print('All source-free opens and digests verified',flush=True)

if __name__=='__main__': main()
