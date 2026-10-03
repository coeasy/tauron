import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const pkg = JSON.parse(
  readFileSync(fileURLToPath(new URL('../package.json', import.meta.url)), 'utf8'),
) as { version: string; tauron?: { publishedNpmVersion?: string } };

/**
 * 本 CLI 自报的版本号——取自本包 `package.json`，不写字面量。
 *
 * 写死的后果是发版之后 `tauron --version` 与 `--help` 继续报**上一个**版本：
 * 用户据此提的 bug 单、排查用的日志全都带着错版本，而这不会让任何门禁变红。
 * 路径对 `src/`（vitest）与 `dist/`（发布产物）都成立，因为 npm 永远会把
 * `package.json` 放进 tarball。
 */
export const CLI_VERSION: string = pkg.version;

/**
 * npm 上**已发布**的 `@tauron/*` 版本号（`@tauron/cli` 与其余包同批发版，
 * 故一个数字就够）。脚手架生成的工程依赖必须钉在这个值上，而不是
 * `CLI_VERSION`：轮 16 实测时 `CLI_VERSION` 是 1.1.0 而 registry `latest` 仍是
 * 1.0.2，`npm install` 会直接 `ETARGET`——按 README 走第二步的用户拿不到
 * 可安装的工程，这是前后端贯通意义上的真断链，不是文档瑕疵。
 *
 * 发版后把 `package.json` 的 `tauron.publishedNpmVersion` 一起抬上去即可；
 * 缺失时退回 `CLI_VERSION`（同批发版时两者相同）。
 */
export const PUBLISHED_NPM_VERSION: string = pkg.tauron?.publishedNpmVersion ?? pkg.version;
