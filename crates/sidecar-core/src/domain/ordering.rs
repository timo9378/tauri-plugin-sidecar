//! Dependency ordering — pure algorithm, no IO.

use std::collections::HashMap;

use super::config::SidecarConfig;
use super::error::SidecarError;

/// Kahn's algorithm over `depends_on`; rejects unknown deps and cycles.
pub fn topo_sort(configs: &[SidecarConfig]) -> Result<Vec<String>, SidecarError> {
    let names: std::collections::HashSet<_> = configs.iter().map(|c| c.name.as_str()).collect();
    for c in configs {
        for dep in &c.depends_on {
            if !names.contains(dep.as_str()) {
                return Err(SidecarError::UnknownDependency {
                    name: c.name.clone(),
                    dep: dep.clone(),
                });
            }
        }
    }

    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in configs {
        in_degree.entry(c.name.as_str()).or_insert(0);
        for dep in &c.depends_on {
            *in_degree.entry(c.name.as_str()).or_insert(0) += 1;
            dependents.entry(dep.as_str()).or_default().push(&c.name);
        }
    }

    let mut queue: Vec<&str> = in_degree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| *n)
        .collect();
    queue.sort_unstable(); // deterministic order

    let mut order = Vec::new();
    while let Some(name) = queue.pop() {
        order.push(name.to_string());
        for dependent in dependents.get(name).cloned().unwrap_or_default() {
            // Every dependent was inserted into in_degree in the first pass.
            let Some(d) = in_degree.get_mut(dependent) else {
                continue;
            };
            *d -= 1;
            if *d == 0 {
                queue.push(dependent);
            }
        }
    }

    if order.len() != configs.len() {
        let stuck = in_degree
            .iter()
            .find(|(_, d)| **d > 0)
            .map(|(n, _)| (*n).to_string())
            .unwrap_or_default();
        return Err(SidecarError::DependencyCycle(stuck));
    }
    Ok(order)
}
