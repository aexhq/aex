import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdtemp, writeFile, readdir, lstat, readFile, rm, chmod, chown } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';

const exec = promisify(execFile);
export const maxBytes = 20 * 1024 * 1024;
let parent;

// Each parser gets a temporary UID and no service credentials. A compromised
// parser cannot read another job or keep a process alive after its UID is reused.
export async function renderPDF(bytes, uid, signal) {
  if (process.platform !== 'linux' || process.getuid() !== 0) throw new Error('PDF rendering requires the isolated Linux service');
  if (!bytes.subarray(0, 5).equals(Buffer.from('%PDF-')) || bytes.length > maxBytes) throw new Error('invalid PDF');
  parent ??= mkdtemp(join(tmpdir(), 'aex-pdf-controller-')).then(async path=>{await chmod(path,0o711);return path;});
  const directory = await mkdtemp(join(await parent, 'job-'));
  await chmod(directory, 0o700);
  await writeFile(join(directory, 'input.pdf'), bytes, { mode: 0o600 });
  await chown(join(directory, 'input.pdf'), uid, uid);
  await chown(directory, uid, uid);
  try {
    await new Promise((resolve, reject) => {
      const child = spawn('prlimit', ['--fsize=20971520', '--as=1073741824', '--cpu=30', '--nproc=64', '--',
        'setpriv', `--reuid=${uid}`, `--regid=${uid}`, '--clear-groups', '--bounding-set=-all', '--no-new-privs',
        'pdftoppm', '-png', '-scale-to', '1600', '-f', '1', '-l', '65', 'input.pdf', 'page'], {
        cwd: directory, env: { PATH: '/usr/bin:/bin', HOME: directory }, stdio: 'ignore', signal, timeout: 45000,
      });
      child.once('error', reject);
      child.once('exit', code => code === 0 ? resolve() : reject(new Error('PDF rendering failed')));
    });
    await kill(uid);
    const names = (await readdir(directory)).filter(name => /^page-\d+\.png$/.test(name)).sort((a,b) => Number(a.match(/\d+/)[0])-Number(b.match(/\d+/)[0]));
    if (!names.length || names.length > 64) throw new Error('PDF page limit exceeded');
    const pages = []; let total = 0;
    for (const name of names) {
      const path = join(directory, name), info = await lstat(path);
      if (!info.isFile() || info.nlink !== 1 || (total += info.size) > maxBytes) throw new Error('PDF output limit exceeded');
      const page = await readFile(path);
      if (!page.subarray(0, 8).equals(Buffer.from('\x89PNG\r\n\x1a\n', 'binary'))) throw new Error('invalid rendered page');
      pages.push(page);
    }
    return pages;
  } finally { await kill(uid); await rm(directory, { recursive: true, force: true }); }
}
async function kill(uid) {
  try { await exec('pkill', ['-KILL', '-u', String(uid)], { env: { PATH:'/usr/bin:/bin' }, timeout:5000 }); }
  catch (error) { if (error.code !== 1) throw error; }
}
