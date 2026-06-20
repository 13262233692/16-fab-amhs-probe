use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OhtMoveEvent {
    pub timestamp: i64,
    pub oht_id: String,
    pub from_node: String,
    pub to_node: String,
    pub event_type: MoveEventType,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveEventType {
    Depart,
    Arrive,
    PassThrough,
    Blocked,
    EmergencyStop,
}

impl fmt::Display for MoveEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MoveEventType::Depart => write!(f, "DEPART"),
            MoveEventType::Arrive => write!(f, "ARRIVE"),
            MoveEventType::PassThrough => write!(f, "PASS"),
            MoveEventType::Blocked => write!(f, "BLOCKED"),
            MoveEventType::EmergencyStop => write!(f, "ESTOP"),
        }
    }
}

impl OhtMoveEvent {
    #[allow(dead_code)]
    pub fn edge_key(&self) -> String {
        format!("{}->{}", self.from_node, self.to_node)
    }

    #[allow(dead_code)]
    pub fn intersection_key(&self) -> Option<String> {
        let from_parts: Vec<&str> = self.from_node.split('-').collect();
        let to_parts: Vec<&str> = self.to_node.split('-').collect();
        if from_parts.len() >= 2 && to_parts.len() >= 2 {
            if from_parts[0] == to_parts[0] {
                Some(format!("IX-{}", from_parts[0]))
            } else {
                None
            }
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn timestamp_str(&self) -> String {
        Utc.timestamp_millis_opt(self.timestamp)
            .earliest()
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
            .unwrap_or_else(|| self.timestamp.to_string())
    }
}
