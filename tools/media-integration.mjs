import { execFileSync,spawnSync } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
await mkdir('artifacts/media-integration',{recursive:true});
const certificate=resolve('artifacts/media-integration/ca.pem'),key=resolve('artifacts/media-integration/key.pem');
execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',key,'-out',certificate,'-days','1','-subj','/CN=Aex integration CA','-addext','subjectAltName=IP:127.0.0.1'],{stdio:'ignore'});
const result=spawnSync(process.execPath,['--test','tests/journeys/media.test.mjs'],{stdio:'inherit',env:{...process.env,NODE_EXTRA_CA_CERTS:certificate,AEX_TEST_TLS_KEY:key}});
if(result.error)throw result.error;process.exit(result.status??1);
