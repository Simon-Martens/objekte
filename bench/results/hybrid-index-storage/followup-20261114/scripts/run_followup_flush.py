import hashlib,json,os,shutil,statistics,subprocess,time
from pathlib import Path
root=Path.cwd();out=root/'target/hybrid-index-storage/followup-final-471b79c3-flush';out.mkdir(exist_ok=True)
program=root/'target/release/examples/bench_managed_flush';records=[]
for i in range(1,8):
 store=out/f'{i}-store'
 cmd=[str(program),'--root',str(root/'examples/musenalm'),'--output',str(store),'--alphabet','ascii']
 t=time.time(); run=subprocess.run(cmd,capture_output=True,text=True)
 if run.returncode:raise RuntimeError(run.stderr[-2000:])
 sample=json.loads(run.stdout)
 assert sample['revision']==sample['durable_generation']
 records.append({'pair':i,'command':cmd,'binary_sha256':hashlib.sha256(program.read_bytes()).hexdigest(),'seconds':time.time()-t,'sample':sample})
 (out/'trials.json').write_text(json.dumps(records,indent=2)+'\n')
 shutil.rmtree(store)
 print(i,round(sample['flush_ns']/1e6,3),round(sample['unchanged_flush_ns']/1e6,3),flush=True)
for key in ['flush_ns','unchanged_flush_ns']:
 print(key,'median ms',statistics.median(x['sample'][key] for x in records)/1e6,flush=True)
