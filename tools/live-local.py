"""Run the live-provider release check against local Linux Aex and Brain processes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request
import uuid


def port():
    with socket.socket() as s:
        s.bind(('127.0.0.1',0))
        return s.getsockname()[1]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace-env',type=Path,required=True)
    args=parser.parse_args()
    values={}
    for line in args.workspace_env.read_text().splitlines():
        key,sep,value=line.partition('=')
        if sep and key.strip() in ('VERCEL_AI_GATEWAY_API_KEY','AI_GATEWAY_API_KEY'):
            values[key.strip()]=value.strip().strip('"').strip("'")
    model_key=values.get('VERCEL_AI_GATEWAY_API_KEY') or values.get('AI_GATEWAY_API_KEY')
    if not model_key: raise ValueError('a gateway model credential is required')
    root=Path(__file__).parent.parent
    brain_port,aex_port,admin_port=port(),port(),port()
    brain_token,operator_token=uuid.uuid4().hex,uuid.uuid4().hex
    schema='live_'+uuid.uuid4().hex
    def database(sql):
        return subprocess.check_output(['node','--input-type=module','-e',
            'import {Client} from \"pg\";const c=new Client({connectionString:process.env.AEX_TEST_DATABASE_URL});await c.connect();await c.query(process.argv[1]);await c.end()',sql],cwd=root,text=True)
    database('CREATE SCHEMA '+schema)
    from urllib.parse import urlparse,parse_qsl,urlencode,urlunparse
    parsed=urlparse(os.environ['AEX_TEST_DATABASE_URL'])
    database_url=urlunparse(parsed._replace(query=urlencode([*parse_qsl(parsed.query),('options','-csearch_path='+schema)])))
    children=[]
    with tempfile.TemporaryDirectory(prefix='aex-live-') as directory:
        path=Path(directory)
        config=json.loads((root/'examples/config.json').read_text())
        config.update(listen=f'127.0.0.1:{aex_port}',operator_listen=f'127.0.0.1:{admin_port}',data_dir=str(path/'aex'),brain_url=f'http://127.0.0.1:{brain_port}',
            agentloops=[hashlib.sha256(Path(os.environ['BRAIN_TEST_REFERENCE_AGENTLOOP']).read_bytes()).hexdigest()],models=['vercel-ai-gateway/openai/gpt-4.1-mini'])
        (path/'config.json').write_text(json.dumps(config))
        environment={**os.environ,'BRAIN_LISTEN':f'127.0.0.1:{brain_port}','BRAIN_DATA_DIR':str(path/'brain'),'BRAIN_ENV_WORKER':os.environ['BRAIN_TEST_WORKER'],'BRAIN_API_TOKEN':brain_token,
            'AEX_DATABASE_URL':database_url,'AEX_SITE_TOKEN':uuid.uuid4().hex,'AEX_BRAIN_TOKEN':brain_token,'AEX_OPERATOR_TOKEN':operator_token,'AEX_URL':f'http://127.0.0.1:{aex_port}','AEX_MODEL_KEY':model_key}
        def operate(body):
            request=urllib.request.Request(f'http://127.0.0.1:{admin_port}/operate',data=json.dumps(body).encode(),headers={'content-type':'application/json','authorization':'Bearer '+operator_token})
            with urllib.request.urlopen(request,timeout=30) as response:return json.load(response)
        def launch(argv,p):
            child=subprocess.Popen(argv,env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
            children.append(child)
            for _ in range(600):
                if child.poll() is not None:raise RuntimeError('local process failed to start')
                try:
                    with urllib.request.urlopen(f'http://127.0.0.1:{p}/health/live',timeout=1):return
                except OSError:time.sleep(.05)
            raise TimeoutError('local readiness timed out')
        try:
            launch([environment['BRAIN_TEST_SERVER']],brain_port)
            launch([environment['AEX_TEST_SERVER'],'serve','--config',str(path/'config.json')],aex_port)
            operate({'action':'report_usage','observed_at':int(time.time()),'sessions':{}})
            operate({'action':'resume'})
            account=operate({'action':'create_account'})['account']
            environment['AEX_API_KEY']=operate({'action':'issue_key','account':account})['token']
            subprocess.run(['node',str(root/'tools/live-check.mjs')],env=environment,check=True,timeout=120)
        finally:
            for child in reversed(children):
                if child.poll() is None:os.killpg(child.pid,signal.SIGKILL)
                child.wait()
            database('DROP SCHEMA '+schema+' CASCADE')


if __name__=='__main__':main()
