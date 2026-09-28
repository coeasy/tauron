//! Acyclic service dependency graph for deterministic startup and reverse shutdown.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceNode {
    pub id: String,
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceGraphError {
    Duplicate(String),
    MissingDependency { service: String, dependency: String },
    Cycle(Vec<String>),
}

#[derive(Debug, Default)]
pub struct ServiceGraph {
    nodes: BTreeMap<String, ServiceNode>,
}

impl ServiceGraph {
    pub fn insert(&mut self, node: ServiceNode) -> Result<(), ServiceGraphError> {
        if self.nodes.contains_key(&node.id) {
            return Err(ServiceGraphError::Duplicate(node.id));
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn startup_order(&self) -> Result<Vec<String>, ServiceGraphError> {
        for node in self.nodes.values() {
            for dep in &node.requires {
                if !self.nodes.contains_key(dep) {
                    return Err(ServiceGraphError::MissingDependency {
                        service: node.id.clone(),
                        dependency: dep.clone(),
                    });
                }
            }
        }

        let mut indegree: BTreeMap<String, usize> =
            self.nodes.keys().map(|id| (id.clone(), 0usize)).collect();
        let mut reverse: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for node in self.nodes.values() {
            for dep in &node.requires {
                *indegree.get_mut(&node.id).expect("node exists") += 1;
                reverse.entry(dep.clone()).or_default().push(node.id.clone());
            }
        }

        let mut ready: BTreeSet<String> = indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());

        while let Some(id) = ready.pop_first() {
            order.push(id.clone());
            if let Some(children) = reverse.get(&id) {
                for child in children {
                    let degree = indegree.get_mut(child).expect("child exists");
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert(child.clone());
                    }
                }
            }
        }

        if order.len() != self.nodes.len() {
            let cycle = indegree
                .into_iter()
                .filter_map(|(id, degree)| (degree != 0).then_some(id))
                .collect();
            return Err(ServiceGraphError::Cycle(cycle));
        }
        Ok(order)
    }

    pub fn shutdown_order(&self) -> Result<Vec<String>, ServiceGraphError> {
        let mut order = self.startup_order()?;
        order.reverse();
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_is_reverse_topology() {
        let mut graph = ServiceGraph::default();
        graph.insert(ServiceNode { id: "contract".into(), requires: vec![] }).unwrap();
        graph.insert(ServiceNode { id: "policy".into(), requires: vec!["contract".into()] }).unwrap();
        graph.insert(ServiceNode { id: "runtime".into(), requires: vec!["policy".into()] }).unwrap();

        assert_eq!(graph.startup_order().unwrap(), vec!["contract", "policy", "runtime"]);
        assert_eq!(graph.shutdown_order().unwrap(), vec!["runtime", "policy", "contract"]);
    }

    #[test]
    fn missing_dependency_fails_before_start() {
        let mut graph = ServiceGraph::default();
        graph.insert(ServiceNode { id: "runtime".into(), requires: vec!["policy".into()] }).unwrap();
        assert!(matches!(
            graph.startup_order(),
            Err(ServiceGraphError::MissingDependency { .. })
        ));
    }

    #[test]
    fn dependency_cycle_is_hard_failure() {
        let mut graph = ServiceGraph::default();
        graph.insert(ServiceNode { id: "a".into(), requires: vec!["b".into()] }).unwrap();
        graph.insert(ServiceNode { id: "b".into(), requires: vec!["a".into()] }).unwrap();
        assert!(matches!(graph.startup_order(), Err(ServiceGraphError::Cycle(_))));
    }
}
