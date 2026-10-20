import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export function brainSource() {
  const module=JSON.parse(execFileSync('go',['list','-m','-json','github.com/aexhq/brain'],{cwd:fileURLToPath(new URL('../runtime',import.meta.url)),encoding:'utf8'}));
  if(module.Replace || !module.Version || !module.Dir)throw new Error('runtime requires the immutable published Brain module');
  return module;
}
