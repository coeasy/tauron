#!/usr/bin/env node
// 命令面全量接口参考的**生产者**：从代码单向生成 docs/api/command-surface.md。
//
// 为什么是生成而不是手写：85 条命令的形参/返回/档位散在五处事实里（tauri.rs 的定义、
// lib.rs 的三个编译期集合、host/adapter 的 authz 档位表、命令函数**委托**到的
// `cmd_*_as` 判定、packages 的调用点）。手写清单必然漂——轮 12 的实测就是漂的样子：
// `app-layer-wire.md` 只覆盖 49/85 条，而插件指南称它为「完整线格式」。
// 生成器把「文档覆盖 = 代码集合」变成可复算断言，`--check` 不一致即红。
//
// 用法：node scripts/generate-command-surface.mjs          # 写文件
//      node scripts/generate-command-surface.mjs --check   # 只判定（CI）

import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const OUT = 'docs/api/command-surface.md';
const TAURI = 'crates/tauron-adapter/src/tauri.rs';
const LIB = 'crates/tauron-adapter/src/lib.rs';
const AUTHZ = 'crates/tauron-host/src/authz.rs';
const read = (rel) => readFileSync(join(ROOT, rel), 'utf8');
const oneLine = (s) => s.replace(/\s+/g, ' ').trim();
const cell = (s) => s.replace(/\|/g, '\\|');

/** 从 `open`（`(` 的下标）起找到配对的 `)`，跳过 `//` 行注释与字符串字面量。 */
function matchParen(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    const ch = text[i];
    if (ch === '/' && text[i + 1] === '/') {
      const nl = text.indexOf('\n', i);
      i = nl < 0 ? text.length : nl - 1;
      continue;
    }
    if (ch === '"') {
      for (let k = i + 1; k < text.length; k++) {
        if (text[k] === '\\') k++;
        else if (text[k] === '"') {
          i = k;
          break;
        }
      }
      continue;
    }
    if (ch === '(') depth++;
    else if (ch === ')' && --depth === 0) return i;
  }
  throw new Error(`括号不配对：${text.slice(open, open + 80)}`);
}

/** 按顶层逗号切分（尖括号/括号内的逗号不算分隔），并剥掉 `#[attr]` 与行注释。 */
function splitTopLevel(text) {
  const parts = [];
  let nest = 0;
  let cur = '';
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (ch === '/' && text[i + 1] === '/') {
      const nl = text.indexOf('\n', i);
      i = nl < 0 ? text.length : nl - 1;
      cur += ' ';
      continue;
    }
    if (ch === '<' || ch === '(' || ch === '[') nest++;
    else if (ch === '>' || ch === ')' || ch === ']') nest--;
    else if (ch === ',' && nest === 0) {
      parts.push(cur);
      cur = '';
      continue;
    }
    cur += ch;
  }
  parts.push(cur);
  return parts.map((p) => oneLine(p.replace(/#\[[^\]]*\]/g, ' '))).filter(Boolean);
}

/** 按花括号配平索引一个源文件里的函数体（同名取第一个定义）。 */
function indexFunctions(rel) {
  const lines = read(rel).split(/\r?\n/);
  const out = new Map();
  for (let k = 0; k < lines.length; k++) {
    const m = lines[k].match(/^\s*(?:pub )?(?:async )?fn ([a-z_][a-z0-9_]*)\s*[(<]/);
    if (!m) continue;
    let depth = 0;
    let started = false;
    const body = [];
    for (let p = k; p < lines.length; p++) {
      body.push(lines[p]);
      for (const ch of lines[p]) {
        if (ch === '{') {
          depth++;
          started = true;
        } else if (ch === '}') depth--;
      }
      if (started && depth === 0) break;
    }
    if (!out.has(m[1])) out.set(m[1], body.join('\n'));
  }
  return out;
}

// 委托链遍历深度上限：命令函数（0）→ `cmd_*_as`（1）→ 私有 helper（2）→ 判定（3）。
// tauri.rs 的 85 个命令体里 `require_*` / `visible_notifications` / `scoped_within_roots`
// 的直接出现次数实测为 0，判定全在 lib.rs 委托层，所以深度是唯一的收集机制；
// 上限只影响「绕远路误收无关函数」的风险。可用 CS_MAX_DEPTH 复算对比。
const MAX_DEPTH = Number(process.env.CS_MAX_DEPTH || 3);

// `require_*` 是「拒绝」型判定；`visible_notifications` / `scoped_within_roots` 是另外两类
// 真判定（按身份过滤、限定根目录），只扫 `require_` 会把「有保护」写成「没有」。
const GUARD_RE = /\b(require_[a-z_]+|visible_notifications|scoped_within_roots)\s*\(([^;]*?)\)/g;
const GUARD_LABEL = {
  require_main_window: '仅主窗 `require_main_window`',
  require_self_plugin_scope: '绑定自身身份 `require_self_plugin_scope`',
  require_settings_key_scope: '绑定自身键空间 `require_settings_key_scope`',
  visible_notifications: '按身份过滤可见集合 `visible_notifications`',
  scoped_within_roots: '限定宿主允许根目录 `scoped_within_roots`',
};

// ── 1. Rust 命令定义：业务形参、返回、`///` 说明、feature 门 ─────────────────
const tauriLines = read(TAURI).split(/\r?\n/);
const defs = new Map();
for (let i = 0; i < tauriLines.length; i++) {
  if (!/#\[tauri::command\]/.test(tauriLines[i])) continue;

  // 往上收连续的 `///`（中间允许属性行与空行）。
  const doc = [];
  for (let k = i - 1; k >= 0; k--) {
    const l = tauriLines[k].trim();
    if (l.startsWith('///')) {
      doc.unshift(l.replace(/^\/\/\/?\s?/, ''));
      continue;
    }
    if (l.startsWith('#') || l === '') continue;
    break;
  }

  let j = i + 1;
  const attrs = [];
  while (j < tauriLines.length && /^\s*#/.test(tauriLines[j])) {
    attrs.push(tauriLines[j].trim());
    j++;
  }
  const name = (tauriLines[j] || '').match(/^\s*pub fn (host_[a-z0-9_]+)/)?.[1];
  if (!name) continue;

  // feature 门写在 `#[tauri::command]` **上方**（tauri.rs:1011/1031 的
  // `#[cfg(feature = "plugin-install")]`）。只从 `#[tauri::command]` 往下找属性会一条
  // 都找不到，于是把 opt-in 命令谎报成「无 feature 门」——85 行的 feature 列全空就是这个错。
  const aboveAttrs = [];
  for (let k = j - 1; k >= 0 && /^\s*#/.test(tauriLines[k]); k--) {
    aboveAttrs.unshift(tauriLines[k].trim());
  }
  const allAttrs = aboveAttrs.concat(attrs);

  let bodyEnd = tauriLines.length;
  for (let k = j + 1; k < tauriLines.length; k++) {
    if (/^\s*pub fn /.test(tauriLines[k]) || /#\[tauri::command\]/.test(tauriLines[k])) {
      bodyEnd = k;
      break;
    }
  }
  const head = tauriLines.slice(j, bodyEnd).join('\n');
  const open = head.indexOf('(');
  const close = matchParen(head, open);
  const afterParams = head.slice(close + 1);
  const brace = afterParams.indexOf('{');
  if (brace < 0) throw new Error(`${name}：找不到函数体起点`);
  const ret = oneLine(
    afterParams
      .slice(0, brace)
      .replace(/^\s*->\s*/, '')
      .replace(/!?\s*$/, ''),
  );
  const params = splitTopLevel(head.slice(open + 1, close)).filter(
    // 宿主注入的参数不是线形参：Tauri 从运行时取，前端不传。
    (p) =>
      !/^(state|app|window|webview)\s*:/.test(p) &&
      !/State<|AppHandle<|WebviewWindow<|TauriCallerSource/.test(p),
  );

  defs.set(name, {
    params,
    ret,
    summary: oneLine(doc.join(' ')),
    // `cfg(all(feature = "desktop", feature = "plugin-runtime"))` 里的每个 feature 都是
    // 注册前提，只取第一个会把「两个门」写成「一个门」。`cfg(test` 与 `not(feature` 不是。
    feature: (() => {
      const gates = allAttrs
        .filter(
          (a) => /^#\[cfg\(/.test(a) && !/^#\[cfg\(test/.test(a) && !a.includes('not(feature'),
        )
        .flatMap((a) => [...a.matchAll(/feature\s*=\s*"([^"]+)"/g)].map((x) => x[1]));
      return gates.length ? [...new Set(gates)].join(' + ') : null;
    })(),
    guards: [],
  });
}

// ── 2. 授权判定：必须**沿委托链**收集 ───────────────────────────────────────
// 命令函数几乎都只做「判定 + 转发」，真判定落在它委托的 `cmd_*_as` 里。只扫命令自己的
// 函数体就会把「有代码层判定」写成「没有」——轮 12 自查时真的踩过，方向与谎报能力同样糟。
const fns = new Map([...indexFunctions(LIB), ...indexFunctions(TAURI)]);
for (const [name, def] of defs) {
  const seen = new Set([name]);
  const queue = [[name, 0]];
  while (queue.length) {
    const [fnName, depth] = queue.shift();
    const body = fns.get(fnName) || '';
    for (const m of body.matchAll(GUARD_RE)) {
      const label = GUARD_LABEL[m[1]];
      if (!label) continue;
      const quoted = (m[2] || '').match(/"([a-z0-9_]+)"/);
      const entry = { guard: label, via: fnName, cmd: quoted ? quoted[1] : name };
      if (!def.guards.some((g) => g.guard === entry.guard && g.cmd === entry.cmd))
        def.guards.push(entry);
    }
    if (depth >= MAX_DEPTH) continue;
    for (const m of body.matchAll(/\b([a-z_][a-z0-9_]*)\s*\(/g)) {
      const callee = m[1];
      if (seen.has(callee) || !fns.has(callee)) continue;
      seen.add(callee);
      queue.push([callee, depth + 1]);
    }
  }
  def.guards.sort((a, b) => a.guard.localeCompare(b.guard) || a.cmd.localeCompare(b.cmd));
}

// ── 3. 命令集合（编译期可选集合 = 对外承诺面）──────────────────────────────
const adapterSrc = read(LIB);
const array = (name) => {
  const m = adapterSrc.match(new RegExp(`pub const ${name}:[^=]*=\\s*&?\\[([\\s\\S]*?)\\];`));
  if (!m) throw new Error(`找不到命令数组 ${name}`);
  return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
};
const sets = {
  substrate: array('SUBSTRATE_COMMANDS'),
  runtime: array('PLUGIN_RUNTIME_COMMANDS'),
  install: array('PLUGIN_INSTALL_COMMANDS'),
};
const inAnySet = new Set([...sets.substrate, ...sets.runtime, ...sets.install]);

// ── 4. 档位表（host 的权威表 + adapter 的 feature 门控表）───────────────────
const authz = new Map();
for (const [src, table] of [
  [read(AUTHZ), 'tauron_host::authz'],
  [adapterSrc, 'tauron_adapter（feature 门控）'],
]) {
  for (const block of src.matchAll(/CommandAuth\s*\{([\s\S]*?)\n\s*\},/g)) {
    const body = block[1];
    const command = body.match(/command:\s*"([^"]+)"/)?.[1];
    if (!command || authz.has(command)) continue;
    authz.set(command, {
      // 适配器侧的表写的是全限定名 `tauron_host::authz::AuthTier::Privileged`，
      // 只匹配 `AuthTier::` 会把 Privileged 打成 `?`（档位是承诺面，不能猜）。
      tier: body.match(/tier:\s*(?:[\w:]*::)?AuthTier::([A-Za-z_]+)/)?.[1] ?? '?',
      consumer: (body.match(/consumer:\s*"([^"]+)"/) || [])[1] ?? '',
      description: (body.match(/description:\s*"([^"]+)"/) || [])[1] ?? '',
      table,
    });
  }
}

// ── 5. 前端落点：哪些包按命令名发起调用（排除测试与契约包）──────────────────
function walkTs(dir, out = []) {
  for (const name of readdirSync(join(ROOT, dir))) {
    if (name === 'node_modules' || name === 'dist' || name.startsWith('.')) continue;
    const p = join(ROOT, dir, name);
    if (statSync(p).isDirectory()) walkTs(dir + '/' + name, out);
    else if (/\.tsx?$/.test(name)) out.push(dir + '/' + name);
  }
  return out;
}
const consumersByCmd = new Map();
for (const rel of walkTs('packages')) {
  if (/\.(test|spec)\.tsx?$/.test(rel) || rel.includes('contract-tests')) continue;
  const text = read(rel);
  const pkg = rel.split('/')[1];
  for (const name of defs.keys()) {
    if (!text.includes(`'${name}'`) && !text.includes(`"${name}"`)) continue;
    if (!consumersByCmd.has(name)) consumersByCmd.set(name, new Set());
    consumersByCmd.get(name).add(pkg);
  }
}

// ── 6. 渲染 ────────────────────────────────────────────────────────────────
const family = (c) => c.replace(/^host_/, '').split('_')[0];
const allNames = [...defs.keys()].sort();
const listOr = (names, empty) => (names.length ? `（${names.join('、')}）` : empty);

/** `///` 注释常以「`host_x`：」开头重复命令名；Markdown 链接在表格里是噪音。 */
const tidy = (s) =>
  oneLine(
    s
      .replace(/^`?host_[a-z0-9_]+`?\s*[：:]\s*/, '')
      .replace(/\[`([^`]+)`\]\([^)]*\)/g, '`$1`')
      .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1'),
  );

/** 档位（authz 登记）与判定（代码里真的执行的 require_*）分列呈现，不互相冒充。 */
const scopeOf = (name) => {
  const def = defs.get(name);
  const a = authz.get(name);
  // 不在档位表 ≠ 主窗专属：本仓的 `host_*` 走 root 注册，capability 文件不按命令名
  // 授权（示例里 `windows` 同时覆盖 `main` 与 `plugin-*`），所以「只有主窗能碰」这句
  // 口径唯一的真凭据是代码层判定。没有判定就如实写没有。
  const tier = a
    ? `\`${a.tier}\`（\`${a.table}\`）`
    : def.guards.length
      ? '不在档位表（主窗面）'
      : '不在档位表，**代码层无判定**';
  if (!def.guards.length) return tier;
  const via = def.guards
    .map((g) => (g.via === name ? g.guard : `${g.guard}（经 \`${g.via}\`）`))
    .join(' + ');
  return `${tier}；${via}`;
};

const table = (names) => {
  const rows = names
    .slice()
    .sort((a, b) => family(a).localeCompare(family(b)) || a.localeCompare(b))
    .map((name) => {
      const def = defs.get(name);
      const a = authz.get(name);
      const consumers = [...(consumersByCmd.get(name) || [])].sort();
      return [
        `\`${name}\``,
        cell(scopeOf(name)),
        def.params.length ? def.params.map((p) => '`' + cell(p) + '`').join('、') : '*无业务形参*',
        '`' + cell(def.ret) + '`',
        def.feature ? `\`${def.feature}\`` : '—',
        consumers.length ? consumers.map((c) => `\`${c}\``).join(' ') : '**缺（孤儿命令）**',
        cell(a ? tidy(a.description || a.consumer) : tidy(def.summary)),
      ].join(' | ');
    })
    .map((row) => `| ${row} |`);
  return (
    `**${names.length} 条**\n\n` +
    '| 命令 | 档位与判定 | Rust 业务形参 | 返回 | feature 门 | 前端落点（package） | 登记语义 |\n' +
    '|---|---|---|---|---|---|---|\n' +
    rows.join('\n')
  );
};

const unregistered = allNames.filter((n) => !inAnySet.has(n));
const orphanCommands = allNames.filter((n) => !(consumersByCmd.get(n) || new Set()).size);
const guardless = allNames.filter((n) => !authz.has(n) && !defs.get(n).guards.length);
const unexplained = allNames.filter(
  (n) => !authz.has(n) && !defs.get(n).guards.length && !defs.get(n).summary,
);

const doc = `# 命令面全量参考（自动生成，请勿手改）

> **这个文件是生成物**：\`node scripts/generate-command-surface.mjs\` 写，
> CI 用 \`pnpm command-surface:check\` 复算——它把「**代码有的命令，文档必须一条不漏**」
> 变成可执行断言，而不是某个人记得去补的表。
>
> 输入是五处事实，全部可复算：\`${TAURI}\` 的 \`#[tauri::command] pub fn host_*\`
> 定义（业务形参、返回类型、\`#[cfg(feature)]\` 门、函数上方 \`///\` 注释首句），
> \`${LIB}\` 的三个编译期命令集合（\`SUBSTRATE_COMMANDS\` /
> \`PLUGIN_RUNTIME_COMMANDS\` / \`PLUGIN_INSTALL_COMMANDS\`），
> \`tauron_host::authz\` 与适配器 feature 门控档位表，**沿委托链**（命令函数 →
> \`cmd_*_as\` → \`admin_gate\` / 私有 helper，深度上限 ${MAX_DEPTH}）收集到的判定
> （\`require_*\` 拒绝型、\`visible_notifications\` 按身份过滤、\`scoped_within_roots\`
> 根目录限定），以及 \`packages/*\` 中按命令名发起调用的位置。
> 语义细节仍写在各专题文档；本表只保证**不漏**与**形参/返回/判定与代码同形**。

## 计数口径

| 集合 | 条数 | 注册形态 |
|---|---|---|
| 底座 \`SUBSTRATE_COMMANDS\` | ${sets.substrate.length} | 只依赖适配器底座即可注册 |
| 插件运行时 \`PLUGIN_RUNTIME_COMMANDS\` | ${sets.runtime.length} | 插件面命令，档位登记在 authz 表 |
| 插件安装 \`PLUGIN_INSTALL_COMMANDS\` | ${sets.install.length} | \`plugin-install\` feature，**opt-in**（\`default = []\`） |
| **合计（去重）** | **${inAnySet.size}** | 默认构建实际可见条数见 \`contracts/public-surface-ledger.json\` |

四条判据同时钉在这里，任何一条变数都说明链路断了：

- 定义了但未落进任何集合的命令：**${unregistered.length}**${listOr(
  unregistered,
  '（无——每个 \`host_*\` 都有集合归属）',
)}
- 后端有、**前端无人调用**的孤儿命令：**${orphanCommands.length}**${listOr(
  orphanCommands,
  `（无——${allNames.length} 条全部有 \`packages/*\` 消费者）`,
)}
- 既不在档位表、沿委托链（含 \`admin_gate\` / \`*_payload\` 等中转函数）也找不到任何
  判定（\`require_*\` / 按身份过滤 / 根目录限定）的命令 —— 这类命令**任何有 IPC 访问的
  webview 都调得到，包括插件窗**，除非它只操作调用方自身：
  **${guardless.length}**${listOr(guardless, '（无）')}
- 上面那些命令里连 \`///\` 说明都没有的：**${unexplained.length}**${listOr(unexplained, '（无）')}

## 底座命令（主窗专属面）

${table(sets.substrate)}

## 插件运行时命令

${table(sets.runtime)}

## 插件安装命令（\`plugin-install\` feature，opt-in）

${table(sets.install)}

## 读表须知

1. **「档位」与「判定」是两回事**。档位（\`Self_\` / \`ScopedRead\` / \`Privileged\`）是
   authz 表里的登记，决定它在能力面（\`CAPABILITIES\`）里叫什么；判定是命令函数
   **或其委托链上的函数**（\`cmd_*_as\`、\`admin_gate\`、\`notifications_list_payload\` 等，
   表里注明经由哪个函数）实际执行的 \`require_*\` / 按身份过滤 / 根目录限定。
   **「不在档位表」不等于「主窗专属」**：本仓 \`host_*\` 走应用层 root 注册，能力文件
   （\`examples/minimal-app/src-tauri/capabilities/default.json\`）的 \`permissions\` 只有
   \`core:default\`、不按命令名授 ACL，而 \`windows\` 同时覆盖 \`main\` 与 \`plugin-*\`——
   所以 capability **管不到** \`host_*\` 的可达性。「只有主窗能碰」这句口径的唯一真凭据是
   代码层判定；本表最后一列如实写出有没有，**没判定就写没判定**。
2. **「代码层无判定」的两种合法情形**（逐条在「登记语义」里给出理由，理由必须在代码注释里）：
   ① 动作目标就是注入的**调用方自身**（如 \`host_window_close\` / \`set_size\` 用的是
   \`window.label()\`，不是入参 label，插件碰不到别人的窗口）；② 只读取值且不暴露跨主体
   拓扑（\`host_i18n_t\` / \`host_i18n_t_params\` 一次答一个键；\`host_brand_info\` 只读且
   诚实降级）。除此之外的新增命令必须带判定，否则 \`wire-gate\` 的清单断言会红。
3. **「前端落点」是链路证明**：列出 \`packages/*\` 里按该命令名发起调用的包。出现
   **缺（孤儿命令）** 即后端注册了却没有任何前端消费者，按仓库口径属孤儿逻辑——
   要么接线，要么删除并在 V4 台账登记。
4. **「Rust 业务形参」已剔除注入参数**（\`state\` / \`app\` / \`window\` /
   \`TauriCallerSource\` / \`State<..>\`），剩下的就是 IPC 请求体里该出现的键
   （Tauri 按 snake_case 参数名收；\`@tauron/host\` 侧的 camelCase → snake_case 映射与
   参数包规则见 \`docs/architecture/app-layer-wire.md\` 第 2 节）。
5. **「feature 门」列只登记命令函数自身的 \`#[cfg(feature)]\`**（写在
   \`#[tauri::command]\` 上方或下方都算）。集合归属才是 opt-in 的完整口径：\`plugin-install\`
   那 2 条既在编译期集合里也带自身 \`cfg\`，所以两处都会出现。
6. 返回类型里的 \`ProviderResult<..>\` / \`DegradedValue<..>\` / \`UnsupportedBody\`
   是**带降级语义的结果形状**（能力未就绪时返回 degraded 而不是报错）。这个形状
   **不等于**桩实现，也不等于已就绪——某个域到底是不是桩，看专题文档的「诚实边界」段。
7. 本表**不解释语义边界**。关键命令的线格式与错误契约见
   \`docs/architecture/app-layer-wire.md\`，面向插件作者的用法见
   \`docs/api/plugin-development-guide.md\`。
`;

const target = resolve(ROOT, OUT);
if (process.argv.includes('--check')) {
  let current = '';
  try {
    current = readFileSync(target, 'utf8');
  } catch {
    current = '';
  }
  if (current !== doc) {
    console.error(
      `✗ ${OUT} 与代码不同步（命令集、签名、档位、判定或前端落点变了）。` +
        ' 跑 `pnpm command-surface:gen` 重新生成后一并提交。',
    );
    process.exit(1);
  }
  console.log(
    `Command surface doc OK: ${allNames.length} commands（底座 ${sets.substrate.length} / ` +
      `运行时 ${sets.runtime.length} / 安装 ${sets.install.length}），孤儿命令 ${orphanCommands.length}，` +
      `未归类 ${unregistered.length}，无代码层判定 ${guardless.length}。`,
  );
} else {
  writeFileSync(target, doc);
  console.log(`wrote ${OUT}: ${allNames.length} commands，无代码层判定 ${guardless.length} 条`);
}
