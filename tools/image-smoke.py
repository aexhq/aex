"""Exercise the built non-root image while keeping its operator listener private."""
import json
import re
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

name='aex-smoke-'+uuid.uuid4().hex
root=Path(__file__).parent.parent
def docker(*args):
    return subprocess.check_output(['docker',*args],text=True).strip()
with tempfile.TemporaryDirectory() as directory:
    config=json.loads((root/'examples/config.json').read_text())
    config.update(data_dir='/var/lib/aex',listen='0.0.0.0:8081')
    path=Path(directory)/'config.json'
    path.write_text(json.dumps(config))
    try:
        docker('network','create',name)
        docker('run','-d','--name',name+'-db','--network',name,'-e','POSTGRES_PASSWORD=aex-local-test','-e','POSTGRES_DB=aex','postgres:17-bookworm')
        for _ in range(60):
            if subprocess.run(['docker','exec',name+'-db','pg_isready','-U','postgres'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0: break
            time.sleep(.5)
        else: raise RuntimeError('PostgreSQL did not become ready')
        docker('run','-d','--name',name,'--network',name,'--read-only','--cap-drop=ALL','--security-opt=no-new-privileges',
            '--mount',f'type=bind,source={path},target=/etc/aex/config.json,readonly',
            '--mount',f'type=volume,source={name},target=/var/lib/aex',
            '-e',f'AEX_DATABASE_URL=postgres://postgres:aex-local-test@{name}-db/aex','-e','AEX_SITE_TOKEN='+'s'*32,'-e','AEX_OPERATOR_TOKEN='+'o'*32,'-e','AEX_BRAIN_TOKEN='+'b'*32,'aex-rewrite:test')
        for attempt in range(60):
            result=subprocess.run(['docker','exec',name,'aex-server','operate','--request','{"action":"inspect"}'],capture_output=True,text=True)
            if result.returncode==0:
                assert json.loads(result.stdout)['accepting'] is False
                break
            time.sleep(.2)
        else:
            raise RuntimeError('image did not become operable')
        assert docker('inspect','--format','{{.Config.User}}',name)=='10002:10002'
        assert docker('inspect','--format','{{json .HostConfig.PortBindings}}',name) in ('{}','null')
        pinned=re.search(r"require github.com/aexhq/brain (\S+)",(root/'runtime/go.mod').read_text()).group(1)
        assert docker('run','--rm','--entrypoint','aex-runtime','aex-rewrite:test','-version')==pinned
        for uid,directory in [('10003','/var/lib/aex-tools-sandbox'),('10004','/var/lib/aex-tools-application')]:
            docker('run','--rm','--user',f'{uid}:{uid}','--read-only','--network','none','--cap-drop=ALL','--security-opt=no-new-privileges',
                   '--tmpfs',f'{directory}:rw,nosuid,noexec,uid={uid},gid={uid},mode=0700','--workdir','/opt/aex','--entrypoint','node','aex-rewrite:test','--input-type=module','-e',
                   'import {Journal} from "./services/tools/common.mjs"; import {Sandboxes} from "./services/tools/sandbox.mjs"; '
                   'import {modalProvider} from "./services/tools/modal.mjs"; import {toolServer} from "./services/tools/server.mjs"; '
                   f'const journal=new Journal("{directory}"); journal.set("proof",{{id:"proof"}}); if(journal.get("proof").id!=="proof")throw new Error("journal failed"); journal.close(); '
                   'const server=toolServer({token:"t".repeat(32),control:()=>{throw new Error("denied")},providers:{}}); '
                   'server.listen(0,"127.0.0.1"); await new Promise(resolve=>server.once("listening",resolve)); '
                   'try {if((await fetch(`http://127.0.0.1:${server.address().port}/application/read`)).status!==401)throw new Error("unauthenticated tool bridge");}finally{await new Promise(resolve=>server.close(resolve));}')
        docker('run','--rm','--user','10006:10006','--read-only','--network','none','--cap-drop=ALL','--security-opt=no-new-privileges',
               '--workdir','/opt/aex','--entrypoint','node','aex-rewrite:test','--input-type=module','-e',
               'import {descriptor} from "./brain-services/browser/src/browser.mjs"; if(descriptor.name!=="browser")throw new Error("browser dependency missing");')
        print('immutable Go runtime, non-root gateway, browser and separate retained tool journals passed')
    finally:
        subprocess.run(['docker','rm','-f',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)
        subprocess.run(['docker','volume','rm',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)

        subprocess.run(['docker','rm','-f',name+'-db'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)
        subprocess.run(['docker','network','rm',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)
