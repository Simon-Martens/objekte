import csv,json,statistics,subprocess
from pathlib import Path
out=Path('/tmp/objekte-plan2-final-paired-query');out.mkdir(exist_ok=False)
cases=','.join(f'Q{i:03d}' for i in range(1,195))
rows=[]
for pair in range(3):
 for position,mode in enumerate(('before','after') if pair%2==0 else ('after','before')):
  binary=Path('/tmp/objekte-plan2-baseline-bin/bench_musenalm' if mode=='before' else '/home/simon/source/objekte-integration-boolean-field-predicates/target/release/examples/bench_musenalm')
  cwd=Path('/home/simon/source/objekte-integration-incremental-index-availability' if mode=='before' else '/home/simon/source/objekte-integration-boolean-field-predicates')
  dst=out/f'{pair+1:02d}-{position+1}-{mode}'
  cmd=[str(binary),'--root','/home/simon/source/objekte/examples/musenalm','--extended','--incremental','--profiles','ascii-off,unicode-off','--cases',cases,'--repetitions','5','--result-dir',str(dst)]
  r=subprocess.run(cmd,cwd=cwd,capture_output=True,text=True)
  if r.returncode: raise SystemExit(f'{mode} pair {pair+1}: {r.stderr[-1000:]}')
  for q in csv.DictReader(open(dst/'query-results.csv')):
   rows.append({'pair':pair+1,'mode':mode,'case_id':q['case_id'],'profile':q['profile'],'median_ns':int(q['median_ns']),'opens':int(q['files_opened']),'status':q['status'],'matches':int(q['total_matches'])})
  print(pair+1,mode,flush=True)
(out/'campaign.json').write_text(json.dumps(rows,indent=2)+'\n')
summary=[]
for case in cases.split(','):
 for profile in ('ascii-off','unicode-off'):
  a=[x for x in rows if x['case_id']==case and x['profile']==profile and x['mode']=='before']
  b=[x for x in rows if x['case_id']==case and x['profile']==profile and x['mode']=='after']
  assert all(x['status']=='ok' for x in a+b)
  assert len({x['matches'] for x in a+b})==1
  assert len({x['opens'] for x in a+b})==1
  ma=statistics.median(x['median_ns'] for x in a);mb=statistics.median(x['median_ns'] for x in b)
  summary.append({'case_id':case,'profile':profile,'before_ns':ma,'after_ns':mb,'delta_ns':mb-ma,'delta_fraction':mb/ma-1})
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
for profile in ('ascii-off','unicode-off'):
 s=[x for x in summary if x['profile']==profile]
 print(profile,'sum before',sum(x['before_ns'] for x in s)/1e6,'sum after',sum(x['after_ns'] for x in s)/1e6,flush=True)
for x in summary:
 if x['delta_ns']>500000 and x['delta_fraction']>0.1:
  print('OUTLIER',x['case_id'],x['profile'],round(x['before_ns']/1e6,3),round(x['after_ns']/1e6,3),flush=True)
