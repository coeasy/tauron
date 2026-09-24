# Changelog

本文件记录 tauron 的显著变更。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> **关于「已知债务」**：本仓库刻意区分「真实实现」与「接口占位」。下面每一节
> 的「已知债务」都是**实测**结果（附复现命令），不是推测。请勿在发布公告里
> 把债务当已完成项宣传。

---

## [0.1.0] - 未发布

首个版本。框架层（`@tauron/types|core|plugin-sdk|adapter-*|cli|market|shell-matrix|dual-world|contract-tests` + `tauron-shell`）
与应用层（`@tauron/host|framework|ui|ui-primitives|app-*` + `tauron-host`/`tauron-adapter`）双栈成型。

### Added

**框架层**

- 单命令信封协议：`plugin_invoke` / `plugin_cancel` / `plugin_emit`
- 双世界隔离架构（`@tauron/dual-world`）；QuickJS-WASM 引擎接入仍为路线图项
- 4+1 插件形态：Rust / JS（真实实现）/ WASM / Process（配置 + 模拟原型）+ B+ 混合
- 双层 ACL：外层 Tauri 静态 ACL + 内层框架动态 ACL
- 每插件隔离的事件总线队列，`Event` 溢出丢最旧 / `Request` 硬失败 / `State` 只留最新；
  `MAX_QUEUE = 1000`，连续溢出 3 次熔断
- 配置 4 层合并（session > plugin > user > default）
- Shell 矩阵 4 形态（local / local-server / remote-url / sub-webview）
- 插件市场：HMAC-SHA256 验签、注册表搜索 / 发布
- CLI 工具链：`create` / `plugin new|dev|test|pack|sign|publish` / `doctor`
- React / Vue / Svelte 适配层

**应用层**

- `host_*` 命令族共 54 条（底座 38 + 插件运行时 16）
- 生命周期状态机：10 态 / 18 事件 / 9 守卫，表驱动 `TRANSITIONS`（表内顺序即优先级），
  `MAX_CHAIN_DEPTH = 2`
- 三档授权 `AuthTier{Self_, ScopedRead, Privileged}`；`self` 档的 pluginId 只从
  webview label（`plugin-<id>`）解析，**忽略调用方入参**（防冒充）
- 崩溃恢复 + 安全模式（启动失败计数、幂等快照、恢复向导）
- 设置中心（schema 注册表 + 四层合并 + get/set/watch）
- i18n 桥、通知中心、白标品牌化、主题/皮肤、schema 规范化管线
- 进程插件 host、WASM supervisor
- Lit Web Components UI 层与 `ui-primitives`

**跨语言契约**

- `@tauron/contract-tests` 的 wire-gate：直接正则解析 Rust 源码文本做断言，**125 条**
- 全部跨 IPC 边界的 Rust 结构体强制 `#[serde(rename_all = "camelCase", deny_unknown_fields)]`
- 双套错误码且零交集：框架层 `SC-xxxx`（14 个）/ 应用层 `E_*`（18 个）

**工程与发布基础设施**（本次补齐）

- `LICENSE`（MIT）
- `.github/workflows/ci.yml`：TS（build→typecheck→lint→test）、Rust 默认特性、
  Rust `tauri` feature、示例工程 `substrate-only`、wire-gate
- `.github/workflows/release.yml`：tag 版本一致性校验 → windows / macOS(arm64+x64) / linux
  矩阵构建 → 草稿 Release
- `Cargo.lock` 纳入版本控制（原先被 `.gitignore` 忽略，构建不可复现）
- `rustfmt.toml` / `clippy.toml` / `eslint.config.js` / `.prettierrc.json`
- 本文件

### Changed

- **15 个 crate 的 `[package]` 元数据统一为 workspace 继承**。此前 9 个 crate 硬编码
  `version = "0.1.0"` / `edition = "2021"`，另有 14 个缺 `repository` —— 升版时这些
  crate 会静默掉队，且 crates.io 元数据不完整。现在全部继承
  `version / edition / rust-version / license / repository / homepage`。
- `Cargo.toml` 的 `repository` 由占位符 `https://example.invalid/tauron` 改为真实地址。
- 20 个包的 `lint` 脚本由 `pnpm run typecheck` 改为 `eslint .`（原值名不副实：
  它不 lint 任何东西）。`@tauron/app-plugin-sdk` 原先根本没有 `lint` 脚本，已补。
- 根 `package.json` 新增 `lint` / `format` / `format:check` / `fmt:rust` /
  `fmt:rust:check` / `lint:rust` 脚本。

### Fixed

- **`tauron-notify` 的活锁风险**：`push` / `trim_to` 的驱逐循环在 `order` 与
  `entries` 失同步时会空转（宿主挂死）。加入收敛保护（一轮未减少即跳出），
  并补 2 个针对性测试。
- **`tauron-settings::merge::write_path` 的可达 panic**：点分路径经过标量中间节点时
  `expect` 会触发。改为按需把中间节点提升为对象，并补 2 个测试。
- **`tauron-theme::ThemeRegistry::default()` 返回空表**：缺省注册表不含内置主题，
  导致 `active_id` 指向不存在的主题。改为默认装载内置 light/dark，并补测试。
- **自动更新的 `relaunch()` 名不副实**：实现走的是 `host_window_quit`（只退出、
  应用不会自己回来），而不是 `host_window_relaunch`（会先把恢复引擎的阶段判定
  对账到插件侧再请求重启）。已改正，并补测试断言「走 relaunch 而非 quit」。
- **`bootstrap.ts` 头注释与代码错位**：注释写「7 阶段」，实际是 8 个阶段，漏掉
  了「上报本轮启动结果」——漏了它每次启动都会被计为崩溃。已补齐注释。
- **`tauron-ui-primitives/src/skeleton.ts` 的 `// @ts-nocheck`**：该指令让整个文件
  不做类型检查。移除后 `tsc --noEmit` **通过** —— 它白挡了检查，没有掩盖任何问题。
- **裸表达式语句**（`signals.test.ts` ×3、`animation-hook.ts`、`component-animation.ts`）：
  改为 `void expr` 并补上「为什么是刻意的」注释（向 effect 注册依赖 / 强制重排）。
- **`plugin-manager.test.ts` 的死变量**：`let callCount = 0` 从未被使用，已删除。
- **`@tauron/cli` 的 `create` / `plugin new` 谎报成功**：两条命令只调生成器就返回
  `Created app/plugin 'x' with N files`，而 `scaffold.ts` / `plugin.ts` **从不碰文件
  系统**——一个字节都没写。现在落盘由 `scaffold-writer.ts` 负责，消息按真实结果分
  三种（真写了 / `--dry-run` 只报告 / 目录已存在且未加 `--force` 时**如实失败**）。
  与轮 12 修掉的 `plugin dev/test/pack/publish` 属同一类缺陷。
- **`bin/tauron.js` 把参数过滤成了「凡以 `-` 开头就丢」**，导致两类静默失效：
  ① `tauron --version` / `--help` 永远回 `No command specified`（`runCli` 里明明有
  这两个分支）；② `--type=wasm` 被丢掉，`tauron plugin new x --type wasm` **静默产出
  js 插件**。改为白名单：只过滤本 wrapper 自己消费的 `--verbose/-v/--dry-run/--force`。
- **`plugin new --type` 只实现了 `--type=x` 一种写法**，与帮助文本写的
  `[--type js|process|wasm]`（空格形式）不符，且非法值被 `as any` 静默降级。
  现在两种写法都生效，非法值**如实失败**并列出可选值。

### 文档

- **删除已失效的历史文档**（4 份，均无引用或状态为假）：

  | 删除 | 原因 |
  |---|---|
  | `docs/architecture/client-foundation-upgrade-plan.md` | 2026-01 审计基线（TS 1122 / Rust 984），数字落后两代 |
  | `docs/architecture/tauron-substrate-refactor-plan.md` | R1–R8 已完成，文档头却标「设计稿（待执行）」——状态是假的 |
  | `docs/architecture/competitive-analysis.md` | 与详版重复，且含已失效声明（「架构文档缺失」「GitHub Actions 未确认」） |
  | `docs/api/openapi.yaml` | **旧品牌 `open-client` 的 HTTP API 规范**，与 tauron 信封协议无关，零引用 |

  耐久产出未丢：R1–R8 的架构决策与六条硬边界沉淀进 `overview.md` 新增的
  「架构演进」一节；canonical 结论在 `canonical-owners.md`。
- 竞品分析由带中文 + 日期的文件名改为稳定路径
  `docs/competitive-analysis/competitive-analysis.md`。
- **修正 `overview.md` 的过期接线状态**：原文写 `tauron-settings` / `tauron-proc`
  「尚未接入适配层运行时」，但两者都已在 `tauron-adapter/Cargo.toml` 里且被真实调用
  （`SettingsStore` / `CommandSpawner` / `CrashTracker`）。改为按依赖表分「已接线 /
  未接线」两栏列证据。
- **修正 `incremental-adoption.md` 的过期行**：原文写设置「接线进行中，`Cargo.toml`
  尚未声明 `tauron-settings`」——已声明，且 `host_settings_migrate` 已注册成命令。
  该行从「尚未接线」表移入「已接线」。
- `README.md`：文档表与项目结构树与实际文件对齐（补上此前漏列的
  `shell-events` / `ui-primitives` / `plugin-context-contract` 三个包）；「快速开始」
  与「CLI 工具」两节不再给出装不上的 `pnpm add @tauron/...` / `npx tauron`。

### 已知债务

以下问题**已知且未解决**，发布时不应被描述为已完成。

**1. Rust 格式化与 lint 从未跑过 → CI 中为 advisory**

```
cargo fmt --all -- --check     # 691 hunk / 59 文件（61 个 .rs 中）
cargo clippy --workspace --all-targets -- -D warnings   # 从未运行
```

代码库是按人工风格写的（对齐注释块、长解释性注释），从未做过 rustfmt 规范化。
`rustfmt.toml` 取的是实测差异最小的一档（`max_width = 100` + `use_small_heuristics = "Max"`）。
**转正条件**：一次性 `cargo fmt --all` + clippy 清理，并验证编译与测试后，
删掉 `ci.yml` 里对应 job 的 `continue-on-error`。

**2. Prettier 从未跑过**

`pnpm format:check` 当前会失败。CI 里没有 format 门禁，避免永久红。

**3. ESLint 有 82 条 warning（0 error）**

`pnpm lint` 退出码为 0，因此已作为**硬门禁**接入 CI。82 条 warning 绝大多数是
测试文件里的未用导入。**不要**加 `--max-warnings 0`，那会让仓库直接变红。

**4. Rust 测试未在有网络的环境执行过**

本仓库的 `cargo test --workspace` 结果以 CI 为准。README 徽章上的 Rust 数字
是源码 `#[test]` 声明数（静态计数），**不是**执行结果 —— feature 门控
（`tauron-adapter` / `tauron-shell` 的 `tauri` feature）另计，两者不同口径。

**5. `crates.io` 发布未就绪**

内部互引用写的是 `{ path = "crates/xxx" }`，cargo 要求 path 依赖同时给出
`version` 才能打包。在补齐 `version` 之前不要执行 `cargo publish`。

**6. npm 包全部 `private: true`**

20 个 `packages/*/package.json` 均为 `private`，无法 `npm publish`。

**7. 示例工程的打包图标不全**

`examples/minimal-app/src-tauri/icons/` 只有 `icon.ico`，Linux 打包需要 png。
CI 里 `example-substrate-only` job 因此设为 advisory。

**8. 命令面缺口**

host 命令面缺 menu / tray / fs / http 四域。

**9. 孤儿 crate**

未被任何其他 crate 依赖：`tauron-acl` / `tauron-brand` / `tauron-theme` /
`tauron-market` / `tauron-distribute` / `tauron-wasm` / `tauron-shell`（`tauron-shell`
是框架层门面，属有意为之；其余为待激活）。

**10. 测试期 peer 依赖告警**

`@testing-library/react-hooks@8` 声明 `react ^16.9 || ^17`，实际装的是 react 18；
`@testing-library/react@16` 缺 `react-dom` peer。属既有状态，不影响测试通过
（`pnpm peers check` 可复现）。

---

## 版本流程

1. 三处版本号一起升：
   - `Cargo.toml` 的 `[workspace.package] version`（15 个 crate 全部继承它）
   - 根 `package.json` 的 `version`
   - 20 个 `packages/*/package.json` 的 `version`
2. 在本文件顶部新增一节，把「未发布」改为实际日期
3. `git tag vX.Y.Z && git push origin vX.Y.Z`
4. `release.yml` 会先校验三处版本号与 tag 一致，再矩阵构建，最后落成**草稿** Release
   （人工过一眼再点发布）

[0.1.0]: https://github.com/coeasy/tauron/releases/tag/v0.1.0
