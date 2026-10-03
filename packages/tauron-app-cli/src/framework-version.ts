/**
 * 脚手架生成的依赖固定到这里，别在生成器里另写字面量。
 *
 * 为什么值得单一真源：这三条 pin（`@tauron/host`、`tauron-adapter`、
 * `tauron.plugin.json` 的 `framework`）必须与发版版本一致，写死的后果是发版后
 * 生成的工程仍在装**上一个**版本并「构建通过」——那比失败更危险。
 * `framework-version.test.ts` 把它钉在本包 `package.json` 上，改版本没改这里就红。
 */
export const FRAMEWORK_VERSION = '1.1.0';
