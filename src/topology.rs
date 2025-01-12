//! Implements the `Topology` struct for managing and analyzing network connectivity.

use bitvec::prelude::*;
use std::{
    collections::VecDeque,
    fmt::Display,
    time::{Duration, Instant},
    usize,
};
use wg_2024::{network::NodeId, packet::NodeType};

const NETWORK_SIZE: usize = 256;

/// Estimated duration after which the topology is considered fully updated.
const ESTIMATED_UPDATE_TIME: Duration = Duration::from_secs(3);

/// Represents the network topology as an undirected graph.
///
/// The `Topology` struct tracks connections between nodes and their types.
/// It uses a bit matrix (`BitArray`) to represent edges between nodes.
pub struct Topology {
    graph: [BitArray<[u8; 32]>; NETWORK_SIZE],
    types: [NodeType; NETWORK_SIZE],
    last_reset: Instant,
}

#[derive(Debug)]
pub enum RoutingError {
    NoPathFound,
    SourceIsDest,
}

impl Topology {
    pub fn new() -> Self {
        Self {
            graph: [BitArray::new([0; 32]); NETWORK_SIZE],
            types: [NodeType::Drone; NETWORK_SIZE],
            last_reset: Instant::now() - ESTIMATED_UPDATE_TIME,
        }
    }

    /// Inserts an edge between two nodes in the topology.
    pub fn insert_edge(&mut self, node1: (NodeId, NodeType), node2: (NodeId, NodeType)) {
        let node1_id = node1.0 as usize;
        let node2_id = node2.0 as usize;

        self.graph[node1_id].set(node2_id, true);
        self.graph[node2_id].set(node1_id, true);

        self.types[node1_id] = node1.1;
        self.types[node2_id] = node2.1;
    }

    /// Removes an edge between two nodes in the topology.
    pub fn remove_edge(&mut self, node1_id: NodeId, node2_id: NodeId) {
        let n1_id = node1_id as usize;
        let n2_id = node2_id as usize;

        self.graph[n1_id].set(n2_id, false);
        self.graph[n2_id].set(n1_id, false);
    }

    /// Finds the shortest path between two nodes using BFS.
    ///
    /// # Arguments
    ///
    /// * `source` - The ID of the source node.
    /// * `dest` - The ID of the destination node.
    ///
    /// # Returns
    ///
    /// - `Ok(Vec<NodeId>)` if a path is found.
    /// - `Err(RoutingError::NoPathFound)` if no path exists.
    /// - `Err(RoutingError::SourceIsDest)` if the source is the same as the destination.
    ///
    /// # Notes
    ///
    /// Routing through a node of type `NodeType::Drone` is not allowed.
    pub fn bfs(&self, source: NodeId, dest: NodeId) -> Result<Vec<NodeId>, RoutingError> {
        let source_id = source as usize;
        let dest_id = dest as usize;

        if source == dest {
            return Err(RoutingError::SourceIsDest);
        }

        let mut visited = vec![false; NETWORK_SIZE];
        let mut parent = vec![None; NETWORK_SIZE];
        let mut queue = VecDeque::new();

        visited[source_id] = true;
        queue.push_back(source_id);

        while let Some(current) = queue.pop_front() {
            for neighbor in self.graph[current].iter_ones() {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    parent[neighbor] = Some(current);

                    if neighbor == dest_id {
                        let mut path = vec![dest];
                        let mut current_node = dest_id;

                        while let Some(p) = parent[current_node] {
                            path.push(p as NodeId);
                            current_node = p;
                        }

                        path.reverse();
                        return Ok(path);

                    //Routing with a node different from drone in the middle is illegal
                    } else if self.types[neighbor] == NodeType::Drone {
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        Err(RoutingError::NoPathFound)
    }

    /// Resets the topology by clearing all edges and resetting node types.
    pub fn reset(&mut self) {
        self.graph = [BitArray::new([0; 32]); NETWORK_SIZE];
        self.types = [NodeType::Drone; NETWORK_SIZE];

        self.last_reset = Instant::now()
    }

    /// Checks if the topology is currently updating.
    ///
    /// # Returns
    ///
    /// `true` if the last reset occurred within the estimated update time; otherwise, `false`.
    pub fn is_updating(&self) -> bool {
        self.last_reset.elapsed() < ESTIMATED_UPDATE_TIME
    }
}

impl Display for Topology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut res = String::new();

        for (i, node) in self.graph.iter().enumerate() {
            if node.count_ones() > 0 {
                res.push_str(&format!("Node {}: ", i));
                let connections: Vec<usize> = node.iter_ones().collect();
                res.push_str(&format!("connected to {:?} ", connections));
            }
        }

        if res.is_empty() {
            res = "No connections in the topology".to_string();
        }

        write!(f, "{}", res)
    }
}
