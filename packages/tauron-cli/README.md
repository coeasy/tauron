# @tauron/cli

CLI 工具 —— `create`、`plugin new|dev|test|pack|sign|publish`、`doctor`。

## 安装

> ⚠️ **尚未发布到 npm**（本包当前 `private: true`），`npm install -g @tauron/cli`
> 装不到任何东西。在仓库内直接跑 bin：

```bash
node packages/tauron-cli/bin/tauron.js --help
```

下文用 `tauron` 代指 `node packages/tauron-cli/bin/tauron.js`（在仓库根执行）。

## 使用

```bash
tauron --version
tauron doctor                        # 环境诊断
tauron create my-app                 # 真的落盘（目录已存在时需 --force）
tauron plugin new com.example.plugin --type js
tauron plugin dev
```

## 各子命令的真实边界

本仓库的规矩是「没接线就写没接线」，下面按真实状态列，不按设计意图列。

| 命令 | 状态 |
|---|---|
| `doctor` | ✅ 真实探测 Node / npm / pnpm / cargo / rustc / Tauri CLI |
| `create` | ✅ 真的落盘；目录已存在时默认拒绝覆盖，`--force` 才覆盖，`--dry-run` 只报告 |
| `plugin new` | ✅ 真的落盘；`--type js\|process\|wasm` 两种写法都支持，非法值如实失败 |
| `plugin sign` | ✅ 算**真实** SHA-256 内容摘要并写出 `.sig`（**不是**非对称签名；要 Ed25519 用 `tauron-app plugin sign`） |
| `plugin dev` / `test` / `pack` / `publish` | ⚠️ **未实现**——如实返回 `success: false`：不谎报已启动 dev server、不把「发现的测试文件数」当通过数、不声称产出归档、不编造商城地址 |
