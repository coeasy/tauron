//! V4 §87.3 zero-surface ReadinessSet（A66）。
//!
//! ReadinessSet 把「就绪 = 哪些必备项全部就绪」从散落各子系统的布尔量收成一个可
//! 求值的集合：kernel / contract / providers / runtimes / surfaces / product
//! predicates 六组事实。**零 Surface**（headless / service 形态）时
//! `required_surfaces` 是空集——空集不阻塞就绪；但一旦声明了某个 surface，
//! 它没就绪就必须具名出现在 `evaluate().blockers` 里，不许被静默跳过。
//! `health()` 把求值结果接回 A103 的 `HealthReport`（liveness/readiness/degraded
//! 分离的单一入口）。

use crate::health::HealthReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessFact {
    pub id: String,
    pub ready: bool,
    pub reason: Option<String>,
}

impl ReadinessFact {
    pub fn ready(id: impl Into<String>) -> Self {
        Self { id: id.into(), ready: true, reason: None }
    }

    pub fn blocked(id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self { id: id.into(), ready: false, reason: Some(reason.into()) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessSet {
    pub kernel: ReadinessFact,
    pub contract: ReadinessFact,
    pub required_providers: Vec<ReadinessFact>,
    pub required_runtimes: Vec<ReadinessFact>,
    /// 零 Surface 形态下为空集；桌面形态下逐个登记（window / tray / menu…）。
    pub required_surfaces: Vec<ReadinessFact>,
    pub product_predicates: Vec<ReadinessFact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationReadiness {
    pub ready: bool,
    pub blockers: Vec<String>,
}

impl ReadinessSet {
    /// 零 Surface（headless / service）形态：只有 kernel 与 contract 两件必答事实，
    /// 其余五组为空；空集在 `evaluate` 中天然不阻塞。
    pub fn headless(kernel: ReadinessFact, contract: ReadinessFact) -> Self {
        Self {
            kernel,
            contract,
            required_providers: Vec::new(),
            required_runtimes: Vec::new(),
            required_surfaces: Vec::new(),
            product_predicates: Vec::new(),
        }
    }

    fn all_facts(&self) -> impl Iterator<Item = &ReadinessFact> {
        std::iter::once(&self.kernel)
            .chain(std::iter::once(&self.contract))
            .chain(self.required_providers.iter())
            .chain(self.required_runtimes.iter())
            .chain(self.required_surfaces.iter())
            .chain(self.product_predicates.iter())
    }

    pub fn evaluate(&self) -> ApplicationReadiness {
        let blockers = self
            .all_facts()
            .filter(|fact| !fact.ready)
            .map(|fact| match &fact.reason {
                Some(reason) => format!("{}: {reason}", fact.id),
                None => fact.id.clone(),
            })
            .collect::<Vec<_>>();
        ApplicationReadiness { ready: blockers.is_empty(), blockers }
    }

    pub fn health(&self) -> HealthReport {
        let evaluated = self.evaluate();
        if evaluated.ready {
            HealthReport::ready()
        } else {
            HealthReport::alive_but_not_ready(evaluated.blockers.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_ready_fact_without_reason_reports_bare_id() {
        let mut set = ReadinessSet::headless(
            ReadinessFact::ready("kernel"),
            ReadinessFact::ready("contract"),
        );
        set.required_runtimes.push(ReadinessFact { id: "wasm".into(), ready: false, reason: None });
        let evaluated = set.evaluate();
        assert!(!evaluated.ready);
        assert_eq!(evaluated.blockers, vec!["wasm".to_string()]);
    }
}
