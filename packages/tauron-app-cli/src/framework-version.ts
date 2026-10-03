import { readFileSync } from 'node:fs';

/**
 * 脚手架生成的依赖固定到这里，别在生成器里另写字面量。
 *
 * 为什么值得单一真源：这三条 pin（`@tauron/host`、`tauron-adapter`、
 * `tauron.plugin.json` 的 `framework`）必须与发版版本一致，写死的后果是发版后
 * 生成的工程仍在装**上一个**版本并「构建通过」——那比失败更危险。
 * `framework-version.test.ts` 把它钉在本包 `package.json` 上，改版本没改这里就红。
 */
export const FRAMEWORK_VERSION = '1.1.0';

/**
 * registry（npm + crates.io 同批发版）上**已发布**的现值。
 *
 * 为什么要有第二个数字：生成的工程把 npm 与 Cargo 都钉在 `FRAMEWORK_VERSION` 上，
 * 而发版有先后——轮 16 实测时源码是 1.1.0、registry 仍是 1.0.2，按 README 走
 * `npm install` 的人直接拿到 `ETARGET`。把 pin 降到 1.0.2 也不对：生成的代码
 * 面向 1.1.0 的 API，装上只会**编译失败**，比装不上更难诊断。
 * 所以这个值的用途不是改 pin，而是**让未发布这件事说出来**（见
 * `REGISTRY_PIN_IS_PUBLISHED`），并在发版后随 `package.json` 一起抬上去。
 *
 * 单一真源：取自本包 `package.json` 的 `tauron.publishedNpmVersion`，缺失时退回
 * `version`（同批发版时两者相同）。`@tauron/cli` 侧有同名口径，两者是否一致由
 * `wire-gate` 的「已发布版本单一真源」用例钉住。
 */
const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as {
  version: string;
  tauron?: { publishedNpmVersion?: string };
};

export const PUBLISHED_FRAMEWORK_VERSION: string = pkg.tauron?.publishedNpmVersion ?? pkg.version;

/**
 * 生成的工程现在能否从 registry 装到所钉版本。false 时 `init` 与脚手架输出都必须
 * 带一句可操作的说明（`--tauron-path` 或等发版），不能只留一个装不了的工程。
 */
export const REGISTRY_PIN_IS_PUBLISHED: boolean = PUBLISHED_FRAMEWORK_VERSION === FRAMEWORK_VERSION;
