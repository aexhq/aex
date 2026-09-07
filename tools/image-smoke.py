"""Exercise the built non-root image while keeping its operator listener private."""
import json
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
        print('non-root image, durable writable path and private operator smoke passed')
    finally:
        subprocess.run(['docker','rm','-f',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)
        subprocess.run(['docker','volume','rm',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)

        subprocess.run(['docker','rm','-f',name+'-db'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)
        subprocess.run(['docker','network','rm',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=False)
