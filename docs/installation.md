# 安装与使用

本文是 tauron 的**落地入口**：怎么装、怎么跑、怎么验证装对了。

> **先看这一句，能省掉大半困惑**：本仓库交付的是**框架**（16 个 Rust crate +
> 20 个可发布 npm 包，另有 1 个私有契约测试包），不是一个可以直接下载双击运行的成品应用。仓库里**唯一能装的东西**
> 是 `examples/minimal-app` 这个**示例应用**——它的作用是把框架的链路真正跑起来给人看。
>
> **registry 现值（2026-10-02 实测，别按本仓库版本号去装）**：npm 侧 20 个公开包的
> `latest` 都是 **1.0.2**，crates.io 侧 `tauron-*` 的 `max_version` 也是 **1.0.2**
> （15 个——`tauron-ffi` 是 2026-09-29 才加的，registry 上还没有它）。本仓库的
> `1.1.0` 已通过 `pnpm publish:npm -- --check`（本机 2026-10-02 实跑，20/20 通过），但
> **尚未执行发布**，所以 `@tauron/*@1.1.0` 与 `create-tauron-app@1.1.0` 现在装不到。

---

## 0. 先分清几种「安装」

「装 tauron」在不同语境下指完全不同的事，先对号入座再看后面：

| # | 你想做什么 | 用哪一节 | 现在能不能做 |
|---|---|---|---|
| A | **跑起来看看**（拿安装包装上就开） | §1 | ⚠️ 部分可以：Windows NSIS **出包**实测过；macOS / Linux 的包 CI 能出，但三种平台的包**都未做过真机安装验证**（出包 ≠ 装过） |
| B | **改代码 / 自己构建** | §2 | ✅ 可以，源码可构建 |
| C | **把 tauron 当库接进自己的项目** | §3 | ✅ 可以，但装到的是 **1.0.2**（registry 现值）；1.1.0 发布后才与本仓库一致 |
| D | **一键脚手架起新工程**（`tauron-app new`） | §3.2 | ✅ `create-tauron-app` 在 npm 上可运行，`latest` 是 **1.0.2**；`@1.1.0` **尚未发布**，现在点名安装会 `ETARGET` |

---

## 1. 安装示例应用

### 1.1 下载

到 [Releases](https://github.com/coeasy/tauron/releases) 页面，在最新版本的
**Assets** 里按平台取文件。命名形如 `Tauron Minimal App_1.1.0_<平台>.<后缀>`：

| 平台 | 取哪个文件 | 格式 |
|---|---|---|
| Windows x64 | `..._x64-setup.exe` | NSIS 安装包 |
| macOS（Apple Silicon） | `..._aarch64.dmg` | 磁盘映像 |
| macOS（Intel） | `..._x64.dmg` | 磁盘映像 |
| Linux | `..._amd64.deb` / `..._x86_64.rpm` / `..._amd64.AppImage` | 三种任选 |

> **发布状态**：Release 由 `release.yml` 在推 `v*` tag 时自动构建并创建公开版本，
> Windows、macOS 和 Linux 的安装包会作为 Release assets 上传。
>
> **当前实测（轮 50）——现在去 Releases 还下载不到任何东西**：GitHub Releases 列表为空，
> 尽管 tag `v1.1.0` 已经存在。拦它的是这条链自己的 `registry-check`：
> `node scripts/check-published-versions.mjs` 判「37 项在 1.1.0 上未通过核验」（npm 缺 20、
> crates.io 缺 9、另 8 项因网络超时未能核实）。registry 里没有 1.1.0，就不会生成一个
> 装着不存在之物的 Release。因此**在 npm + crates.io 发布完成之前，唯一可用的安装路径是
> §2.3 的源码构建**；上表的文件名是发布后的命名约定，不是当前可下载清单。
> 另有一条待决：`v1.1.0` 标签指向的是轮 13–18 的提交，而 main 已推进到轮 22–50——
> 发布前必须让「registry 里的源码」与「tag」是同一份（另起版本号，或显式移标签，后者属破坏性操作）。
>
> **Windows 只提供 NSIS，不提供 MSI**：`targets: "all"` 在 Windows 上等于
> nsis + msi，而 MSI 需要构建期下载 WiX 工具链，属额外网络依赖，失败时会连
> NSIS 一起拿不到。原因与转正条件登记在 [CHANGELOG 的「已知债务」第 11 条](../CHANGELOG.md)。
>
> **跨平台边界（如实说明）**：`release.yml` 的构建矩阵**有** Linux（deb / rpm /
> AppImage）与 macOS（aarch64 / x64 的 dmg 与 .app）两条腿，**也有** Windows 腿——
> 但 CI 只保证「能出包」，**三种平台的包都没有做过真机安装验证**：Windows 的 NSIS
> 包本机只实测到 `tauri build --bundles nsis` **出包成功**这一步（出包 ≠ 装过）。
> 另外 `--bundles nsis,msi` 这类组合**未在 CI 验证**（Windows 腿固定写 `--bundles nsis`）。

### 1.2 Windows

1. 双击 `Tauron Minimal App_1.1.0_x64-setup.exe`。
2. 会弹 **Windows SmartScreen**（「Windows 已保护你的电脑」）——因为安装包
   **没有代码签名**。点「更多信息」→「仍要运行」。
3. 按向导装完，从开始菜单启动。

卸载：设置 → 应用 → 「Tauron Minimal App」→ 卸载。

### 1.3 macOS

1. 打开 `.dmg`，把 **Tauron Minimal App** 拖进「应用程序」。
2. 首次打开会被 **Gatekeeper** 拦下（「无法打开，因为无法验证开发者」）——
   因为应用**没有签名、没有公证**。二选一：
   - **右键点图标 → 打开 → 再点「打开」**（推荐，只需一次）；
   - 或在终端里去掉隔离属性：
     ```bash
     xattr -dr com.apple.quarantine "/Applications/Tauron Minimal App.app"
     ```
3. 装的是哪个架构要看清：Apple Silicon 用 `aarch64`，Intel 用 `x64`。
   装错了能启动但会走 Rosetta 或直接报架构不符。

卸载：把「应用程序」里的 App 拖进废纸篓。

### 1.4 Linux

**Debian / Ubuntu（`.deb`）**

```bash
# 用 apt 装本地 deb，它会自动补依赖（比 dpkg -i 省事）
sudo apt install ./Tauron*_amd64.deb
```

**Fedora / RHEL（`.rpm`）**

```bash
sudo rpm -i ./Tauron*x86_64.rpm
# 或
sudo dnf install ./Tauron*.rpm
```

**AppImage（免安装）**

```bash
chmod +x ./Tauron*_amd64.AppImage
./Tauron*_amd64.AppImage
```

**系统依赖**：deb/rpm 会把 `webkit2gtk-4.1` 系列作为依赖自动拉上。AppImage
不打这些依赖，若启动报缺库，手动补：

```bash
sudo apt install libwebkit2gtk-4.1-0 libjavascriptcoregtk-4.1-0 libgtk-3-0
```

卸载：deb → `sudo apt remove tauron-minimal-app`；rpm → `sudo rpm -e tauron-minimal-app`；
AppImage → 直接删文件。

### 1.5 装完怎么确认「真的通了」

启动后你会看到一个主窗。下面四条是快速验收清单；插件管理、命令面板、启动恢复和跨主体调用也可以继续检查：

| 点它 | 应该看到 | 背后走的链路 |
|---|---|---|
| **格式化（沙箱插件）** | 文本被规整（多余空白被压缩） | 宿主 → iframe 插件握手（token 经 URL hash 注入）→ postMessage 调用 → 返回 |
| **最小化窗口** | 窗口真的最小化 | 前端 → `host_window_minimize` → Tauri `window.minimize()` |
| **读剪贴板** | 弹出剪贴板当前内容 | 前端 → `host_clipboard_read`（进程内真实读取） |
| **检查更新** | 宿主没注入更新端点时如实答"更新通道答不了"；注入了才报"有/没有新版本" | 前端 `AutoUpdateClient.checkUpdate()` → `host_updater_check`（**真通道**；示例的 `currentVersion` 取自本示例 `package.json`） |

> **示例里仍然有刻意保留的诚实边界**：下载与安装两条是**桩**，返回带 `simulated: true`，
> 前端见到它**不得**推进状态机。这不是 bug，也不是「已实现」——本项目刻意区分
> 「真实实现」与「接口占位」，不伪造成功。看到 `simulated: true` 说明行为正确。
>
> ⚠️ **"检查有没有更新"与"把更新装上"是两件事**（轮 29 定口径，轮 31 补齐壳层）：
> `host_updater_check` / `host_updater_status` 在宿主注入 `EndpointClient` 后**真跑**
> 清单校验 + 灰度 + 签名判定；SDK 的 `AutoUpdateClient.checkUpdate()`（轮 29）与
> `<oc-updater-dialog>` 的「检查更新」经 `ShellController`（轮 31）**共用同一份判定**
> （`toUpdateInfo`）。`host_market_check` 仍是宿主本地桩，且**按设计**不读调用方下发的
> 更新源参数——拿它的 `available` 当更新结论就是这两轮修掉的断链。下载/安装两条仍是桩，
> 所以"能如实报有更新"与"能把更新装上"今天仍是两件事。
>
> 报"没有更新"时会带一句**依据**（轮 33）：`available: false` 至少对应四种情况——已是
> 最新、不在灰度批次、被崩溃门禁停发、端点未装配，后三种只在 `host_updater_status` 里，
> 两条入口现在都会把它并进 `info.reason`（`灰度 30%｜崩溃门禁未停发｜…`）后打印出来。
> 其中宿主账本会写成 `宿主账本 installed:2.0.0（模拟推进，未真的装上）`——线字段
> `stateSimulated` 与 `state` 成对读，桩推进的账本不会被读成"真的装上了"。
>
> 检查之后按钮上写着什么，也不是组件自己决定（轮 32）：状态取值来自
> `@tauron/shell-events` 的 `UPDATER_STATUSES`，主按钮动作由 `UPDATER_PRIMARY_ACTION`
> 推导——`error`（含"更新通道答不了"）与 `idle` 都是「检查更新」，`available` 才是
> 「开始更新」，而「立即重启」**只在** `ready` 出现。今天宿主桩走不到 `ready`，
> 所以页面上看不到重启按钮属于预期，不是又一处断链。

如果四条都能点出反应，说明 **Rust 命令层 ↔ Tauri IPC ↔ 前端适配层** 这条主链是通的。

### 1.6 数据目录

应用会把恢复计数与设置落在系统配置目录：

| 平台 | 路径 |
|---|---|
| Windows | `%APPDATA%\com.tauron.minimal-app\` |
| macOS | `~/Library/Application Support/com.tauron.minimal-app/` |
| Linux | `~/.config/com.tauron.minimal-app/` |

删掉这个目录即可回到全新状态（崩溃恢复计数、设置都会被清空）。

---

## 2. 从源码构建

### 2.1 工具链

| 工具 | 版本要求 | 说明 |
|---|---|---|
| Node.js | **>= 22** | 根 `package.json` 的 `engines` 声明 |
| pnpm | **11.7.0** | 根 `package.json` 的 `packageManager` 钉死；CI 用同一版本 |
| Rust | **>= 1.98**（stable） | 根 `Cargo.toml` 的 `rust-version` |
| Tauri CLI | v2（无需预装） | 走 `pnpm dlx @tauri-apps/cli@2`，按需下载 |

### 2.2 平台系统依赖

**Linux**（Tauri 必需）：

```bash
sudo apt install -y \
  libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev libsoup-3.0-dev \
  libgtk-3-dev librsvg2-dev patchelf file
# 要打 rpm 再加：rpm
```

**macOS**：`xcode-select --install`（Xcode Command Line Tools）。

**Windows**：Visual Studio Build Tools（含 MSVC 与 Windows SDK）+ WebView2
（Win10/11 一般已内置）。

### 2.3 构建示例应用

```bash
# 1) 装依赖（workspace 级）
pnpm install

# 2) 先构建所有 TS 包 —— 这一步不能省！
#    各包之间以 workspace:* 互引，测试/构建是从 dist/ 解析导入的；
#    不先 build 会得到一批 "Failed to resolve import" 的假失败。
pnpm -r build

# 3) 构建桌面应用（含前端产物 + Rust + 打包）
cd examples/minimal-app
pnpm tauri build
```

产物位置：

| 平台 | 路径 |
|---|---|
| Windows | `examples/minimal-app/src-tauri/target/release/bundle/nsis/*.exe` |
| macOS | `examples/minimal-app/src-tauri/target/release/bundle/{macos,dmg}/` |
| Linux | `examples/minimal-app/src-tauri/target/release/bundle/{deb,rpm,appimage}/` |

> **只想出 NSIS、不想被 WiX 拖累**：`pnpm tauri build --bundles nsis`。
> 这正是 `release.yml` 的 Windows 腿采用的方式（见 §1.1 的说明）。

### 2.4 开发模式（改代码看效果）

```bash
# 终端 1：前端 dev server（vite，双入口：index.html / plugin.html）
cd examples/minimal-app && pnpm dev

# 终端 2：Tauri 开发窗口（会自动连上面的 devUrl :5173）
cd examples/minimal-app && pnpm tauri dev
```

### 2.5 只验证前端（不需要 Tauri 运行时）

```bash
pnpm --filter minimal-app build
```

---

## 3. 把 tauron 接进自己的项目

### 推荐路径：用户不需要逐个安装 20 个 npm 包或 15 个 Rust crate

包数是 Tauron 仓库的发布/维护单元，不是应用开发者的必装清单。直接集成时只需选入口：

| 目标 | 前端直接依赖 | Rust 直接依赖 | 最短路径 |
|---|---|---|---|
| 新建 Tauri 2 客户端 | 脚手架生成 `@tauron/host` | 脚手架生成 `tauron-adapter` | `npm create tauron-app@latest -- ./my-app --framework react`，然后 `cd my-app && npm install` |
| 接入已有 Tauri 2 客户端 | CLI 自动加 `@tauron/host` 与 `@tauron/ui` | CLI 自动加 `tauron-adapter` | `npx @tauron/app-cli@latest init --dir ./my-app`，再按 CLI 提示安装依赖 |
| 手工集成 | 从 `@tauron/host` 开始；需要组件时再加 `@tauron/ui` | 只加 `tauron-adapter` 并启用 `tauri` feature | 只需安装直接使用的包；npm / Cargo 会解析 Tauron 的传递依赖 |

`tauron-adapter` 是 Rust 侧聚合入口，下面 10+ 个内部 crate 会作为传递依赖处理；前端包同样会自动解析内部包依赖。React、Vue、Svelte 是脚手架选择的前端方案，用户不需要同时安装三套框架适配器。需要更细粒度控制时，再按后续章节逐包集成。

### 3.0 五种「安装/集成」方式一览（先说边界）

「支持多种安装方式」拆开是下面五条路径：

| # | 方式 | 具体做法 | 现在能不能用 | 依据 |
|---|---|---|---|---|
| 1 | **源码集成** | `path` / `file:` 指向本机 tauron checkout | ✅ 可用于源码联调 | §3.3 |
| 2 | **npm 包集成** | `npm i @tauron/...` | ✅ 20 个公开包可安装，registry `latest` = **1.0.2** | §3.4 |
| 3 | **crate 集成** | `cargo add tauron-*` | ✅ crates.io 上 15 个 `tauron-*` = **1.0.2**（`tauron-ffi` 尚未发布）；干净消费工程见 §3.7 | §3.5 |
| 4 | **一键脚手架** | `npm create tauron-app` | ✅ 可运行（`latest` = **1.0.2**）；点名 `@1.1.0` 现在会 `ETARGET` | §3.2 |
| 5 | **安装包** | 从 Releases 取 Tauri bundle 装上即用 | ⚠️ 仅限**示例应用**；Windows 腿 `--bundles nsis` 出包实测过，**装机未验证** | §1 |

### 3.1 发布状态

**registry 现值与仓库版本不是一个数，这里分开写。**

- **registry 现值**（2026-10-02 实测：`npm view <包> version` / crates.io `max_version`）：
  npm 侧 20 个公开包的 `latest` 全是 **1.0.2**；crates.io 侧 **15** 个 `tauron-*` 的
  `max_version` 是 **1.0.2**。第 16 个 crate `tauron-ffi`（2026-09-29 才加进 workspace）
  registry 上还没有，它会随 1.1.0 首发。
- **仓库版本 1.1.0**：`pnpm publish:npm -- --check` 本机实跑通过（20/20 个可发布包
  逐一 `pnpm pack` + 解包校验），`cargo` 侧的内部互引用也已全部同时给出 `version`，
  但**发布动作（`--publish`）尚未执行**。点名 `@1.1.0` 现在会拿到
  `npm ETARGET — No matching version found`，这正是
  `node scripts/verify-registry-consumer.mjs` 当前唯一红灯的直接原因
  （该脚本故意从仓库读版本、不写死，就是为了不发版时**必然**红，而不是假装绿）。

- **npm**：21 个 package manifests 中 **20 个可发布**并补齐发布元数据（`license` /
  `repository` / `homepage` / `bugs` / `keywords` / `engines` / `publishConfig.access=public`
  / `files` 白名单）；第 20 个 `@tauron/contract-tests` **刻意保留 `private`**
  （它是仓库内的契约测试 harness，`dist/` 里只有 `*.test.js`、没有 `index.js`，
  用例还依赖 monorepo 目录布局，发布出去对第三方无意义）。
- **crates**：15 个 crate 的内部互引用都已同时给出 `version`；发布检查会从每个 `.crate`
  tarball 实际构建，并断言产物 manifest 里 `path` 已被 cargo 剥离，只剩 `version`。

发布编排脚本、发布状态与验收见 §3.7。

### 3.2 一键脚手架（最快路径）

可从 npm 直接启动脚手架：

```bash
# npm create 会运行 create-tauron-app 包，生成已接线的 Tauri 2 工程
npm create tauron-app@latest -- ./my-app --framework react

# 已有 Tauri 2 项目：注入接入
npx @tauron/app-cli@latest init --dir ./my-existing-app

# ⚠️ 别点名 @1.1.0：registry 上的 latest 目前是 1.0.2，1.1.0 还没发布，
#    点名会得 `npm ETARGET — No matching version found`。
```

在 Tauron 仓库中进行本地源码开发时，可运行 `node packages/tauron-app-cli/dist/cli.js new ./my-app
--tauron-path <checkout>`；该选项明确生成本地 `path` / `file:` 依赖。

**能一键跑通到哪一步**：

| 产出 | 状态 |
|---|---|
| `src-tauri/`（`Cargo.toml` / `main.rs` / `build.rs` / `capabilities/default.json` / `tauri.conf.json`） | ✅ 真装配，与 `examples/minimal-app` 同源 |
| 依赖坐标 | ⚠️ **与本仓库同版本**：1.1.0 的 CLI 生成 `@tauron/host = "1.1.0"` + `tauron-adapter = "=1.1.0"`（本机实测产物），而 registry 现值只有 **1.0.2**——所以**发布前**用仓库内 CLI 生成的工程 `npm install` / `cargo check` 会失败。发布后此条自动成立；发布前请改用 `npm create tauron-app@latest`（1.0.2 的生成器钉 1.0.2，可正常安装）或 `--tauron-path` 走本地源码 |
| 前端 bundler / dev-server 配置 | ✅ 生成 `vite.config.ts`（端口/产物目录与 `tauri.conf.json` 对齐）+ 根 `index.html` + `beforeDevCommand` / `beforeBuildCommand` |
| `src-tauri/icons/` | ⚠️ 生成**纯色占位图**（6 个文件，覆盖 Windows / macOS / Linux 打包所需）；**发布前须替换成品牌图标**。缺这组文件连 `cargo check` 都过不去（`tauri-build` 在 Windows 上要 `icons/icon.ico`） |

生成后的下一步：

```bash
cd my-app && npm install && npm run tauri dev
```

> **实测（2026-10-01）**：生成物（含占位图标与 vite 配置）开箱即可编译——默认形态
> （85 条：示例应用自己的 `default = ["plugin-install", "runtime-wasm-broker"]`）与
> `--features substrate-only`（61 条）两档 `cargo check` **均通过**。
>
> **仅覆盖 Rust 侧**：`npm install` / `npm run tauri dev` 未在本机验证（要从 registry
> 取包，沙箱无网络）。前置条件两条：本机 tauron 检出已 `pnpm install`、且
> `packages/*/dist` 已构建——因为 `file:` 依赖是软链，`@tauron/host` 自解析其
> `workspace:*` 依赖走的是检出根自己的 `node_modules`。

`init` 仅自动接入 Tauri 2。若目标项目已有 `.invoke_handler(..)`，CLI 保留宿主源码并报告手工
合并 Tauron 命令的步骤；检查未完成会返回非零状态。已有 `client-config.json` 与 capability
文件也会保留。修改 `package.json` 后需运行项目所用包管理器的 install 命令。

### 3.3 方式 1：源码集成（`path` / `file:`）

**前端（TS 包）**：

```bash
# 同一个 pnpm workspace 内：直接用 workspace 协议
pnpm add @tauron/types@workspace:* @tauron/core@workspace:*

# 跨仓库：file: 指向本机 checkout 的 packages/<包名>（一键脚手架用的就是这一招）
pnpm add @tauron/types@file:../tauron/packages/types
```

**Rust 侧（crate）**：

```toml
# src-tauri/Cargo.toml
[dependencies]
tauron-shell = { path = "../tauron/crates/tauron-shell", features = ["tauri"] }
# 应用层整套（83 条；再加 features = ["plugin-install"] 才是 85 条）：
# tauron-adapter = { path = "../tauron/crates/tauron-adapter", features = ["tauri"] }
```

> `file:` / `path` 都是**指向本机 checkout**，不会从 registry 取包。适合修改 Tauron 源码或联调；
> 普通新项目优先用上面的 registry 脚手架。代价：接入方的依赖里出现了一个本地绝对/相对路径，
> 换机器、换目录都要重新对齐。

完整的 Rust 侧命令注册与前端运行时初始化代码见
[README 的「快速开始」](../README.md#快速开始第三方集成)。

### 3.4 方式 2：npm 包集成

20 个公共 npm 包可以通过常规包管理器安装（**registry 现值 = `1.0.2`**，见 §3.1）：

- 20 个可发布包已补齐 `license` / `repository` / `homepage` / `bugs` /
  `keywords` / `engines`（`>=22`，与 README 徽章一致）/ `publishConfig.access=public`
  与 `files` 白名单（至少 `dist` + `README.md`）。
- 顺带修掉一处发布缺陷：8 个包原本 `main` 指向 `./dist/index.cjs`，而构建**从不产出
  任何 `.cjs`**（`tsc` 单趟只出 ESM）；已改指真实入口 `./dist/index.js`。
- `pnpm publish:npm -- --check` 本机跑通：对 20 个包逐一 `pnpm pack` 并**解包校验**
  「含 `dist/`、入口文件齐全、版本同源、无 `workspace:` 残留」。

**发布必须用 pnpm（不是 npm）——这是实测结论**：`npm pack` 会把 `workspace:*`
**原样写进 tarball 的 `package.json`**，而 npm 在 workspace 之外解析 `workspace:`
会直接报 `EUNSUPPORTEDPROTOCOL`；`pnpm pack` / `pnpm publish` 则会把它改写成具体
版本（本机解包实测：`"@tauron/types": "1.1.0"`）。发布脚本因此固定走 pnpm，
且把「无 `workspace:` 残留」作为硬校验项。

发布后用法：

```bash
pnpm add @tauron/types @tauron/core @tauron/host   # npm / yarn 同理
```

真实发布状态以 [npm 包列表](https://www.npmjs.com/org/tauron) 和 [正式 GitHub Release](https://github.com/coeasy/tauron/releases) 为准。

### 3.5 方式 3：crate 集成

15 个 crate 均已通过 `cargo package` 检查：产物 manifest 里内部依赖已只剩 `version`
（`path` 由 cargo 剥离）。本次发布工作流按依赖顺序上传，完成后再由 Windows 干净消费者工程实际安装构建。

- ✅ **本机已验证**：`cargo package -p <crate> --allow-dirty --offline` 对全部 15 个 crate
  产出并构建 `.crate`，且产物内 `[dependencies.tauron-*]` 只剩 `version = "1.1.0"`、没有 `path`。
- ⏳ **CI 发布与验收**：由配置了发布凭据的 GitHub Actions 执行；失败可从断点安全续发。

**发布必须按依赖拓扑顺序逐个来**（被依赖者先发）——`cargo publish` 剥离 `path` 后
会去 crates.io 解析内部依赖，被依赖者不在 registry 上就解析失败。顺序由
`scripts/publish-crates.mjs` 从 `cargo metadata` **运行时实算**（不写死），实测序列见 §3.7。

发布后用法：

```toml
[dependencies]
# 应用层：整套 host_* 命令（默认 83 条；要装插件加 features = ["tauri", "plugin-install"] → 85 条）
tauron-adapter = { version = "=1.0.2", features = ["tauri"] }
# 或框架层：只要信封协议 3 条命令
tauron-shell = { version = "=1.0.2", features = ["tauri"] }
```

> 上面的 `=1.0.2` 是**今天能从 crates.io 解析到的版本**（2026-10-02 实测）。
> 本仓库的 1.1.0 发布后请改回 `=1.1.0`；注意 1.1.0 把 `tauron-adapter` 的
> `plugin-install` 从默认特性**改成了 opt-in**，所以照旧写 `features = ["tauri"]`
> 拿到的命令面是 83 条而不是 85 条。

### 3.6 三档装配——先决定你要哪一档

tauron 的装配是**分档**的，别一上来就全接：

| 档位 | 拿到什么 | 命令面 | 适用 |
|---|---|---|---|
| **只取底座** | 窗口/剪贴板/对话框/事件/设置/恢复/i18n/通知/品牌/主题，以及菜单/托盘/文件系统/HTTP/更新通道五个宿主能力域 | 61 条 | 不跑插件系统的普通客户端 |
| **底座 + 插件运行时** | 上面 + 注册表/生命周期/流式调用 | 83 条（要装插件再显式开 `plugin-install` → **85**） | 要装插件 |
| **完整客户端** | 上面 + 设置中心/白标/主题/UI 组件 | 83 条 + UI 层（同上） | 交付完整产品 |

> 命令面口径：底座 `tauron_substrate_handler!` **61** 条；`tauron_plugin_handler!` 在其上
> 加插件运行时 **22** 条 = **83**；`plugin-install` feature 另加 `host_registry_install` /
> `host_registry_install_preview` **2** 条，该 feature 是 **opt-in**
> （`crates/tauron-adapter/Cargo.toml` 的 `default = []`，V4 minimal-substrate 规则），
> 显式开启后共 **85** 条。1.0-W6 曾把它放进默认特性，1.1 已改回 opt-in——凡读到
> 「默认 85 条」的旧表述，以 `Cargo.toml` 与本段为准。

逐档的依赖清单、装配代码与注意事项见
[渐进接入指南](./integration/incremental-adoption.md)——**这是集成方的第一入口**。

### 3.7 发布编排与验收

两个脚本都是 **Node 22、零新依赖、默认 `--check`（只校验不发布）**：

| 命令 | `--check` 做什么 | 真发布（显式，缺 token 即拒绝） |
|---|---|---|
| `pnpm publish:npm -- --check` | `pnpm -r build` 后逐包 `pnpm pack`，**解包**校验 tarball 内容 | `pnpm publish:npm -- --publish`，需 `NPM_TOKEN` |
| `pnpm publish:crates -- --check` | 按 `cargo metadata` 实算的拓扑序逐 crate `cargo package`，**解包**校验产物 manifest | `pnpm publish:crates -- --publish`，需 `CARGO_REGISTRY_TOKEN` |

**本机实测（2026-09-27，Windows，Node 25 运行时 / 目标 Node 22 语法）**：

- npm：20 个可发布包**全部通过**；`@tauron/contract-tests` 按设计跳过（`private`）。
- crates：15 个 crate**全部通过**，产物 `Cargo.toml` 的 `[dependencies.tauron-*]`
  已剥成 `version`、无 `path`。
- crates `--check` 会**临时**用 `--config patch.crates-io.<dep>.path=…` 把内部依赖指回
  本地，仅为在**无网络**下让 cargo 完成依赖解析；脚本会解包**断言该 patch 不影响产物内容**。

**crates 发布顺序（脚本运行时实算，不写死；2026-09-27 实测序列）**：

```
tauron-host → tauron-acl → tauron-brand → tauron-distribute → tauron-i18n
→ tauron-market → tauron-notify → tauron-proc → tauron-recovery → tauron-schema
→ tauron-settings → tauron-theme → tauron-wasm → tauron-adapter → tauron-shell
```

> 顺序不是「按字母」而是**拓扑序**：11 个叶子 crate 无内部依赖可任意先发；
> `tauron-acl` / `tauron-market` 依赖 `tauron-host`，`tauron-settings` 依赖
> `tauron-schema`，`tauron-adapter` 依赖其余 12 个——所以 adapter 必须排在最后一批。
> 脚本每次运行都从 `cargo metadata` 重算，**新增/删除内部依赖不会让顺序漂移**。

**当前状态（2026-10-02 实测）**：npm 20 个公开包的 `latest` 都是 **1.0.2**（逐个
`npm view <包> version` 量得）；crates.io 上 **15 个** `tauron-*` 是 **1.0.2**（逐个
`cargo add <crate> --dry-run` 量得），`tauron-ffi` 一个版本都没有——它是 2026-09-29
才进 workspace 的，1.0.2 那班车没带上它。本仓库的 **1.1.0 已备好但 `--publish` 尚未
执行**，所以 `check-published-versions.mjs 1.1.0` 与 `verify:registry-consumer` 两条
门禁今天必红（前者逐个 404，后者 `ETARGET create-tauron-app@1.1.0`）。Rust crate
tarball 已修正并且全部通过实际构建验收，续发按上面的拓扑序跑
`node scripts/publish-crates.mjs --publish` 即可（含 `tauron-ffi` 的首发）。

---

## 4. 已知限制（诚实清单）

装之前先知道这些，能省掉大量「是不是坏了」的排查：

| 限制 | 说明 |
|---|---|
| **安装包未签名 / 未公证** | Windows 会弹 SmartScreen，macOS 会被 Gatekeeper 拦。**不是**安装包坏了 |
| **Windows 只有 NSIS，没有 MSI** | 见 §1.1 与 CHANGELOG 债务 #11；`--bundles nsis,msi` 未在 CI 验证 |
| **仓库版本 1.1.0 尚未发布** | registry 现值是 1.0.2（npm 20 包 / crates.io 15 crate，`tauron-ffi` 未发）。点名 `@1.1.0` 会 `ETARGET`，`check-published-versions.mjs 1.1.0` 与 `verify:registry-consumer` 因此必红——这是**发布状态**门禁，不是主干坏了 |
| **跨平台安装体验** | 示例应用安装包需要完成签名、公证和真机安装验证，见 §1 / §3.7 |
| **示例应用是示例** | 界面极简，提供插件调用、窗口、系统能力、恢复与命令面板等演示链路，不是产品形态的客户端 |
| **更新检查是模拟的** | 返回带 `simulated: true` 的响应，不真连更新服务器 |
| **图标是示例图标** | 由 `app-icon.svg` 生成的几何标记，非正式品牌资产 |
| **安装包未做真机安装验证** | CI 能出包（Windows / Linux / macOS 三条腿），但「装完能不能用」**三个平台都没验证过**；Windows 本机只到「出包成功」这一步 |

---

## 5. 常见问题

**Q：Releases 页面什么都没有。**
可能还没有推过 `v*` tag，或多平台构建仍在进行。推送 tag 后，矩阵构建成功会自动创建公开
Release 并附上安装包；想自己出包看 §2。

**Q：`pnpm -r test` 报一堆 "Failed to resolve import"。**
没先 `pnpm -r build`。测试是从各包的 `dist/` 解析 workspace 导入的。
正确顺序：**build → typecheck → lint → test**（根 `pnpm verify` 就是这个顺序）。

**Q：`tauri build` 卡在 `Downloading .../wix314-binaries.zip` 然后失败。**
那是 Windows 上 MSI 打包要的 WiX 工具链在下载。改用
`pnpm tauri build --bundles nsis` 跳过 MSI。

**Q：macOS 提示「应用已损坏，无法打开」。**
未公证导致的隔离属性，不是文件损坏。见 §1.3 的 `xattr` 命令。

**Q：Linux 启动报缺 `libwebkit2gtk-4.1.so.0`。**
装 §1.4 列的系统依赖；AppImage 不打包这些库。

**Q：示例应用启动后一片空白。**
先确认第 2.3 步的 `pnpm -r build` 跑过——`tauri.conf.json` 的 `frontendDist`
指向 `../dist`，那个目录是构建产物、不入版本库。

**Q：`cargo test` 和 README 徽章上的 Rust 数字对不上。**
徽章是源码 `#[test]` 的**声明数**（含 feature 门控用例），不是执行结果。
两个口径的区别见 [README 的「测试」一节](../README.md#测试)。

---

## 6. 相关文档

| 想了解 | 看这个 |
|---|---|
| 这是什么、成熟度如何 | [README](../README.md) |
| 三档装配怎么选、怎么接 | [渐进接入指南](./integration/incremental-adoption.md) |
| 写插件 | [插件开发指南](./api/plugin-development-guide.md) |
| 宿主配置文件（`ClientConfig`） | [客户端配置参考](./api/client-config.md) |
| 架构与关键设计决策 | [架构概览](./architecture/overview.md) |
| `host_*` 线格式协议 | [应用层线格式协议](./architecture/app-layer-wire.md) |
| 版本变更与已知债务 | [CHANGELOG](../CHANGELOG.md) |
