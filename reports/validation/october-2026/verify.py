#!/usr/bin/env python3
"""Verify the compact completed-evidence archive without private input files."""
import hashlib,json,math,re
from pathlib import Path

def reject_duplicates(pairs):
 out={}
 for key,value in pairs:
  if key in out:raise ValueError('duplicate JSON key')
  out[key]=value
 return out

def read_json(path):
 return json.loads(path.read_text(),object_pairs_hook=reject_duplicates,parse_constant=lambda _: (_ for _ in ()).throw(ValueError('nonfinite JSON')))

def kind(value):
 if value is None:return 'null'
 if isinstance(value,bool):return 'boolean'
 if isinstance(value,int):return 'integer'
 if isinstance(value,float):
  assert math.isfinite(value);return 'number'
 if isinstance(value,str):return 'string'
 if isinstance(value,dict):assert not value;return 'empty_object'
 if isinstance(value,list):assert not value;return 'empty_array'
 raise ValueError('unsupported JSON value')

def leaves(value,path=''):
 if isinstance(value,dict) and value:
  for key,child in value.items():yield from leaves(child,path+'/'+key.replace('~','~0').replace('/','~1'))
 elif isinstance(value,list) and value:
  for index,child in enumerate(value):yield from leaves(child,path+'/'+str(index))
 else:yield path,kind(value)

def containers(value,path=''):
 if isinstance(value,(dict,list)):
  yield path,'object' if isinstance(value,dict) else 'array'
  children=value.items() if isinstance(value,dict) else enumerate(value)
  for key,child in children:
   yield from containers(child,path+'/'+str(key).replace('~','~0').replace('/','~1'))

FORBIDDEN_KEYS={'pid','backend_pid','process_id','proc_start_ticks','origin','token_path','access_token_path','environment','worktree','binary','path','owned_temp_directory','child_pids','stderr','stdout','tail','authorization','api_key'}
PRIVATE_TEXT=re.compile(r'/home/|/tmp/|\.git/|\b(?:Basic|Bearer)\s+[A-Za-z0-9+/=_-]+|jo_(?:access|local)_[a-f0-9]{64}|synthetic-mixed-soak-provider-key',re.I)
def privacy(value):
 if isinstance(value,dict):
  assert not (set(k.lower() for k in value)&FORBIDDEN_KEYS),'forbidden field'
  for k,v in value.items():assert not PRIVATE_TEXT.search(k),'private key text';privacy(v)
 elif isinstance(value,list):
  for v in value:privacy(v)
 elif isinstance(value,str):assert not PRIVATE_TEXT.search(value),'private/credential text'

def verify(folder):
 folder=folder.resolve();members=list(folder.iterdir())
 assert not any(p.is_symlink() or not p.is_file() for p in members),'regular files only'
 assert sum(p.stat().st_size for p in members if p.name!='verify.py')<=500000,'archive summary budget'
 assert (folder/'verify.py').stat().st_size<=32768,'verifier size budget'
 manifest=read_json(folder/'evidence-manifest.json');privacy(manifest)
 assert manifest['status']=='session_stopped_at_user_request'
 assert manifest['pending']==[]
 assert set(manifest['documents'])=={'README.md','issue-pr-index.md'}
 expected={'evidence-manifest.json','verify.py',*manifest['files'],*manifest['documents']}
 assert {p.name for p in folder.iterdir()}==expected,'unexpected/missing archive member'
 assert not any(p.is_symlink() or not p.is_file() for p in folder.iterdir()),'regular files only'
 total=0
 for name,meta in manifest['files'].items():
  assert re.fullmatch(r'[a-z0-9-]+\.json',name)
  raw=(folder/name).read_bytes();total+=len(raw)
  assert len(raw)==meta['published_bytes'] and hashlib.sha256(raw).hexdigest()==meta['published_sha256'],name
  data=read_json(folder/name);privacy(data)
  assert dict(leaves(data))==meta['allowed_leaves'],name+' leaf schema'
  assert dict(containers(data))==meta['allowed_containers'],name+' container schema'
 for name,meta in manifest['documents'].items():
  raw=(folder/name).read_bytes();total+=len(raw)
  assert len(raw)==meta['published_bytes'] and hashlib.sha256(raw).hexdigest()==meta['published_sha256'],name
  privacy(raw.decode('utf-8'))
 verifier=(folder/'verify.py').read_bytes()
 assert hashlib.sha256(verifier).hexdigest()==manifest['verifier_sha256'],'verifier hash'
 assert total+len((folder/'evidence-manifest.json').read_bytes())<=500000,'archive summary budget'
 assert read_json(folder/'million-boundaries-failed.json')['http_failed']['passed'] is False
 assert read_json(folder/'million-boundaries-failed.json')['profile_failed']['passed'] is False
 assert read_json(folder/'million-store-profile-v3.json')['profile_v3']['passed'] is True
 assert read_json(folder/'browser-9000s-completed.json')['browser']['complete'] is True
 interrupted=read_json(folder/'interrupted-endurance.json')
 assert len(interrupted['varied']['runs'])==2
 assert all(run['passed'] is False and run['not_full_duration_pass'] is True and run['state']=='stopped' for run in interrupted['varied']['runs'])
 assert interrupted['mixed_and_monitor']['mixed']['raw_passed'] is False
 assert interrupted['mixed_and_monitor']['status']=='interrupted_by_user'
 assert interrupted['browser_interrupted']['complete'] is False
 assert interrupted['browser_interrupted']['counts']['steady_state_failures']==3
 assert len(interrupted['browser_interrupted']['preserved_steady_errors'])==3
 assert interrupted['mixed_and_monitor']['not_launched']=={'final_backend_cycles':100,'final_browser_seconds':5400}
 native=read_json(folder/'native-9bb84db.json')['native_final']
 assert native['passed'] is True and native['run']['conclusion']=='success'
 assert native['run']['head_sha']=='9bb84db756c76e834d6592d55dda5e9cd9689f3f'
 assert len(native['targets'])==6 and len({row['target'] for row in native['targets']})==6
 assert all(row['report_and_package_log_match'] is True and row['sdk_passed'] is True and row['upgrade_checks_passed']==36 for row in native['targets'])
 assert native['aggregate']['conclusion']=='success' and native['publish']=='skipped'
 return {'passed':True,'files':len(manifest['files']),'summary_bytes':total,'manifest_bytes':(folder/'evidence-manifest.json').stat().st_size,'private_sources_required':False,'documents':len(manifest['documents']),'final_validation':'user_stopped_with_partial_runs_preserved'}

if __name__=='__main__':print(json.dumps(verify(Path(__file__).resolve().parent),sort_keys=True))
