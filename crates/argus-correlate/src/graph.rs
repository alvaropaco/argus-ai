//! The operational environment graph: typed nodes and edges with ownership and
//! dependency traversals (CAP-5).

use std::collections::{HashMap, HashSet};

use argus_domain::{NodeKind, RelationshipKind, ResourceId};

/// A directed edge: `from` is related to `to` via `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: ResourceId,
    pub to: ResourceId,
    pub kind: RelationshipKind,
}

/// An in-memory typed graph over the environment's resources.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    nodes: HashMap<ResourceId, NodeKind>,
    edges: Vec<Edge>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, id: ResourceId, kind: NodeKind) {
        self.nodes.insert(id, kind);
    }

    pub fn add_edge(&mut self, from: ResourceId, to: ResourceId, kind: RelationshipKind) {
        self.edges.push(Edge { from, to, kind });
    }

    pub fn kind(&self, id: &ResourceId) -> Option<NodeKind> {
        self.nodes.get(id).copied()
    }

    pub fn nodes(&self) -> impl Iterator<Item = (&ResourceId, NodeKind)> {
        self.nodes.iter().map(|(id, kind)| (id, *kind))
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Transitive ancestors via ownership edges (`RunsOn`/`BelongsTo`/
    /// `ManagedBy`/`DeployedBy`). Returns every node that (transitively) owns
    /// `id`, sorted for determinism.
    pub fn owners(&self, id: &ResourceId) -> Vec<ResourceId> {
        self.traverse_ownership(id)
    }

    /// The nearest owner of `id` of the given `kind`, if any. This answers
    /// "which application owns this process" / "which deployment owns this
    /// container".
    pub fn nearest_owner_of_kind(&self, id: &ResourceId, kind: NodeKind) -> Option<ResourceId> {
        // BFS by distance so the nearest match wins.
        let mut visited: HashSet<ResourceId> = HashSet::new();
        let mut frontier: Vec<ResourceId> = vec![id.clone()];
        while let Some(current) = frontier.pop() {
            for edge in &self.edges {
                if edge.from == current && edge.kind.is_ownership() && !visited.contains(&edge.to) {
                    if self.nodes.get(&edge.to).copied() == Some(kind) {
                        return Some(edge.to.clone());
                    }
                    visited.insert(edge.to.clone());
                    frontier.push(edge.to.clone());
                }
            }
        }
        None
    }

    /// Transitive dependents: every node that (transitively) `DependsOn` `id`,
    /// sorted for determinism.
    pub fn dependents(&self, id: &ResourceId) -> Vec<ResourceId> {
        let mut result = Vec::new();
        let mut visited: HashSet<ResourceId> = HashSet::new();
        let mut stack = vec![id.clone()];
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            for edge in &self.edges {
                if edge.to == current
                    && edge.kind == RelationshipKind::DependsOn
                    && !visited.contains(&edge.from)
                {
                    stack.push(edge.from.clone());
                    result.push(edge.from.clone());
                }
            }
        }
        result.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        result
    }

    /// Transitive dependencies: every node that `id` (transitively) `DependsOn`,
    /// sorted for determinism.
    pub fn dependencies(&self, id: &ResourceId) -> Vec<ResourceId> {
        let mut result = Vec::new();
        let mut visited: HashSet<ResourceId> = HashSet::new();
        let mut stack = vec![id.clone()];
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            for edge in &self.edges {
                if edge.from == current
                    && edge.kind == RelationshipKind::DependsOn
                    && !visited.contains(&edge.to)
                {
                    stack.push(edge.to.clone());
                    result.push(edge.to.clone());
                }
            }
        }
        result.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        result
    }

    fn traverse_ownership(&self, id: &ResourceId) -> Vec<ResourceId> {
        let mut result = Vec::new();
        let mut visited: HashSet<ResourceId> = HashSet::new();
        let mut stack = vec![id.clone()];
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            for edge in &self.edges {
                if edge.from == current && edge.kind.is_ownership() && !visited.contains(&edge.to) {
                    stack.push(edge.to.clone());
                    result.push(edge.to.clone());
                }
            }
        }
        result.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(kind: &str, id: &str) -> ResourceId {
        ResourceId::new(kind, id).unwrap()
    }

    fn sample_graph() -> Graph {
        let mut g = Graph::new();
        let process = rid("process", "1234");
        let container = rid("container", "abc");
        let pod = rid("k8s-pod", "checkout-7d9f");
        let deployment = rid("k8s-deployment", "checkout-api");
        let app = rid("application", "checkout-api");
        let db = rid("service", "postgres");

        g.add_node(process.clone(), NodeKind::Process);
        g.add_node(container.clone(), NodeKind::Container);
        g.add_node(pod.clone(), NodeKind::K8sPod);
        g.add_node(deployment.clone(), NodeKind::K8sDeployment);
        g.add_node(app.clone(), NodeKind::Application);
        g.add_node(db.clone(), NodeKind::Service);

        g.add_edge(process, container.clone(), RelationshipKind::BelongsTo);
        g.add_edge(container, pod.clone(), RelationshipKind::BelongsTo);
        g.add_edge(pod, deployment.clone(), RelationshipKind::ManagedBy);
        g.add_edge(deployment, app.clone(), RelationshipKind::DeployedBy);
        g.add_edge(app, db, RelationshipKind::DependsOn);
        g
    }

    #[test]
    fn owners_are_transitive() {
        let g = sample_graph();
        let owners = g.owners(&rid("process", "1234"));
        assert_eq!(
            owners,
            vec![
                rid("application", "checkout-api"),
                rid("container", "abc"),
                rid("k8s-deployment", "checkout-api"),
                rid("k8s-pod", "checkout-7d9f"),
            ]
        );
    }

    #[test]
    fn nearest_owner_of_kind_finds_the_application() {
        let g = sample_graph();
        let app = g
            .nearest_owner_of_kind(&rid("process", "1234"), NodeKind::Application)
            .unwrap();
        assert_eq!(app, rid("application", "checkout-api"));
    }

    #[test]
    fn nearest_owner_of_kind_finds_the_deployment() {
        let g = sample_graph();
        let dep = g
            .nearest_owner_of_kind(&rid("container", "abc"), NodeKind::K8sDeployment)
            .unwrap();
        assert_eq!(dep, rid("k8s-deployment", "checkout-api"));
    }

    #[test]
    fn dependents_find_what_depends_on_a_resource() {
        let g = sample_graph();
        let dependents = g.dependents(&rid("service", "postgres"));
        assert_eq!(dependents, vec![rid("application", "checkout-api")]);
    }

    #[test]
    fn dependencies_find_what_a_resource_depends_on() {
        let g = sample_graph();
        let deps = g.dependencies(&rid("application", "checkout-api"));
        assert_eq!(deps, vec![rid("service", "postgres")]);
    }

    #[test]
    fn no_owners_for_isolated_node() {
        let mut g = Graph::new();
        g.add_node(rid("host", "x"), NodeKind::Host);
        assert!(g.owners(&rid("host", "x")).is_empty());
    }
}
