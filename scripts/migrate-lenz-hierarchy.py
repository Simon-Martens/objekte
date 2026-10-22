#!/usr/bin/env python3
"""Convert Lenz child-side locations into parent-owned reference arrays, without changing IDs.
Run with --apply to write. Data is verified before writes; original JSON is backed up outside
of the repository. Re-running after migration is a no-op. Schemas/UI are versioned separately.
"""
import argparse, copy, json, pathlib, tempfile
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('root',nargs='?',default='examples/lenz-archiv')
p.add_argument('--apply',action='store_true')
args=p.parse_args(); root=pathlib.Path(args.root)
original={name:{path.stem:(path,json.loads(path.read_text())) for path in sorted((root/'data'/name).glob('*.json'))} for name in ('orte','sammlungen','handschriften')}
data={name:{key:copy.deepcopy(value) for key,(_,value) in docs.items()} for name,docs in original.items()}
if not any('Ort' in d or 'Sammlung' in d for n in ('sammlungen','handschriften') for d in data[n].values()):
 print('Already migrated. No files changed.'); raise SystemExit
for ort in data['orte'].values():
 assert not ort.get('sammlungen') and not ort.get('handschriften'), 'Mixed migration state'
 ort['sammlungen']=[];ort['handschriften']=[]
for key,group in data['sammlungen'].items():
 assert not group.get('handschriften'), 'Mixed migration state'
 ort=group.pop('Ort'); assert ort in data['orte'], (key,ort)
 data['orte'][ort]['sammlungen'].append(key); group['handschriften']=[]
direct=0
for key,record in data['handschriften'].items():
 ort=record.pop('Ort',None); group=record.pop('Sammlung',None)
 if group:
  assert group in data['sammlungen'], (key,group)
  parent=original['sammlungen'][group][1]['Ort']
  assert not ort or ort==parent, (key,ort,parent)
  data['sammlungen'][group]['handschriften'].append(key)
 else:
  assert ort in data['orte'], (key,ort)
  data['orte'][ort]['handschriften'].append(key); direct+=1
# Every record has exactly one placement after migration. Multiple placements remain possible.
placed=[i for n in ('orte','sammlungen') for v in data[n].values() for i in v['handschriften']]
assert len(placed)==len(set(placed))==len(data['handschriften'])
assert set(placed)==set(data['handschriften'])
assert sum(len(v['sammlungen']) for v in data['orte'].values())==len(data['sammlungen'])
for name,docs in data.items():
 for key,value in docs.items():
  before=original[name][key][1]
  removed={'Ort','Sammlung'} if name=='handschriften' else {'Ort'} if name=='sammlungen' else set()
  added={'sammlungen','handschriften'} if name=='orte' else {'handschriften'} if name=='sammlungen' else set()
  assert {k:v for k,v in value.items() if k not in added}=={k:v for k,v in before.items() if k not in removed}
print(f"Verified {len(data['orte'])} Orte, {len(data['sammlungen'])} Sammlungen, {len(placed)} Handschriften ({direct} direct). IDs and all other content unchanged.")
if args.apply:
 backup=pathlib.Path(tempfile.mkdtemp(prefix='objekte-before-hierarchy-'))
 for name,docs in data.items():
  (backup/name).mkdir()
  for key,value in docs.items():
   path=original[name][key][0];(backup/name/path.name).write_bytes(path.read_bytes())
   path.write_text(json.dumps(value,ensure_ascii=False,indent=2)+'\n')
 print(f'Applied. Original JSON backup: {backup}')
else: print('Dry run. Add --apply to write.')
