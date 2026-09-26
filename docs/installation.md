# 安装与使用

本文是 tauron 的**落地入口**：怎么装、怎么跑、怎么验证装对了。

> **先看这一句，能省掉大半困惑**：本仓库交付的是**框架**（15 个 Rust crate +
> 20 个 npm 包），不是一个可以直接下载双击运行的成品应用。仓库里**唯一能装的东西**
> 是 `examples/minimal-app` 这个**示例应用**——它的作用是把框架的链路真正跑起来给人看。

---

## 0. 先分清三种「安装」

「装 tauron」在不同语境下指三件完全不同的事，先对号入座再看后面：

| # | 你想做什么 | 用哪一节 | 现在能不能做 |
|---|---|---|---|
| A | **跑起来看看**（拿安装包装上就开） | §1 | ✅ 可以，Release 有各平台安装包 |
| B | **改代码 / 自己构建** | §2 | ✅ 可以，源码可构建 |
| C | **把 tauron 当库接进自己的项目** | §3 | ⚠️ 可以，但**只能 path/workspace 接入**——尚未发布到 npm / crates.io |

---

## 1. 安装示例应用

### 1.1 下载

到 [Releases](https://github.com/coeasy/tauron/releases) 页面，在最新版本的
**Assets** 里按平台取文件。命名形如 `Tauron Minimal App_0.1.0_<平台>.<后缀>`：

| 平台 | 取哪个文件 | 格式 |
|---|---|---|
| Windows x64 | `..._x64-setup.exe` | NSIS 安装包 |
| macOS（Apple Silicon） | `..._aarch64.dmg` | 磁盘映像 |
| macOS（Intel） | `..._x64.dmg` | 磁盘映像 |
| Linux | `..._amd64.deb` / `..._x86_64.rpm` / `..._amd64.AppImage` | 三种任选 |

> **发布状态**：Release 由 `release.yml` 在推 `v*` tag 时自动构建，产物先落成
> **草稿**（`draft: true`），需要维护者人工过一眼再点发布。所以如果你在 Releases
> 页看不到东西，是因为草稿还没发布——见 §5「没有安装包可下怎么办」。
>
> **Windows 只提供 NSIS，不提供 MSI**：`targets: "all"` 在 Windows 上等于
> nsis + msi，而 MSI 需要构建期下载 WiX 工具链，属额外网络依赖，失败时会连
> NSIS 一起拿不到。原因与转正条件登记在 [CHANGELOG 的「已知债务」第 11 条](../CHANGELOG.md)。

### 1.2 Windows

1. 双击 `Tauron Minimal App_0.1.0_x64-setup.exe`。
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

启动后你会看到一个主窗，里面有四条可点的演示链路。**这四条就是验收清单**——
它们各自对应框架里一条真实链路，点一遍就知道装对了没有：

| 点它 | 应该看到 | 背后走的链路 |
|---|---|---|
| **格式化（沙箱插件）** | 文本被规整（多余空白被压缩） | 宿主 → iframe 插件握手（token 经 URL hash 注入）→ postMessage 调用 → 返回 |
| **最小化窗口** | 窗口真的最小化 | 前端 → `host_window_minimize` → Tauri `window.minimize()` |
| **读剪贴板** | 弹出剪贴板当前内容 | 前端 → `host_clipboard_read`（进程内真实读取） |
| **检查更新** | 返回一个**带 `simulated: true` 标记**的响应 | 前端 → `host_market_check` |

> **最后一条是刻意设计的诚实边界**：更新检查目前返回的是**模拟响应**，
> 字段里带 `simulated: true`。这不是 bug，也不是「已实现」——本项目刻意区分
> 「真实实现」与「接口占位」，不伪造成功。看到 `simulated: true` 说明行为正确。

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

### 3.1 ⚠️ 尚未发布到 registry

**20 个 npm 包全部是 `private: true`，15 个 crate 的内部互引用是 path 依赖**
（cargo 打包要求 path 依赖同时给出 `version`）。所以：

- ❌ `pnpm add @tauron/core` —— 装不到
- ❌ `cargo add tauron-host` —— 装不到
- ✅ 用 `workspace:*`（同一个 workspace 内）或 `file:` / `path`（跨仓库）

阻塞项逐条登记在 [CHANGELOG.md 的「已知债务」](../CHANGELOG.md)。

### 3.2 最小接入

```bash
# 前端：同一 pnpm workspace 内
pnpm add @tauron/types@workspace:* @tauron/core@workspace:*
# 跨仓库则用 file: 指向本仓库的 packages/<包名>
```

```toml
# src-tauri/Cargo.toml
[dependencies]
tauron-shell = { path = "../tauron/crates/tauron-shell", features = ["tauri"] }
```

完整的 Rust 侧命令注册与前端运行时初始化代码见
[README 的「快速开始」](../README.md#快速开始第三方集成)。

### 3.3 三档装配——先决定你要哪一档

tauron 的装配是**分档**的，别一上来就全接：

| 档位 | 拿到什么 | 命令面 | 适用 |
|---|---|---|---|
| **只取底座** | 窗口/剪贴板/对话框/事件/设置/恢复/i18n/通知/品牌 | 38 条 | 不跑插件系统的普通客户端 |
| **底座 + 插件运行时** | 上面 + 注册表/生命周期/流式调用 | 59 条 | 要装插件 |
| **完整客户端** | 上面 + 设置中心/白标/主题/UI 组件 | 59 条 + UI 层 | 交付完整产品 |

逐档的依赖清单、装配代码与注意事项见
[渐进接入指南](./integration/incremental-adoption.md)——**这是集成方的第一入口**。

---

## 4. 已知限制（诚实清单）

装之前先知道这些，能省掉大量「是不是坏了」的排查：

| 限制 | 说明 |
|---|---|
| **安装包未签名 / 未公证** | Windows 会弹 SmartScreen，macOS 会被 Gatekeeper 拦。**不是**安装包坏了 |
| **Windows 只有 NSIS，没有 MSI** | 见 §1.1 与 CHANGELOG 债务 #11 |
| **未发布到 npm / crates.io** | 只能 path / workspace 接入，见 §3.1 |
| **示例应用是示例** | 界面极简，只有四条演示链路，不是产品形态的客户端 |
| **更新检查是模拟的** | 返回带 `simulated: true` 的响应，不真连更新服务器 |
| **图标是示例图标** | 由 `app-icon.svg` 生成的几何标记，非正式品牌资产 |
| **Linux / macOS 未在真机安装验证** | CI 能出包，但「装完能不能用」只在 Windows 上实测过 |

---

## 5. 常见问题

**Q：Releases 页面什么都没有。**
产物先落成**草稿**，需要维护者点发布；或者当前还没有推过 `v*` tag。
想自己出包看 §2。

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
| 架构与关键设计决策 | [架构概览](./architecture/overview.md) |
| `host_*` 线格式协议 | [应用层线格式协议](./architecture/app-layer-wire.md) |
| 版本变更与已知债务 | [CHANGELOG](../CHANGELOG.md) |
