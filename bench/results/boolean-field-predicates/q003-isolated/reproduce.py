import csv,json,statistics,subprocess
from pathlib import Path
out=Path('/tmp/objekte-plan2-q003-isolated');out.mkdir(exist_ok=False)
rows=[]
for pair in range(5):
 for position,mode in enumerate(('before','after') if pair%2==0 else ('after','before')):
  binary=Path('/tmp/objekte-plan2-baseline-bin/bench_musenalm' if mode=='before' else '/home/simon/source/objekte-integration-boolean-field-predicates/target/release/examples/bench_musenalm')
  cwd=Path('/home/simon/source/objekte-integration-incremental-index-availability' if mode=='before' else '/home/simon/source/objekte-integration-boolean-field-predicates')
  dst=out/f'{pair+1:02d}-{position+1}-{mode}'
  cmd=[str(binary),'--root','/home/simon/source/objekte/examples/musenalm','--extended','--incremental','--profiles','ascii-off','--cases','Q003','--repetitions','15','--result-dir',str(dst)]
  r=subprocess.run(cmd,cwd=cwd,capture_output=True,text=True)
  if r.returncode:raise SystemExit(f'{mode} pair {pair+1}: {r.stderr[-1000:]}')
  q=list(csv.DictReader(open(dst/'query-results.csv')))[0]
  rows.append({'pair':pair+1,'position':position+1,'mode':mode,'median_ns':int(q['median_ns']),'opens':int(q['files_opened']),'status':q['status'],'matches':int(q['total_matches'])})
  print(pair+1,mode,rows[-1]['median_ns']/1e6,flush=True)
(out/'campaign.json').write_text(json.dumps(rows,indent=2)+'\n')
for mode in ('before','after'):
 a=[r['median_ns'] for r in rows if r['mode']==mode]
 print(mode,statistics.median(a)/1e6,flush=True)
