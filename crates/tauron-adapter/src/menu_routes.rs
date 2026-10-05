// 菜单点击回传的路由表（轮 37）。
//
// 这里是「点击之后帧发到哪里」的唯一事实源。它单独成模块而不是留在 `lib.rs`，
// 有两个原因：一是 `lib.rs` 的条目预算（见 `contracts/adapter-domain-ownership.json`）
// 已接近上限，新增能力默认应开新域文件；二是这张表的两个读者（`tauri.rs` 的
// `TauriMenuSink` 与 `TauriTraySink`）都在 feature-gated 的平台层，把共享状态放在
// 平台无关的模块里才能离线测试。

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::MenuSpec;

/// 菜单/托盘点击回传的**约定 topic**（轮 37；注意它**不是**宿主补的默认值）。
///
/// 宿主只对 `MenuItemSpec.event` 里**显写了**的 topic 发帧，`None` 就是不发——
/// 本常量不会被替你填进 `event`。它的用途是让仓库内的接线（示例应用与
/// `@tauron/host` 的 `MenuClickFrame`）共用同一个值：两处字面量必然漂移，
/// 所以 TS 侧只允许 `packages/tauron-host/src/host-topics.ts` 再声明一次，
/// 由 `wire-gate` 逐字比对两个词表。
pub const MENU_CLICK_TOPIC: &str = "tauron://menu-click";

/// 菜单点击路由的**来源分道**。
///
/// Tauri 只有一个**全局**菜单事件监听表（`manager.menu.global_event_listeners`），
/// 应用菜单与托盘菜单的点击都进它——菜单项 id 因此是全局标识，不是"每个菜单自己的"。
/// 但两个来源的生命周期不同：`host_menu_reset` 撤掉应用菜单时，**不得**连带把托盘
/// 菜单的路由清掉（反向同理）。所以路由按 lane 各存一份，整条 lane 随它的来源一起替换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuLane {
    /// `host_menu_set` / `host_menu_popup` 建的菜单。
    AppMenu,
    /// `host_tray_create` / `host_tray_set_menu` 建的托盘菜单。
    TrayMenu,
}

impl MenuLane {
    /// 点击回传载荷里 `source` 字段的取值（TS `MenuClickFrame` 同词表）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppMenu => "menu",
            Self::TrayMenu => "tray",
        }
    }
}

/// `菜单项 id → 事件 topic` 的路由表，按 [`MenuLane`] 分道。
///
/// 平台实现（`tauri.rs` 的 `TauriMenuSink`）注册一个全局监听，命中即经
/// `AppHandle::emit` 把 `{ id, source, native: true }` 发到调用方在
/// `MenuItemSpec.event` 里指定的 topic。**不经过** `host_events_*` 事件总线——
/// 那套是另一条通路（订阅 + `host_events_drain` 取件），把两者混为一谈会让接线方
/// 去 `drain` 一个永远不会有帧的队列。
#[derive(Default)]
pub struct MenuRouteTable {
    lanes: Mutex<Vec<(MenuLane, HashMap<String, String>)>>,
}

impl MenuRouteTable {
    /// 用 `routes` **整体替换**一条 lane。
    ///
    /// 替换而非累加：来源撤掉菜单后，它的路由必须一起失效，否则残留条目会让
    /// "这个 id 已无人订阅"变成"偶尔还能收到旧 topic 的帧"。
    pub fn replace(&self, lane: MenuLane, routes: HashMap<String, String>) {
        let mut lanes = self.lanes.lock();
        if let Some(existing) = lanes.iter_mut().find(|(l, _)| *l == lane) {
            existing.1 = routes;
        } else {
            lanes.push((lane, routes));
        }
    }

    /// 查某个菜单项 id 的回传目标；命中时一并给出来源。
    ///
    /// 同一 id 同时登记在两条 lane 里时按**登记顺序**取第一条——这是调用方把 id 起的
    /// 重了（菜单项 id 全局唯一是 Tauri 的前提），本表不猜意图。
    pub fn lookup(&self, id: &str) -> Option<(MenuLane, String)> {
        self.lanes
            .lock()
            .iter()
            .find_map(|(lane, routes)| routes.get(id).map(|t| (*lane, t.clone())))
    }

    /// 某条 lane 当前登记了哪些 id（排序后返回，供测试与诊断）。
    pub fn ids(&self, lane: MenuLane) -> Vec<String> {
        let mut out: Vec<String> = self
            .lanes
            .lock()
            .iter()
            .find(|(l, _)| *l == lane)
            .map(|(_, routes)| routes.keys().cloned().collect())
            .unwrap_or_default();
        out.sort();
        out
    }
}

/// 进程内的菜单路由表（`TauriMenuSink` / `TauriTraySink` 共用）。
///
/// 用 `OnceLock` 而不是塞进 `SubstrateState`：注册全局监听的是**平台层**，而
/// `MenuSink`/`TraySink` 两个 trait 的签名里都没有总线或状态句柄，注入点也就无处可传。
pub fn menu_routes() -> &'static MenuRouteTable {
    static TABLE: std::sync::OnceLock<MenuRouteTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(MenuRouteTable::default)
}

/// 按规格收集 `id → topic`（`event` 为 `None` 的项不登记：只记录不发布）。
pub fn menu_routes_of(spec: &MenuSpec) -> HashMap<String, String> {
    spec.items
        .iter()
        .filter_map(|item| item.event.as_ref().map(|topic| (item.id.clone(), topic.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MenuItemSpec;

    fn item(id: &str, event: Option<&str>) -> MenuItemSpec {
        MenuItemSpec {
            id: id.to_string(),
            label: id.to_string(),
            enabled: true,
            event: event.map(|e| e.to_string()),
        }
    }

    fn spec(items: Vec<MenuItemSpec>) -> MenuSpec {
        MenuSpec { items }
    }

    #[test]
    fn lane_两道互不干扰() {
        let table = MenuRouteTable::default();
        table.replace(MenuLane::AppMenu, HashMap::from([("a".to_string(), "t-a".to_string())]));
        table.replace(MenuLane::TrayMenu, HashMap::from([("t".to_string(), "t-t".to_string())]));
        assert_eq!(table.ids(MenuLane::AppMenu), vec!["a".to_string()]);
        assert_eq!(table.ids(MenuLane::TrayMenu), vec!["t".to_string()]);
        // 撤掉应用菜单整道，不影响托盘道。
        table.replace(MenuLane::AppMenu, HashMap::new());
        assert!(table.ids(MenuLane::AppMenu).is_empty());
        assert_eq!(table.ids(MenuLane::TrayMenu), vec!["t".to_string()]);
        assert_eq!(table.lookup("t"), Some((MenuLane::TrayMenu, "t-t".to_string())));
        assert_eq!(table.lookup("a"), None);
    }

    #[test]
    fn replace_旧_id_随之失效() {
        let table = MenuRouteTable::default();
        table.replace(MenuLane::AppMenu, HashMap::from([("old".to_string(), "t-old".to_string())]));
        table.replace(MenuLane::AppMenu, HashMap::from([("new".to_string(), "t-new".to_string())]));
        assert_eq!(table.ids(MenuLane::AppMenu), vec!["new".to_string()]);
        // 残留条目不会继续把点击发往旧 topic。
        assert_eq!(table.lookup("old"), None);
    }

    #[test]
    fn lookup_同号按登记顺序取第一条() {
        let table = MenuRouteTable::default();
        table.replace(
            MenuLane::TrayMenu,
            HashMap::from([("dup".to_string(), "t-tray".to_string())]),
        );
        table.replace(MenuLane::AppMenu, HashMap::from([("dup".to_string(), "t-app".to_string())]));
        // 先登记的 lane 胜出（本表不猜意图）。
        assert_eq!(table.lookup("dup"), Some((MenuLane::TrayMenu, "t-tray".to_string())));
    }

    #[test]
    fn menu_routes_of_只登记带_event_的项() {
        let routes = menu_routes_of(&spec(vec![
            item("with", Some(MENU_CLICK_TOPIC)),
            item("without", None),
        ]));
        assert_eq!(routes.len(), 1);
        assert_eq!(routes.get("with").map(String::as_str), Some(MENU_CLICK_TOPIC));
        assert!(!routes.contains_key("without"));
    }

    #[test]
    fn lane_as_str_与_ts_词表一致() {
        assert_eq!(MenuLane::AppMenu.as_str(), "menu");
        assert_eq!(MenuLane::TrayMenu.as_str(), "tray");
    }

    #[test]
    fn 点击_topic_线值() {
        assert_eq!(MENU_CLICK_TOPIC, "tauron://menu-click");
    }
}
