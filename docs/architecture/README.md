# tauron 架构文档

本目录包含 tauron 的架构设计、协议规范与优化方案。

## 文档地图

| 文档 | 内容 | 性质 |
|---|---|---|
| [../installation.md](../installation.md) | 安装与使用：三种「安装」辨析、各平台安装示例应用、从源码构建、接入自己项目、装完自检、已知限制与 FAQ | 落地入口——**第一次接触先读这份** |
| [overview.md](./overview.md) | 整体架构图、两层架构、包命名体系、模块依赖、关键设计决策 | **权威说明**——改架构先改这里 |
| [app-layer-wire.md](./app-layer-wire.md) | 应用层 `host_*` 命令族的线格式规范（**关键命令**的参数形状 / 返回值 / 生命周期事件 / 能力档位）与限制登记；全量清单见下方生成物 | 协议契约，由 wire-gate 门禁锁定 |
| [canonical-owners.md](./canonical-owners.md) | canonical 归属表：哪些实现是唯一事实源、哪些是已冻结的 legacy 门面 | 架构决策——改这张表等于改架构 |
| [multi-plugin-substrate-roadmap.md](./multi-plugin-substrate-roadmap.md) | 0.3 优化改进方案：成熟度记分卡、残差清单、S/M/X 改进项、轮次编排 | 历史方案（§2 残差表已过期，见其顶部复核横幅） |
| [capability-closure-plan.md](./capability-closure-plan.md) | 0.4 能力收口方案：A1–A8 + 轮 19–29；**仅 A1–A3 已落地** | 历史方案（A4–A8 由下方 1.0 方案接管） |
| [full-architecture-refactor-plan.md](./full-architecture-refactor-plan.md) | 1.0 全量架构重评估与重构方案：实测缺陷清单、W1–W10 工作流、轮 30–41 编排 | 前瞻计划（V4 之前的主线） |
| [v4-industrial-gap-closure-plan.md](./v4-industrial-gap-closure-plan.md) | V4 工业级缺口收口方案：A01–A110 与 §134/§136/§141 逐项「已落 / 部分 / 未落」台账、F1–F5 危险缺口、Batch 0–6' 编排与推迟清单 | **当前执行台账**（对外措辞与发布判据以这份为准） |
| [../Tauron-Architecture-Competitive-Analysis-Optimization-Plan-V5.md](../Tauron-Architecture-Competitive-Analysis-Optimization-Plan-V5.md) | V5 竞分析与优化方案：七 Plane 目标架构、RuntimeDriver / HostTransport / Manifest V3 契约、Phase A–G 优先级与退出门槛、能力诚实分级 | **对照基线**（轮 13 起的新增工作对照此方案；前瞻方案，不描述现状） |
| [../api/command-surface.md](../api/command-surface.md) | 命令面**全量**参考：85 条 `host_*` 的业务形参 / 返回 / 档位与判定 / feature 门 / 前端落点 | **生成物**（`pnpm command-surface:gen`，CI `command-surface:check` 复算，勿手改） |
| [../integration/incremental-adoption.md](../integration/incremental-adoption.md) | 三档装配指南：只取底座 / 底座 + 插件运行时 / 完整客户端 | 集成方入口 |
| [../api/plugin-development-guide.md](../api/plugin-development-guide.md) | 插件开发指南：类型、清单、权限词表、宿主命令面、错误码、生命周期状态机、sidecar ABI、CLI 的真实边界 | 插件作者入口 |
| [../competitive-analysis/competitive-analysis.md](../competitive-analysis/competitive-analysis.md) | 竞品全景图、功能对比矩阵、头条特性兑现度标记 | 定位参考（带快照日期） |

## 关于已清理的历史文档

本目录此前还放着两份计划文档，已在 0.3 轮清理：

| 已删除 | 删除原因 |
|---|---|
| `client-foundation-upgrade-plan.md` | 2026-01 的审计基线（TS 1122 / Rust 984），数字比现状落后两代；结论已被后续两轮重构取代 |
| `tauron-substrate-refactor-plan.md` | 底座重构 R1–R8 **已全部完成**，但文档头仍标「设计稿（待执行）」——状态是假的，会误导读者 |

**不需要读原文**：这两份文档的耐久产出都已沉淀到别处。

- R1–R8 的架构决策 → [overview.md](./overview.md) 的「关键设计决策」+「架构演进」
- canonical 归属结论 → [canonical-owners.md](./canonical-owners.md)
- 已达成、不得回退的硬不变量 → [multi-plugin-substrate-roadmap.md](./multi-plugin-substrate-roadmap.md) §0

## 核心架构原则

1. **平台无关核心** — Rust crate 的默认构建不依赖 `tauri`，核心逻辑可离线测试
2. **信封协议** — 单一 `plugin_invoke` 命令，统一调用 / 取消 / 进度
3. **Js 型隔离来自 webview 边界 + 逐命令身份判定**（**不是**进程内沙箱）。
   `@tauron/dual-world` 的进程内 JS 沙箱是 fail-closed 模拟（`SANDBOX_UNAVAILABLE`），
   不伪报执行成功；进程内 WASM 运行时为路线图项。
4. **双层 ACL** — 外层 Tauri 静态 + 内层框架动态
5. **插件形态以 `PluginType` 枚举为准**（`crates/tauron-host/src/manifest.rs`）：
   Js / Process 有生产执行器，Rust / Wasm 目前返回 `E_PLUGIN_TYPE_NO_RUNTIME`（诚实失败）。
   **代码里不存在「B+ 混合模式」**——该宣称已于 1.0 重评估中删除。
6. **跨语言契约测试** — TS↔Rust 协议验证（wire-gate，直接扫 Rust 源码文本）
7. **诚实边界** — 未接线的能力显式标注，用 `simulated` 字段与诚实失败码表达，不伪造成功

## 技术栈

| 层 | 技术 |
|---|---|
| Rust 壳层 | Rust 1.98、serde、parking_lot、uuid |
| TypeScript 核心 | TypeScript 5.8、Node 22 |
| 包管理 | pnpm workspace、Cargo workspace |
| 测试 | vitest（TS）、cargo test（Rust） |
| 构建 | tsc、`pnpm -r build`、Tauri v2 |
| CI / 发布 | GitHub Actions（`ci.yml` 门禁 / `release.yml` 矩阵构建） |
| Lint | ESLint / rustfmt / clippy（硬门禁）、cargo-deny（硬门禁：`ci.yml` 的 `deny` 作业 `command: check`，无 `continue-on-error`） |
