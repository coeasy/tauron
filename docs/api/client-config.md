# 客户端配置接口（`ClientConfig`）

> 第三方集成侧的**唯一配置入口**。本文写的是当前真实行为，包括哪些键今天不生效——
> 一份不说明落点的配置文档，等于让集成方以为自己配置成功了。

## 1. 它是什么

`ClientConfig`（Rust：`crates/tauron-host/src/config.rs`；TS 镜像：
`packages/tauron-host/src/client-config.ts`）把客户端侧的全部配置聚合成一个结构：
注册表过滤与容量、日志级别、数据目录、以及若干运行时开关。

- Rust 侧 `#[serde(deny_unknown_fields)]`：**一个拼错的键会让整份文件加载失败**，
  不是"忽略那一项"。
- 所有字段都是 `Option`：缺省由 `Default` 提供，空配置 = 全量加载 + 默认限制。

## 2. 怎么加载

| 途径 | 说明 |
| --- | --- |
| `ClientConfig::from_file(path)` | 参考宿主 `examples/minimal-app` 的 `load_adapter_config()` 在环境变量 `TAURON_CLIENT_CONFIG=/path/to/client-config.json` 非空时调用它 |
| `ClientConfig::from_json(&str)` | 内联配置；同样先 `validate()` |
| `ClientConfig::to_json()` | 生成模板（`tauron-app client config` 走的是 TS 侧等价生成器） |

加载失败时参考宿主**不静默降级**：它在 stderr 打出失败原因，然后回落到
`AdapterConfig::default()`。也就是说"配置没生效"在宿主日志里看得见。

前端目前没有运行时读取路径：`client-config.json` 由 `tauron-app client config` /
`tauron-app init` 写出，供**宿主进程**消费。`@tauron/host` 侧的
`validateClientConfig` / `fullLoadConfig` / `minimalConfig` / `templateConfig` 是
同构校验与模板生成器，不自动读盘。

## 3. 键表与**落点**

「有落点」= 有生产代码真的读它并据此改变行为。权威表是
`tauron_host::config::CLIENT_CONFIG_LANDING`，TS 侧镜像是
`CLIENT_CONFIG_LANDING`（`@tauron/host`），CLI 侧清单是
`CLIENT_CONFIG_KEYS` / `CLIENT_CONFIG_UNWIRED_KEYS`（`@tauron/app-cli`）。
三份清单的逐键一致性由 `packages/tauron-host/src/gates.test.ts` 机器校验。

| 键 | 类型 | 缺省 | 落点 |
| --- | --- | --- | --- |
| `registry` | object | `default_config()` | ✅ `AdapterConfig::registry`（含 `plugin_filter`、容量上限、TTL） |
| `log_level` | `"error"\|"warn"\|"info"\|"debug"\|"trace"` | `"info"` | ✅ 参考宿主写入 `RUST_LOG`（未显式设置时） |
| `data_dir` | string | 平台默认 | ✅ `AdapterConfig::recovery_data_dir`（恢复标记与设置文档的落盘目录） |
| `env_overrides` | map<string,string> | 空 | ✅ `AdapterConfig::plugin_env_overrides` → `tauron-proc::SpawnConfig::env`，每次 `host_runtime_spawn` 注入 sidecar 进程 |
| `auto_update` | bool | `true` | ⚠️ **无落点**：更新开关在前端 `AutoUpdateClient` 的 `autoDownload`（+ 必填的 `currentVersion`），没有从本键派生。**注意 `endpoints`/`pubkey` 不是开关**（轮 29 定口径）：更新端点的权威来源是宿主装配注入的 `EndpointClient`，前端那两个字段今天不生效 |
| `update_check_interval_secs` | u64（必须 > 0） | `86400` | ⚠️ **无落点**：同上，`AutoUpdateConfig::checkIntervalSecs` 不从本键派生 |
| `crash_report_enabled` | bool | `false` | ⚠️ **无落点**：仓库里只有崩溃**窗口计数**（`tauron-proc::CrashTracker`），没有报告导出器 |
| `brand_id` | string | 内置品牌 | ⚠️ **无落点**：运行时品牌是 `TAURON_BRAND_CONFIG[_JSON]` 提供的**单个** `BrandConfig`，不存在"按 id 选品牌"的目录 |
| `plugin_paths` | string[] | 仅内置 | ⚠️ **无落点**：没有"扫描目录注册插件"的生产入口，安装侧只有单个 `plugin_install_dir` 根 |
| `performance_monitoring` | bool | `false` | ⚠️ **无落点**：`EventBus` 的 `BusStats`/`QueueStats` 没有命令读取，也没有 CPU/内存采样器 |

⚠️ 的键**照旧被解析、照旧被 `validate()` 校验**，但不驱动任何行为。装配层不会为它们
造布尔位——那会把"没实现"重新包装成"已生效"。

## 4. 不生效的键必须说出来

- Rust：`ClientConfig::unwired_fields()` 返回**本份配置实际写了**的无落点键
  （键名 + 原因）。参考宿主在启动横幅逐个 `eprintln!`：
  `[tauron] 配置项 \`brand_id\` 当前不生效（无宿主落点）：…`
  （`tauron-app new` 生成的宿主模板同源。）
- TS：`clientConfigUnwiredKeys(config)` / `clientConfigUnwiredSummary(config)`。
- CLI：`tauron-app client config` 写文件后逐项 warn；
  `tauron-app doctor` 对已存在的 `client-config.json`：
  未知键 → **fail**（宿主会拒绝整份文件），值非法 → **fail**，
  有键无落点 → **warn**，全部有落点 → **pass**。

只报"用户写了的"键：没写过的键谈不上被忽略。

## 5. 校验规则

`validate()`（Rust）与 `validateClientConfig()`（TS，CLI 侧是
`validateClientConfigFile`）同语义：

- `log_level` 必须取五个合法值之一；
- `update_check_interval_secs` 必须 > 0；
- `registry.max_plugins` / `max_active_identities` / `max_pending_calls` /
  `pending_ttl_secs` 必须 > 0（Rust 侧是无符号类型，负数在反序列化就失败；
  TS 侧数值是 double，因此 `<= 0` 一律拒绝，两侧对齐）；
- `plugin_filter.allow` 与 `deny` 不得同时含同一个插件 id。

错误码：解析/校验失败统一为 `E_INVALID_MANIFEST`（不可重试）。

## 6. 最小示例

```json
{
  "log_level": "info",
  "data_dir": "~/.config/acme/desktop",
  "env_overrides": { "ACME_PROFILE": "prod" },
  "registry": {
    "max_plugins": 6,
    "plugin_filter": { "allow": ["com.acme.formatter"], "types": ["js"] }
  }
}
```

这份配置每个键都有落点，`tauron-app doctor` 应报 **pass**。加上
`"auto_update": false` 之后 doctor 会 warn——它现在确实不改变任何行为。

## 7. 相关文档

- 命令面与宿主装配：[命令面全量参考](./command-surface.md)、
  [插件开发指南](./plugin-development-guide.md)
- 增量接入（配置在既有项目里怎么落地）：
  [incremental-adoption](../integration/incremental-adoption.md)
