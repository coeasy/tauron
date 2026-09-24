// §4.13 schema 规范化管线（`tauron-schema`）。
//
// 架构 §6.2 定稿管线：
//
// ```text
// Rust struct
//   │ #[derive(JsonSchema)] + #[schemars(extend("x-tauron", ...))]
//   ▼  schemars 1.2.2（draft 2020-12）
// JSON Schema（$defs/$ref、enum→oneOf）          ← codegen 可复现这一形态
//   │ ① $ref 本地展开  ② enum/Option 规范化  ③ x-tauron → uiSchema
//   ▼
// 平面 schema + uiSchema（同一份喂 RJSF 与 WC 渲染器）
//   │
//   ▼ 写回前按 schema 校验（含范围/枚举）
// ```
//
// 为什么要这条管线（ADR-12）：schemars 1.2.2 默认 draft 2020-12，**必然**
// 产出 `$defs`+`$ref`，且 **Rust enum 默认产出 `oneOf`**——正撞 RJSF
// #4666（嵌套 oneOf/anyOf 不支持）与 #4505（bundled `$ref` 未支持），
// 表现为"表单渲染不出来"且报错难懂。
//
// 门禁 §8-12（见 {@link gate::assert_flat}）：产物中禁止出现嵌套
// oneOf/anyOf 与未展开 `$ref`；Rust enum 必须内部标签化。
//
// 本 crate 的错误类型不跨 IPC：schema 管线在构建期/安装期/写入前被消费，
// 由 §4.12 设置中心翻译成 `tauron_host::error::HostError`。

pub mod codegen;
pub mod error;
pub mod gate;
pub mod pipeline;
pub mod validate;
pub mod xoc;

pub use codegen::{generate, Def, DRAFT_2020_12};
pub use error::{SchemaError, SchemaResult};
pub use gate::{assert_flat, report, ShapeReport};
pub use pipeline::{compile, compile_to_doc, Compiled, MAX_REF_DEPTH};
pub use validate::{into_schema_error, validate, ValidationError};
pub use xoc::{ALL_WIDGETS, EXT_KEY, KNOWN_KEYS, XOC_VERSION, Extension};
