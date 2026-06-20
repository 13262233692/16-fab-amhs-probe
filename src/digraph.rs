use crate::event::OhtMoveEvent;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeWeight {
    pub from_node: String,
    pub to_node: String,
    pub flow_count: u64,
    pub blocked_count: u64,
    pub total_duration_ms: u64,
    pub avg_duration_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntersectionCongestion {
    pub intersection_id: String,
    pub total_flow: u64,
    pub blocked_count: u64,
    pub blocking_ratio: f64,
    pub connected_edges: usize,
    pub active_ohts: usize,
}

pub struct TrackGraph {
    graph: DiGraph<String, EdgeWeight>,
    node_map: HashMap<String, NodeIndex>,
    oht_positions: HashMap<String, String>,
}

impl TrackGraph {
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            node_map: HashMap::new(),
            oht_positions: HashMap::new(),
        }
    }

    pub fn build_from_events(&mut self, events: &[OhtMoveEvent]) {
        for evt in events {
            self.ensure_node(&evt.from_node);
            self.ensure_node(&evt.to_node);

            self.update_edge(evt);
            self.oht_positions.insert(evt.oht_id.clone(), evt.to_node.clone());
        }
    }

    fn ensure_node(&mut self, name: &str) {
        if !self.node_map.contains_key(name) {
            let idx = self.graph.add_node(name.to_string());
            self.node_map.insert(name.to_string(), idx);
        }
    }

    fn update_edge(&mut self, evt: &OhtMoveEvent) {
        let from_idx = self.node_map[&evt.from_node];
        let to_idx = self.node_map[&evt.to_node];

        if let Some(edge_idx) = self.graph.find_edge(from_idx, to_idx) {
            let weight = self.graph.edge_weight_mut(edge_idx).unwrap();
            weight.flow_count += 1;
            if evt.event_type == crate::event::MoveEventType::Blocked {
                weight.blocked_count += 1;
            }
            weight.total_duration_ms += evt.duration_ms;
            weight.avg_duration_ms =
                weight.total_duration_ms as f64 / weight.flow_count as f64;
        } else {
            let weight = EdgeWeight {
                from_node: evt.from_node.clone(),
                to_node: evt.to_node.clone(),
                flow_count: 1,
                blocked_count: if evt.event_type == crate::event::MoveEventType::Blocked {
                    1
                } else {
                    0
                },
                total_duration_ms: evt.duration_ms,
                avg_duration_ms: evt.duration_ms as f64,
            };
            self.graph.add_edge(from_idx, to_idx, weight);
        }
    }

    pub fn top_congested_intersections(&self, top_n: usize) -> Vec<IntersectionCongestion> {
        let mut node_flow: HashMap<String, (u64, u64, usize, usize)> = HashMap::new();

        for node_idx in self.graph.node_indices() {
            let node_name = &self.graph[node_idx];
            let mut total_incoming: u64 = 0;
            let mut total_blocked: u64 = 0;
            let mut edge_count: usize = 0;

            for edge in self.graph.edges(node_idx) {
                total_incoming += edge.weight().flow_count;
                total_blocked += edge.weight().blocked_count;
                edge_count += 1;
            }

            for edge in self.graph.edges_directed(node_idx, petgraph::Direction::Incoming) {
                total_incoming += edge.weight().flow_count;
                total_blocked += edge.weight().blocked_count;
                edge_count += 1;
            }

            let active_ohts = self
                .oht_positions
                .values()
                .filter(|pos| *pos == node_name)
                .count();

            let entry = node_flow
                .entry(node_name.clone())
                .or_insert((0, 0, 0, 0));
            entry.0 += total_incoming;
            entry.1 += total_blocked;
            entry.2 += edge_count;
            entry.3 += active_ohts;
        }

        let mut intersections: Vec<IntersectionCongestion> = node_flow
            .into_iter()
            .filter(|(_, (flow, _, _, _))| *flow > 0)
            .map(
                |(id, (total_flow, blocked_count, connected_edges, active_ohts))| {
                    let blocking_ratio = if total_flow > 0 {
                        blocked_count as f64 / total_flow as f64
                    } else {
                        0.0
                    };
                    IntersectionCongestion {
                        intersection_id: id,
                        total_flow,
                        blocked_count,
                        blocking_ratio,
                        connected_edges,
                        active_ohts,
                    }
                },
            )
            .collect();

        intersections.sort_by(|a, b| b.total_flow.cmp(&a.total_flow));
        intersections.truncate(top_n);
        intersections
    }

    pub fn graph_stats(&self) -> GraphStats {
        let total_flow: u64 = self
            .graph
            .edge_indices()
            .map(|e| self.graph.edge_weight(e).unwrap().flow_count)
            .sum();
        let total_blocked: u64 = self
            .graph
            .edge_indices()
            .map(|e| self.graph.edge_weight(e).unwrap().blocked_count)
            .sum();

        GraphStats {
            node_count: self.graph.node_count(),
            edge_count: self.graph.edge_count(),
            total_flow,
            total_blocked,
            unique_ohts: self.oht_positions.len(),
        }
    }

    pub fn export_json(&self) -> serde_json::Value {
        let nodes: Vec<serde_json::Value> = self
            .graph
            .node_indices()
            .map(|idx| {
                serde_json::json!({
                    "id": self.graph[idx],
                    "index": idx.index(),
                })
            })
            .collect();

        let edges: Vec<serde_json::Value> = self
            .graph
            .edge_indices()
            .map(|eidx| {
                let (src, dst) = self.graph.edge_endpoints(eidx).unwrap();
                let w = self.graph.edge_weight(eidx).unwrap();
                serde_json::json!({
                    "source": self.graph[src],
                    "target": self.graph[dst],
                    "flow": w.flow_count,
                    "blocked": w.blocked_count,
                    "avg_duration_ms": w.avg_duration_ms,
                })
            })
            .collect();

        serde_json::json!({
            "nodes": nodes,
            "edges": edges,
        })
    }
}

#[derive(Debug)]
pub struct GraphStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub total_flow: u64,
    pub total_blocked: u64,
    pub unique_ohts: usize,
}
