import hashlib,json,statistics,subprocess,time
from pathlib import Path
root=Path.cwd();full=root/'target/hybrid-index-storage/followup-final-ceba166b-full';out=root/'target/hybrid-index-storage/followup-final-ceba166b-opens';out.mkdir(exist_ok=True)
programs={'start':root/'target/hybrid-index-storage/followup-baseline-e397/bench_musenalm','candidate':root/'target/release/examples/bench_musenalm'}
records=[]
for mode in ['ondemand','resident']:
 for pair in range(1,8):
  for label in (['start','candidate'] if pair%2 else ['candidate','start']):
   result=out/f'{mode}-{pair}-{label}'
   cmd=[str(programs[label]),'--root',str(root/'examples/musenalm'),'--result-dir',str(result),'--extended','--incremental','--profiles','ascii-off,unicode-off','--repetitions','1','--cases','Q001','--verified-cache-bytes','0','--prepared-cache-bytes','16777216','--query-workers','32','--storage','custom',str(full/f'{label}-store')]
   if mode=='resident': cmd.append('--fully-resident')
   t=time.time();run=subprocess.run(cmd,capture_output=True,text=True)
   (out/f'{mode}-{pair}-{label}.log').write_text(run.stdout+'\nSTDERR\n'+run.stderr)
   if run.returncode:raise RuntimeError(run.stderr[-2000:])
   index=json.loads((result/'index-results.json').read_text())
   measures=[json.loads(line) for line in (result/'measurements.jsonl').read_text().splitlines()]
   assert len(index)==2 and len(measures)==2
   assert all(x['build_parses']==0 for x in index)
   rec={'mode':mode,'pair':pair,'label':label,'command':cmd,'seconds':time.time()-t,'open_ns':{x['alphabet']:x['open_ns'] for x in index},'first_query_ns':{x['profile']['alphabet']:x['total_ns'] for x in measures},'binary_sha256':hashlib.sha256(programs[label].read_bytes()).hexdigest()}
   records.append(rec);(out/'trials.json').write_text(json.dumps(records,indent=2)+'\n')
   print(mode,pair,label,'done',round(rec['seconds'],2),flush=True)
for mode in ['ondemand','resident']:
 for alphabet in ['ascii','unicode']:
  for field in ['open_ns','first_query_ns']:
   vals={label:[r[field][alphabet] for r in records if r['mode']==mode and r['label']==label] for label in programs}
   print(mode,alphabet,field,'median start/candidate ms',*[round(statistics.median(vals[label])/1e6,3) for label in programs],flush=True)
