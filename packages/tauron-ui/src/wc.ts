// `@tauron/ui/wc` 兼容转出（R3 后组件归属 `@tauron/ui-primitives`）。
//
// 保留本入口是为了让既有的 `import '@tauron/ui/wc'`（注册自定义元素）继续可用；
// 新代码可以直接 `import '@tauron/ui-primitives/wc'`。
export * from '@tauron/ui-primitives/wc';
