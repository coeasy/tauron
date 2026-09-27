#!/usr/bin/env node
import { chmodSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const output = join(root, 'dist', 'create-tauron-app.js');

mkdirSync(dirname(output), { recursive: true });
writeFileSync(
  output,
  `#!/usr/bin/env node\nimport { main } from '@tauron/app-cli/cli';\n\nmain().catch((error) => {\n  console.error(error instanceof Error ? error.message : String(error));\n  process.exitCode = 1;\n});\n`,
  'utf8',
);
chmodSync(output, 0o755);
console.log('Built create-tauron-app executable.');
