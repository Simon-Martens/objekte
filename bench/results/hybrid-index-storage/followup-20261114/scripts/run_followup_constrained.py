import json,subprocess
from pathlib import Path
root=Path.cwd();out=root/'target/hybrid-index-storage/followup-final-ceba166b-constrained';full=root/'target/hybrid-index-storage/followup-final-ceba166b-full'
programs={'start':root/'target/hybrid-index-storage/followup-baseline-e397/bench_musenalm','candidate':root/'target/release/examples/bench_musenalm'}
records=[]
for mode,label in [('ondemand','start'),('resident','start'),('resident','candidate')]:
 name=f'{label}-{mode}-512m';result=out/name;timefile=out/f'{name}.time'
 cmd=['systemd-run','--user','--scope','-p','MemoryMax=512M','--collect','--','/usr/bin/time','-f','major=%F minor=%R input=%I rss_kb=%M','-o',str(timefile),str(programs[label]),'--root',str(root/'examples/musenalm'),'--result-dir',str(result),'--extended','--incremental','--profiles','unicode-off','--repetitions','30','--cases','Q006,Q175','--verified-cache-bytes','0','--prepared-cache-bytes','16777216','--query-workers','32','--storage','custom',str(full/f'{label}-store')]
 if mode=='resident':cmd.append('--fully-resident')
 run=subprocess.run(cmd,capture_output=True,text=True)
 (out/f'{name}.log').write_text(run.stdout+'\nSTDERR\n'+run.stderr)
 print(name,'exit',run.returncode,'time',timefile.read_text().strip() if timefile.exists() else 'missing',flush=True)
 if run.returncode:raise RuntimeError(run.stderr[-1000:])
 records.append({'mode':mode,'label':label,'command':cmd,'time':timefile.read_text().strip()})
(out/'runs.json').write_text(json.dumps(records,indent=2)+'\n')
