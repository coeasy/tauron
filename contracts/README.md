# `contracts/` —— 机器复核的公共账本（single source of truth）

本目录是「**对外承诺的事实**」的账本层：凡是被 README / 文档 / 示例 / CI 当作"可用能力"或
"某个数字"对外宣称的东西，都在这里留一份**可机器对账**的记录，并有一个门禁负责把它和真实
代码/文件钉在一起。核心纪律只有一条——

> **手写数字与手写"已接线"即 bug。** 每个账本要么由**生产者脚本从代码复算**，要么即使手写，
> 也必须有一个**能失败**的复核门（ratchet / 等值读数钉 / 三方对读），否则它就是下一个会腐烂的谎。

改任何账本前，先找到「复核门禁」列里那个脚本，把它跑绿；账本与代码不一致时，**改代码或改账本，
但不许关掉门禁**。

## 台账索引

| 账本 | 内容 | 生产者（谁写） | 复核门禁（谁判红） | 运行位置 |
|---|---|---|---|---|
| `facts.json` | 对外数字单一真源：Rust `#[test]` 声明数、TS 测试文件数、crate/package 数、命令面、版本 | `scripts/generate-facts.mjs`（从代码复算，同时反向重写 README 徽章与事实表） | `generate-facts.mjs --check` + `--self-test`（注入漂移须被抓） | `pnpm gates:check` |
| `error/error-codes.json` | 宿主错误码公共账本：`code`/`retryClass` 为协议面，`summary`/`recovery` 为人读路标 | 手写协议面（权威在 Rust `ErrorCode` 枚举与 `retry_class()`）；文档由 `scripts/generate-error-codes-doc.mjs` 单向生成 `docs/api/error-codes.md` | `generate-error-codes-doc.mjs --check` + `--self-test`（码集合=枚举全等 / 路标非空 / retryClass 合法）；`wire-gate` 三方对读 code/retryClass | `pnpm gates:check` + `pnpm -F @tauron/contract-tests test` |
| `orphan-public-api.json` | 孤儿公共 API 台账：宣称可用却在接线面零消费者的公开符号，逐条写 `disposition` | 人工登记（配 ratchet 基线 + 等值读数钉 `discoveryObserved`） | `scripts/check-orphan-public-api.mjs`（新增孤儿即红；已接线条目须零命中 / wiredWitness 须有命中） | `pnpm gates:check` |
| `adapter-domain-ownership.json` | 适配层域归属基线：lib.rs 顶层条目、各域 impl/fn 数量封顶 | 人工设基线 | `scripts/check-adapter-domain-ownership.mjs`（数量只降不升）+ `--self-test` | `pnpm gates:check` |
| `tier-bundles.json` | 命令档位与 bundle 划分（S⊂E⊂P 单调、85 条命令单属一 bundle） | 人工定义 | `scripts/check-tier-bundle.mjs`；`wire-gate` 对读 | `pnpm gates:check` + `contract-tests` |
| `shelf-advertising.json` | 货架规则观察产出：README `pnpm add` 广告位、`create-tauron-app#dependencies`、`private:true` 包集合、`reference-only` 包集合、已知包集合（R-5；刻意不含手写成熟度分级——遵循 `module-maturity.json` 的"手写等级=复制谎报"纪律） | `scripts/check-shelf-rules.mjs --write`（从 README/packages/*/package.json/create-tauron-app/package.json/module-maturity 复算） | `scripts/check-shelf-rules.mjs`（Inv-Sync 复算等值 / Inv-B private 不上广告 / Inv-C reference-only 不进脚手架 / Inv-Known 广告名必收录）+ `--self-test`（7 条变异夹具）；`wire-gate` 对读四段观察与实存文件 | `pnpm gates:check` + `contract-tests` |
| `placeholder-ledger.json` | 全仓 `simulated: true` 诚实占位的逐文件计数 + 归宿（`disposition ∈ {wire,downgrade-private,delete,defer}` + `batch` 批次归属）——独立审计根因 1「占位无转正约束」与统一计划纪律 2「占位必须有归宿」的机器化；补齐 `check-simulated-never-commits` / `check-success-requires-effect` 只钉 6 具名文件形状、抓不到别处新长桩的作用域缺口（IND-3.3/3.4） | `scripts/check-placeholder-ledger.mjs --write`（扫 crates/ + packages/ 全部 .rs/.ts 非测试文件，stripped 投影后按文件计 `simulated: true` 出现数；保留已有归宿、新桩只留空 `""` 逼维护者亲自填归宿——`--write` 从不自动决定归宿） | `scripts/check-placeholder-ledger.mjs`（Inv-Scan 双向计数：`unregistered` 未登记新桩 / `stale` 转正或删除后没销账 / `count` 数量漂移；Inv-Disposition 归宿 ∈ 枚举；Inv-Batch 批次非空）+ `--self-test`（5 条变异夹具，五判据各按预期把对应规则打红）；`wire-gate` 松核对 file 存在 + 含 simulated 形状 + pnpm 入口 + gates:check 链 + CI 调用 | `pnpm gates:check` + `contract-tests` |
| `module-maturity.json` | 模块收录 / adapter 依赖形态 / 文档接线判定的**机械可反推**事实（刻意不含成熟度分级——那需 conformance 证据背书，手写等级会复制谎报模式） | 人工登记机械事实 | `wire-gate`「module-maturity 台账」逐条与真实 `Cargo.toml`/`overview.md`/`canonical-owners.md`/`package.json` 对账 | `pnpm -F @tauron/contract-tests test` |
| `performance-budgets.json` | 性能/体积预算：wire 往返、安装流、二进制、发布产物大小上限 | 人工设预算 | `scripts/check-performance-size.mjs`、`scripts/check-release-artifact-sizes.mjs`；`wire-gate` | 专项 CI job |
| `public-surface-ledger.json` | 默认构建实际对外可见的命令/符号面（与全量命令参考区分） | `scripts/generate-public-surface-ledger.mjs` | `generate-public-surface-ledger.mjs --check`（漂移即红） | 专项 CI job |
| `target-matrix.json` | OS×arch×profile×runtime 的发布与 conformance 目标矩阵（V4 A99） | 人工定义目标；`scripts/generate-release-evidence.mjs` 产出证据 | `scripts/check-target-matrix.mjs`、`scripts/check-release-artifact-sizes.mjs` | 专项 CI job |

## 读约定

- **生产者 vs 复核者**：标注"生产者脚本"的账本，其内容不应手改——改代码后重跑生产者，`--check`
  会证明文档/台账是否跟着更新。标注"人工"的账本（多为基线/预算/处置），改它必须**同时**满足
  复核门的单向棘轮或等值读数钉，不能只改数不接线。
- **哪些进 `gates:check`**：`facts`、`error-codes`、`orphan-public-api`、`adapter-domain-ownership`、
  `tier-bundle` 以及一批 `check-*` 结构门在 `pnpm gates:check` 里串行执行；`performance-budgets`、
  `public-surface-ledger`、`target-matrix` 属专项 CI job（离线/联网/发布产物相关，不在本地串门里）。
- **占位即真相**：诚实失败态（`simulated`、`delivered:false`、`E_PLUGIN_TYPE_NO_RUNTIME`）是**类型化的
  如实状态**，不是待抹平的缺陷。账本记录它们，是为了让"哪条链还没通"可被复算，而不是被遗忘。
