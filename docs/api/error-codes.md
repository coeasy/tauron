# 宿主错误码参考（自动生成，请勿手改）

> **这个文件是生成物**：`node scripts/generate-error-codes-doc.mjs` 从
> `contracts/error/error-codes.json` + `crates/tauron-host/src/error.rs` 的 `pub enum ErrorCode` 单向生成，
> CI 用 `pnpm gates:check` 里的 `--check` 复算。它把三件事变成可执行断言，而不是某人
> 记得去补的表：
>
> 1. **文档码集合 = Rust 枚举码集合**（跨源单一真源，枚举才是权威）；
> 2. **每条码都有非空 `summary` 与 `recovery`**——空占位即红，杜绝「用空字段冒充已文档化」；
> 3. **`retryClass` 落在合法词表**（never / manual / auto-idempotent / after-reconnect）。
>
> `retryClass` 与 `code` 是**协议面**，由 wire-gate 三方对读（Rust `retry_class()` /
> `@tauron/host` 词表 / 本 registry）；`summary`/`recovery` 是给人看的**路标**，
> 不参与协议判定。自动重试永远不 implied：`retryable` 仅对 `auto-idempotent` 为真，
> 而当前集合里没有任何码走这一档（见下）。

## 计数口径

共 **24** 条错误码；按 `retryClass` 分布：never **20**、manual **3**、after-reconnect **1**。

| 错误码 | retryClass | 含义（一句话） | 调用方恢复动作（下一步该做什么） |
|---|---|---|---|
| `E_HOST_PANIC` | `never` | handler panic，经 catch_unwind 归一化（ADR-04）。 | 不自动重试：panic 可能已产生部分副作用、回滚无证据。记录现场并上报，由人工判定是否重放该操作。 |
| `E_UNKNOWN_PLUGIN` | `never` | 未知 plugin_id。 | 先注册/安装该插件（host_plugin_register 或安装链路），或核对 plugin_id 拼写；不要对同一未知 id 重试。 |
| `E_AUTH_DENIED` | `never` | 授权档位不满足，或身份被伪造（§2.1 self 档忽略入参 pluginId）。 | 调用方身份/档位不足，重试不能解决：改用有权限的主体（主窗 / 特权面）发起，或申请相应授权。 |
| `E_INVALID_MANIFEST` | `never` | manifest 不合法：未知字段、非法 id、权限表外字符串、缺 framework range。 | 修正 manifest 本身（加载期硬校验失败）；重试无意义，先让清单合法再重载。 |
| `E_STATE_INVALID_TRANSITION` | `never` | 状态机无匹配规则（非法迁移）。 | 调用序列错了：按生命周期正确顺序驱动（先 prepare 再 start、先 start 再 invoke），不要对终态重复驱动。 |
| `E_CALL_NOT_FOUND` | `never` | pending call 不存在或已结束。 | 该调用已不可操作：若需结果去 host_call_take 取已结算项，否则放弃这个 call_id，勿重复 cancel / 回填。 |
| `E_CALL_TIMEOUT` | `manual` | pending call 超时。 | 人工决定：超时不等于未发生，先查副作用是否已落地，再决定是否重发；幂等性未证明前不自动重试。 |
| `E_FORBIDDEN_PERMISSION` | `never` | 申请了「禁止授予清单」内的权限（§2.1）。 | 该权限无论如何都批不下来：从 manifest 申请集里移除它，而不是重试或换主体申请。 |
| `E_ABI_MISMATCH` | `never` | ABI 指纹不匹配：spawn 前比对宿主契约与调用方声明的 profile.abi，两份都合法但彼此不符。 | 版本兼容问题：升级插件或宿主到 ABI 相符的一侧；走重装（E_INSTALL_FAILED 的路径）解决不了。 |
| `E_PLUGIN_DISABLED` | `never` | 插件已禁用——enabled 标志做即时拒绝（ADR-05）。 | 需用户/管理员在设置里启用该插件后重试；运行期自动重试无效。 |
| `E_REGISTRY_FULL` | `never` | 注册表达到活跃身份上限（§4.6：缺省 8）。 | 容量闸：先卸载或停用不再需要的插件释放槽位，再注册新插件；不要盲目重试。 |
| `E_CALL_PENDING_FULL` | `never` | pending call 表达到上限。 | 并发在途调用过多：等待已有调用结算（host_call_take）后再发起新调用。 |
| `E_SUBSCRIPTION_FULL` | `never` | 订阅表达到上限（eventbus::MAX_SUBSCRIPTIONS）。 | 先退订不再需要的事件，腾出订阅额度后再 subscribe。 |
| `E_PLUGIN_EXISTS` | `never` | 插件已存在（重复注册）。 | 同一 plugin_id 只注册一次：复用已注册实例，或先卸载再重装；重复注册是调用方逻辑错误。 |
| `E_INSTALL_FAILED` | `never` | 安装期失败：验签 / hash / 解包 / range 任一环节（D3：落 INSTALL_FAILED）。 | 安装前置不满足（来源包本身有问题），重试同一包无效：更换可信来源包或修复包内容。 |
| `E_PLUGIN_FILTERED` | `manual` | 插件被配置过滤器排除（配置化选择加载）。 | 这是配置意图而非故障：如确需加载，修改配置把它纳入过滤范围后由人工重试。 |
| `E_PLUGIN_TYPE_NO_RUNTIME` | `never` | 该插件类型没有运行期执行器（诚实失败码：不静默成功、也不伪造 pid）。 | 形态没有可用执行器：换用受支持的形态，或等待对应 RuntimeDriver（见 runtime_driver.rs 契约）接入；绝不能当成功处理。 |
| `E_LEASE_EXPIRED` | `after-reconnect` | 运行时租约不存在或已失效（进程已回收 / 宿主已重启）。 | 租约语义（区别于 E_CALL_NOT_FOUND）：重新 host_runtime_spawn 建立新租约后再调用。 |
| `E_STREAM_FULL` | `never` | 流句柄数已达上限（同一进程内并发打开的流太多）。 | 先 host_stream_close 关闭不用的流再开新流；这不是插件数上限（那是 E_REGISTRY_FULL）。 |
| `E_CALL_ALREADY_SETTLED` | `never` | 一次跨主体调用已被结算，重复回填被拒（0.4-A1）。 | 结果已定、不允许被第二次回填悄悄改写：去 host_call_take 取已结算结果；重复 host_call_result 说明执行方行为异常。 |
| `E_CONTRIBUTES_DRIFT` | `never` | manifest 声明的贡献与 activate 期实际注册的贡献不一致（0.4-W3）。 | 宣称与事实的落差：让插件补齐实际注册的贡献，或修正 contributes 声明使两者一致；确定性故障，重试不会凭空出现缺失注册。 |
| `E_STREAM_BACKPRESSURE` | `manual` | 流生产者耗尽接收方授予的字节窗口；被拒帧不投递、不占序号。 | 等接收方补给（host_stream_grant）后由人工重发被拒帧；被拒帧未消费序号，勿假定已送达。 |
| `E_CALL_CYCLE` | `never` | 同步调用图会成环、重入一个主体，或超出有界跳数预算。 | 调用拓扑错误：打断环 / 去掉重入 / 拆分调用链使图有界无环；重试同一拓扑必再失败。 |
| `E_EVENT_CAUSATION_LIMIT` | `never` | 事件因果链超出有界深度预算。 | 事件风暴 / 递归触发：降低因果链深度（避免事件处理器再发同类事件），非重试可解。 |

## 读表须知

1. **`never` 不是「别管它」**：多数码是确定性故障（清单写错、序列错了、容量闸、宣称与事实
   落差），自动重试只会再失败一次——恢复动作那一列就是给调用方分流用的路标。
2. **`manual` / `after-reconnect` 要人推一把**：超时（`E_CALL_TIMEOUT`）与反压
   （`E_STREAM_BACKPRESSURE`）都可能已经产生副作用，重发前必须自己判定幂等性；
   租约失效（`E_LEASE_EXPIRED`）要先 `host_runtime_spawn` 建新租约。
3. **`E_PLUGIN_TYPE_NO_RUNTIME` 是诚实失败码**，不是桩成功：本仓的
   `RuntimeDriver` 契约（`crates/tauron-host/src/runtime_driver.rs`）正是要把这条码逐步
   变成「对应有执行器」的真运行路径（见统一开发计划 T-6/T-10/T-11/T-12）。
4. 语义边界仍以代码为准：错误模型的产生点写在 `crates/tauron-host/src/error.rs` 的文档注释里，
   命令的档位与判定见 `docs/api/command-surface.md`。
