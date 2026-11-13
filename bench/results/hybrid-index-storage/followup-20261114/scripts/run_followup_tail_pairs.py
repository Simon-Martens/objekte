import csv,hashlib,json,statistics,subprocess,time
from pathlib import Path
root=Path.cwd(); out=root/'target/hybrid-index-storage/followup-final-ceba166b-tail'; out.mkdir(exist_ok=True)
full=root/'target/hybrid-index-storage/followup-final-ceba166b-full'
programs={'start':root/'target/hybrid-index-storage/followup-baseline-e397/bench_musenalm','candidate':root/'target/release/examples/bench_musenalm'}
records=[]
for mode in ['memory','ondemand']:
 for pair in range(1,8):
  order=['start','candidate'] if pair%2 else ['candidate','start']
  for label in order:
   result=out/f'{mode}-{pair}-{label}'
   cmd=[str(programs[label]),'--root',str(root/'examples/musenalm'),'--result-dir',str(result),'--extended','--incremental','--profiles','ascii-off,unicode-off','--repetitions','30','--cases','Q167,Q175,Q177','--verified-cache-bytes','0','--prepared-cache-bytes','16777216','--query-workers','32']
   if mode=='memory': cmd+=['--storage','memory']
   else: cmd+=['--storage','custom',str(full/f'{label}-store')]
   t=time.time(); run=subprocess.run(cmd,capture_output=True,text=True)
   (out/f'{mode}-{pair}-{label}.log').write_text(run.stdout+'\nSTDERR\n'+run.stderr)
   if run.returncode: raise RuntimeError(run.stderr[-2000:])
   rows=list(csv.DictReader((result/'query-results.csv').open()))
   records.append({'mode':mode,'pair':pair,'label':label,'command':cmd,'seconds':time.time()-t,'rows':rows})
   (out/'trials.json').write_text(json.dumps(records,indent=2)+'\n')
   print(mode,pair,label,'done',round(time.time()-t,1),flush=True)
for mode in ['memory','ondemand']:
 for case in ['Q167','Q175','Q177']:
  for profile in ['ascii-off','unicode-off']:
   vals={label:[int(next(row for row in rec['rows'] if row['case_id']==case and row['profile']==profile)['p95_ns']) for rec in records if rec['mode']==mode and rec['label']==label] for label in programs}
   print(mode,case,profile,'p95 median ms',*[round(statistics.median(vals[label])/1e6,3) for label in programs],flush=True)
