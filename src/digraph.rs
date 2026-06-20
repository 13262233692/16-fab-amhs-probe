use crate::event::OhtMoveEvent;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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

#[derive(Debug, Clone, Copy)]
pub enum MergeStrategy {
    SemanticBuckets,
    #[allow(dead_code)]
    FixedBuckets(usize),
    MaxNodes(usize),
    None,
}

pub struct NodeDownsampler {
    strategy: MergeStrategy,
    alias_map: HashMap<String, String>,
    semantic_prefixes: HashSet<String>,
    bucket_interval: u32,
}

impl NodeDownsampler {
    pub fn new(strategy: MergeStrategy) -> Self {
        let mut prefixes = HashSet::new();
        for p in &[
            "IX", "TRK", "BNK", "STK", "EQP", "OUT", "IN", "BUF", "LP", "PO",
            "STN", "LOAD", "UNLD", "PORT", "TEMP", "ZONE", "AREA",
        ] {
            prefixes.insert(p.to_string());
        }

        let bucket_interval = match strategy {
            MergeStrategy::FixedBuckets(n) => n as u32,
            _ => 10,
        };

        Self {
            strategy,
            alias_map: HashMap::new(),
            semantic_prefixes: prefixes,
            bucket_interval,
        }
    }

    pub fn with_semantic_buckets(bucket_interval: u32) -> Self {
        Self::new(MergeStrategy::SemanticBuckets).set_interval(bucket_interval)
    }

    pub fn with_max_nodes(max_nodes: usize) -> Self {
        Self::new(MergeStrategy::MaxNodes(max_nodes))
    }

    pub fn no_merge() -> Self {
        Self::new(MergeStrategy::None)
    }

    pub fn set_interval(mut self, interval: u32) -> Self {
        self.bucket_interval = interval;
        self
    }

    pub fn resolve(&mut self, node: &str) -> String {
        if let MergeStrategy::None = self.strategy {
            return node.to_string();
        }

        if let Some(alias) = self.alias_map.get(node) {
            return alias.clone();
        }

        let resolved = self.compute_bucket(node);
        self.alias_map.insert(node.to_string(), resolved.clone());
        resolved
    }

    fn compute_bucket(&self, node: &str) -> String {
        match self.strategy {
            MergeStrategy::None => node.to_string(),
            MergeStrategy::SemanticBuckets | MergeStrategy::FixedBuckets(_) => {
                self.semantic_bucket(node)
            }
            MergeStrategy::MaxNodes(_) => node.to_string(),
        }
    }

    fn semantic_bucket(&self, node: &str) -> String {
        let bytes = node.as_bytes();
        let mut prefix_end = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b.is_ascii_alphabetic() {
                prefix_end = i + 1;
            } else {
                break;
            }
        }

        if prefix_end == 0 {
            return "MISC-OTHER".to_string();
        }

        let prefix = &node[..prefix_end];

        if !self.semantic_prefixes.contains(prefix) {
            return format!("MISC-{}", prefix);
        }

        let rest = &node[prefix_end..];

        let sep_pos = rest.find(|c: char| c.is_ascii_digit());
        let num_start = match sep_pos {
            Some(pos) => prefix_end + pos,
            None => return format!("{}-GROUP", prefix),
        };

        let mut num_str = String::new();
        for &b in &node.as_bytes()[num_start..] {
            if b.is_ascii_digit() {
                num_str.push(b as char);
            } else {
                break;
            }
        }

        let num: u32 = match num_str.parse() {
            Ok(n) => n,
            Err(_) => return format!("{}-GROUP", prefix),
        };

        let interval = self.bucket_interval;
        let bucket = (num / interval) * interval;
        let bucket_end = bucket + interval - 1;

        format!("{}-{:03}-{:03}", prefix, bucket, bucket_end)
    }

    pub fn apply_max_nodes_limit(
        &mut self,
        event_counts: &HashMap<String, u64>,
    ) {
        if let MergeStrategy::MaxNodes(limit) = self.strategy {
            let unique: HashSet<String> = self.alias_map.values().cloned().collect();
            if unique.len() <= limit {
                return;
            }

            let mut counts: Vec<(String, u64)> = unique
                .into_iter()
                .map(|k| {
                    let count = event_counts.get(&k).copied().unwrap_or(0);
                    (k, count)
                })
                .collect();
            counts.sort_by(|a, b| b.1.cmp(&a.1));

            let top: HashSet<String> = counts
                .iter()
                .take(limit.saturating_sub(1))
                .map(|(k, _)| k.clone())
                .collect();

            let merged_name = "OVERFLOW-MERGED".to_string();
            for (node, _) in &counts {
                if !top.contains(node) {
                    let raw_nodes: Vec<String> = self
                        .alias_map
                        .iter()
                        .filter(|(_, v)| *v == node)
                        .map(|(k, _)| k.clone())
                        .collect();
                    for raw in raw_nodes {
                        self.alias_map.insert(raw, merged_name.clone());
                    }
                }
            }
        }
    }

    pub fn merge_stats(&self) -> (usize, usize) {
        let unique: HashSet<&String> = self.alias_map.values().collect();
        (self.alias_map.len(), unique.len())
    }
}

impl Default for NodeDownsampler {
    fn default() -> Self {
        Self::with_semantic_buckets(10)
    }
}

pub struct TrackGraph {
    graph: DiGraph<String, EdgeWeight>,
    node_map: HashMap<String, NodeIndex>,
    oht_positions: HashMap<String, String>,
    downsampler: NodeDownsampler,
}

impl TrackGraph {
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            node_map: HashMap::new(),
            oht_positions: HashMap::new(),
            downsampler: NodeDownsampler::default(),
        }
    }

    pub fn with_downsampler(downsampler: NodeDownsampler) -> Self {
        Self {
            graph: DiGraph::new(),
            node_map: HashMap::new(),
            oht_positions: HashMap::new(),
            downsampler,
        }
    }

    pub fn build_from_events(&mut self, events: &[OhtMoveEvent]) {
        let mut node_counts: HashMap<String, u64> = HashMap::new();

        for evt in events {
            let from_key = self.downsampler.resolve(&evt.from_node);
            let to_key = self.downsampler.resolve(&evt.to_node);

            *node_counts.entry(from_key.clone()).or_insert(0) += 1;
            *node_counts.entry(to_key.clone()).or_insert(0) += 1;

            self.ensure_node(&from_key);
            self.ensure_node(&to_key);

            self.update_edge(evt, &from_key, &to_key);
            self.oht_positions
                .insert(evt.oht_id.clone(), to_key);
        }

        self.downsampler.apply_max_nodes_limit(&node_counts);

        let merge_info = self.downsampler.merge_stats();
        if merge_info.0 > 0 {
            log::info!(
                "节点降采样: {} 原始节点 -> {} 桶节点 (压缩率 {:.1}%)",
                merge_info.0,
                merge_info.1,
                if merge_info.0 > 0 {
                    100.0 * (1.0 - merge_info.1 as f64 / merge_info.0 as f64)
                } else {
                    0.0
                }
            );
        }
    }

    fn ensure_node(&mut self, name: &str) {
        if !self.node_map.contains_key(name) {
            let idx = self.graph.add_node(name.to_string());
            self.node_map.insert(name.to_string(), idx);
        }
    }

    fn update_edge(&mut self, evt: &OhtMoveEvent, from_key: &str, to_key: &str) {
        let from_idx = self.node_map[from_key];
        let to_idx = self.node_map[to_key];

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
                from_node: from_key.to_string(),
                to_node: to_key.to_string(),
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

            for edge in self
                .graph
                .edges_directed(node_idx, petgraph::Direction::Incoming)
            {
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
