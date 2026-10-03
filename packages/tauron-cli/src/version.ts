import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

/**
 * 本 CLI 自报的版本号——取自本包 `package.json`，不写字面量。
 *
 * 写死的后果是发版之后 `tauron --version` 与 `--help` 继续报**上一个**版本：
 * 用户据此提的 bug 单、生成的插件 manifest、排查用的日志全都带着错版本，
 * 而这不会让任何门禁变红。路径对 `src/`（vitest）与 `dist/`（发布产物）都成立，
 * 因为 npm 永远会把 `package.json` 放进 tarball。
 */
export const CLI_VERSION: string = (
  JSON.parse(readFileSync(fileURLToPath(new URL('../package.json', import.meta.url)), 'utf8')) as {
    version: string;
  }
).version;
