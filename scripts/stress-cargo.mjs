import { execFileSync } from 'node:child_process';
import path from 'node:path';

// Cargo owns target-directory and target-triple resolution. Use the executable
// from this successful build, including when an unchanged artifact is fresh.
export function buildLockHelper(repositoryRoot) {
  const output = execFileSync('cargo', [
    'build', '--release', '--locked', '--example', 'hold_db_lock',
    '--message-format=json-render-diagnostics',
  ], {
    cwd: repositoryRoot, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  let executable;
  for (const line of output.split(/\r?\n/)) {
    if (!line.trim()) continue;
    let message;
    try { message = JSON.parse(line); }
    catch { throw new Error('Cargo returned malformed build metadata'); }
    if (message?.reason !== 'compiler-artifact'
      || message.target?.name !== 'hold_db_lock'
      || !Array.isArray(message.target.kind)
      || !message.target.kind.includes('example')) continue;
    if (typeof message.executable !== 'string' || !path.isAbsolute(message.executable)
      || message.executable.includes('\0')) {
      throw new Error('Cargo returned an invalid lock-helper executable path');
    }
    if (executable !== undefined) throw new Error('Cargo returned multiple lock-helper executables');
    executable = message.executable;
  }
  if (executable === undefined) throw new Error('Cargo did not report the lock-helper executable');
  return executable;
}
