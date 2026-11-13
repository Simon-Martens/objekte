import csv,json,math,statistics
from pathlib import Path
root=Path.cwd(); source=root/'target/hybrid-index-storage/followup-final-ceba166b-full'; out=root/'target/hybrid-index-storage/followup-final-ceba166b-summary'; out.mkdir(exist_ok=True)
all_rows=[]; summary={}
for mode in ['memory','resident','ondemand']:
 a={(r['case_id'],r['profile']):r for r in csv.DictReader((source/f'{mode}-start/query-results.csv').open())}
 b={(r['case_id'],r['profile']):r for r in csv.DictReader((source/f'{mode}-candidate/query-results.csv').open())}
 assert a.keys()==b.keys() and len(a)==440
 assert json.loads((source/f'{mode}-start/result-digests.json').read_text())==json.loads((source/f'{mode}-candidate/result-digests.json').read_text())
 for k in sorted(a):
  x,y=a[k],b[k]
  assert x['status']==y['status']=='ok'
  assert x['total_matches']==y['total_matches'] and x['returned_records']==y['returned_records']
  assert int(x['verified_result_hits'])==int(y['verified_result_hits'])==0
  row={'mode':mode,'case_id':k[0],'profile':k[1]}
  for field in ['median_ns','p95_ns']:
   u,v=int(x[field]),int(y[field])
   row.update({f'start_{field}':u,f'candidate_{field}':v,f'delta_{field}':v-u,f'ratio_{field}':v/u})
  for field in ['total_matches','returned_records','candidates','files_opened','bytes_read','record_parses','selected_payload_reads']:
   if field in x: row[f'start_{field}']=x[field]
   if field in y: row[f'candidate_{field}']=y[field]
  all_rows.append(row)
 subset=[r for r in all_rows if r['mode']==mode]
 summary[mode]={field:math.exp(statistics.mean(math.log(r[f'ratio_{field}']) for r in subset)) for field in ['median_ns','p95_ns']}
 summary[mode]['cases']=len(subset)
with (out/'per-case.csv').open('w',newline='') as f:
 writer=csv.DictWriter(f,fieldnames=list(dict.fromkeys(k for row in all_rows for k in row)),lineterminator='\n');writer.writeheader();writer.writerows(all_rows)
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
