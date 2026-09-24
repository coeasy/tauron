# tauron 架构文档

本目录包含 tauron 的架构设计、协议规范与优化方案。

## 文档地图

| 文档 | 内容 | 性质 |
|---|---|---|
| [overview.md](./overview.md) | 整体架构图、两层架构、包命名体系、模块依赖、关键设计决策 | **权威说明**——改架构先改这里 |
| [app-layer-wire.md](./app-layer-wire.md) | 应用层 `host_*` 命令族的线格式规范（参数形状 / 返回值 / 生命周期事件 / 能力档位）与限制登记 | 协议契约，由 wire-gate 门禁锁定 |
| [canonical-owners.md](./canonical-owners.md) | canonical 归属表：哪些实现是唯一事实源、哪些是已冻结的 legacy 门面 | 架构决策——改这张表等于改架构 |
| [multi-plugin-substrate-roadmap.md](./multi-plugin-substrate-roadmap.md) | 0.3 优化改进方案：成熟度记分卡、残差清单、S/M/X 改进项、轮次编排 | **唯一的前瞻计划** |
| [../integration/incremental-adoption.md](../integration/incremental-adoption.md) | 三档装配指南：只取底座 / 底座 + 插件运行时 / 完整客户端 | 集成方入口 |
| [../api/plugin-development-guide.md](../api/plugin-development-guide.md) | 插件开发指南：类型、清单、权限、CLI 的真实边界 | 插件作者入口 |
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
3. **双世界隔离** — 双世界隔离架构（QuickJS-WASM 引擎接入为路线图项）
4. **双层 ACL** — 外层 Tauri 静态 + 内层框架动态
5. **4+1 插件形态** — Rust / JS（真实实现）/ WASM / Process（配置 + 模拟原型）+ B+ 混合模式
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
| Lint | ESLint（硬门禁）、rustfmt / clippy / cargo-deny（advisory） |
