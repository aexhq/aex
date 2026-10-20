import { spawnSync } from 'node:child_process';
import { cp, mkdir, readFile, writeFile, readdir, chmod } from 'node:fs/promises';
import { join,resolve } from 'node:path';
import { brainSource } from './runtime-source.mjs';

const root=process.cwd(),artifacts=resolve('artifacts'),runners=join(artifacts,'runners');
function run(command,args,cwd=root,extra={}) {
  const result=spawnSync(command,args,{cwd,stdio:'inherit',shell:process.platform==='win32' && command==='npm',env:{...process.env,...extra}});
  if(result.error)throw result.error;if(result.status!==0)process.exit(result.status??1);
}
await mkdir(artifacts,{recursive:true});
run('go',['mod','download'],join(root,'runtime'));const source=brainSource();
await cp(join(source.Dir,'runners'),runners,{recursive:true,force:true});
async function writable(directory){await chmod(directory,0o755);for(const entry of await readdir(directory,{withFileTypes:true})){const path=join(directory,entry.name);if(entry.isDirectory())await writable(path);else await chmod(path,0o644);}}
await writable(runners);
for(const name of ['pi','codex','opencode']) {run('npm',['ci','--ignore-scripts'],join(runners,name));if(name==='pi')run('npm',['run','build'],join(runners,name));}
run('go',['build','-trimpath','-o',join(artifacts,process.platform==='win32'?'aex-runtime.exe':'aex-runtime'),'.'],join(root,'runtime'));
run('cargo',['build','--locked','-p','aex-server']);run('npm',['run','build']);
const packages=join(artifacts,'sdk-packages');await mkdir(packages,{recursive:true});
run('npm',['pack','--workspace','@aexhq/sdk','--pack-destination',packages]);
const version=JSON.parse(await readFile('packages/sdk/package.json','utf8')).version;
const consumer=join(artifacts,'sdk-consumer');await mkdir(consumer,{recursive:true});await writeFile(join(consumer,'package.json'),JSON.stringify({private:true,type:'module'}));
run('npm',['install','--ignore-scripts','--no-audit','--save-exact',join(packages,`aexhq-sdk-${version}.tgz`)],consumer);
run('node',['tools/browser-journey-bundle.mjs']);
const paths={
 AEX_TEST_SERVER:join(root,'target/debug',process.platform==='win32'?'aex-server.exe':'aex-server'),
 AEX_TEST_RUNTIME:join(artifacts,process.platform==='win32'?'aex-runtime.exe':'aex-runtime'),
 AEX_TEST_RUNNERS:runners,AEX_TEST_SDK_ENTRY:join(consumer,'node_modules/@aexhq/sdk/dist/index.js'),
};
await writeFile(join(artifacts,'journey-paths.json'),JSON.stringify(paths));
run('node',['--test','tests/journeys/hosted.test.mjs'],root,paths);
