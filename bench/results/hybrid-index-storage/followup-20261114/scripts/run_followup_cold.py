import json,os,subprocess,time
from pathlib import Path
root=Path.cwd();full=root/'target/hybrid-index-storage/followup-final-ceba166b-full';out=root/'target/hybrid-index-storage/followup-final-ceba166b-cold';out.mkdir(exist_ok=True)
programs={'start':root/'target/hybrid-index-storage/followup-baseline-e397/bench_musenalm','candidate':root/'target/release/examples/bench_musenalm'}
records=[]
def resident(files):
 p=subprocess.run(['fincore','-J','-b',*[str(f) for f in files]],capture_output=True,text=True,check=True)
 return sum(int(item['res']) for item in json.loads(p.stdout)['fincore'])
for pair in range(1,4):
 for label in (['start','candidate'] if pair%2 else ['candidate','start']):
  store=full/f'{label}-store';files=[p for p in store.rglob('*') if p.is_file() and p.suffix in ('.idx','.bin')]
  before=resident(files)
  for p in files:
   with p.open('rb') as f:os.posix_fadvise(f.fileno(),0,0,os.POSIX_FADV_DONTNEED)
  evicted=resident(files)
  result=out/f'{pair}-{label}'
  cmd=[str(programs[label]),'--root',str(root/'examples/musenalm'),'--result-dir',str(result),'--extended','--incremental','--profiles','ascii-off,unicode-off','--repetitions','1','--cases','Q001','--verified-cache-bytes','0','--prepared-cache-bytes','16777216','--query-workers','32','--storage','custom',str(store)]
  timefile=out/f'{pair}-{label}.time'
  run=subprocess.run(['/usr/bin/time','-f','major_faults=%F minor_faults=%R filesystem_inputs=%I filesystem_outputs=%O','-o',str(timefile),*cmd],capture_output=True,text=True)
  (out/f'{pair}-{label}.log').write_text(run.stdout+'\nSTDERR\n'+run.stderr)
  if run.returncode:raise RuntimeError(run.stderr[-2000:])
  after=resident(files)
  index=json.loads((result/'index-results.json').read_text());measure=[json.loads(line) for line in (result/'measurements.jsonl').read_text().splitlines()]
  record={'pair':pair,'label':label,'command':cmd,'index_file_bytes':sum(p.stat().st_size for p in files),'cached_bytes_before':before,'cached_bytes_after_fadvise':evicted,'cached_bytes_after_query':after,'time':timefile.read_text().strip(),'open_ns':{x['alphabet']:x['open_ns'] for x in index},'first_query_ns':{x['profile']['alphabet']:x['total_ns'] for x in measure}}
  records.append(record);(out/'trials.json').write_text(json.dumps(records,indent=2)+'\n')
  print(pair,label,'cached',before,evicted,after,'open',record['open_ns'],'time',record['time'],flush=True)
