import hashlib, json, subprocess, time
from pathlib import Path
root=Path.cwd()
out=root/'target/hybrid-index-storage/followup-final-ceba166b-full'
out.mkdir(parents=True,exist_ok=True)
programs={'start':root/'target/hybrid-index-storage/followup-baseline-e397/bench_musenalm','candidate':root/'target/release/examples/bench_musenalm'}
common=['--root',str(root/'examples/musenalm'),'--extended','--incremental','--profiles','ascii-off,unicode-off','--verified-cache-bytes','0','--prepared-cache-bytes','16777216','--query-workers','32']
records=[]
for label in programs:
    store=out/f'{label}-store'
    result=out/f'prepare-{label}'
    command=[str(programs[label]),*common,'--repetitions','1','--cases','Q001','--result-dir',str(result),'--storage','custom',str(store)]
    t=time.time(); run=subprocess.run(command,capture_output=True,text=True)
    (out/f'prepare-{label}.log').write_text(run.stdout+'\nSTDERR\n'+run.stderr)
    print(f'prepare {label}: code {run.returncode} {time.time()-t:.1f}s',flush=True)
    if run.returncode: raise RuntimeError(run.stderr[-3000:])
for mode in ['memory','resident','ondemand']:
    for label in ['start','candidate']:
        result=out/f'{mode}-{label}'
        command=[str(programs[label]),*common,'--repetitions','30','--result-dir',str(result)]
        if mode=='memory': command+=['--storage','memory']
        else:
            command+=['--storage','custom',str(out/f'{label}-store')]
            if mode=='resident': command+=['--fully-resident']
        t=time.time(); run=subprocess.run(command,capture_output=True,text=True)
        (out/f'{mode}-{label}.log').write_text(run.stdout+'\nSTDERR\n'+run.stderr)
        record={'mode':mode,'label':label,'command':command,'binary_sha256':hashlib.sha256(programs[label].read_bytes()).hexdigest(),'seconds':time.time()-t,'returncode':run.returncode}
        records.append(record)
        (out/'runs.json').write_text(json.dumps(records,indent=2)+'\n')
        print(f'{mode} {label}: code {run.returncode} {record["seconds"]:.1f}s',flush=True)
        if run.returncode: raise RuntimeError(run.stderr[-3000:])
