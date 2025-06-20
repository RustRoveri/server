//! Implements the `Topology` struct for managing and analyzing network connectivity.

use bitvec::prelude::*;
use std::{
    cmp::Ordering,
    collections::{BinaryHeap, VecDeque},
    fmt::Display,
    time::{Duration, Instant},
    usize,
};
use wg_2024::{network::NodeId, packet::NodeType};

const NETWORK_SIZE: usize = 256;

/// Estimated duration after which the topology is considered fully updated.
const ESTIMATED_UPDATE_TIME: Duration = Duration::from_secs(2);

struct Rate {
    pub success: f64,
    pub failure: f64,
}

#[derive(Copy, Clone, PartialEq)]
struct State {
    pdr: f64,
    position: usize,
}

impl Eq for State {}

impl Ord for State {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .pdr
            .total_cmp(&self.pdr)
            .then_with(|| self.position.cmp(&other.position))
    }
}

impl PartialOrd for State {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Represents the network topology as an undirected graph.
///
/// The `Topology` struct tracks connections between nodes and their types.
/// It uses a bit matrix (`BitArray`) to represent edges between nodes.
pub struct Topology {
    node_id: NodeId,
    graph: [BitArray<[u8; 32]>; NETWORK_SIZE],
    types: [NodeType; NETWORK_SIZE],
    observed_trend: [Rate; NETWORK_SIZE],
    last_reset: Instant,
}

#[derive(Debug)]
pub enum RoutingError {
    NoPathFound,
    SourceIsDest,
}

impl Topology {
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            graph: [BitArray::new([0; 32]); NETWORK_SIZE],
            types: {
                let mut types = [NodeType::Drone; NETWORK_SIZE];
                types[node_id as usize] = NodeType::Server;
                types
            },
            observed_trend: [const {
                Rate {
                    success: 1.0,
                    failure: 1.0,
                }
            }; NETWORK_SIZE],
            last_reset: Instant::now() - ESTIMATED_UPDATE_TIME,
        }
    }

    /// Inserts an edge between two nodes in the topology.
    pub fn insert_edge(&mut self, node1: (NodeId, NodeType), node2: (NodeId, NodeType)) {
        let node1_id = node1.0 as usize;
        let node2_id = node2.0 as usize;

        self.graph[node1_id].set(node2_id, true);
        self.graph[node2_id].set(node1_id, true);

        if self.node_id != node1.0 {
            self.types[node1_id] = node1.1;
        }

        if self.node_id != node2.0 {
            self.types[node2_id] = node2.1;
        }
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

    pub fn dijkstra(&self, source: NodeId, dest: NodeId) -> Result<Vec<NodeId>, RoutingError> {
        let source_id = source as usize;
        let dest_id = dest as usize;

        if source == dest {
            return Err(RoutingError::SourceIsDest);
        }

        let mut dist: Vec<f64> = (0..NETWORK_SIZE).map(|_| f64::INFINITY).collect();
        let mut prev: Vec<Option<usize>> = vec![None; NETWORK_SIZE];
        let mut heap = BinaryHeap::new();

        dist[source_id] = 0.0;
        heap.push(State {
            pdr: 0.0,
            position: source_id,
        });

        while let Some(State { pdr, position }) = heap.pop() {
            if position == dest_id {
                let mut path = Vec::new();
                let mut current = Some(dest_id);

                while let Some(node) = current {
                    path.push(node as NodeId);
                    current = prev[node];
                }

                path.reverse();
                return Ok(path);
            }

            if pdr > dist[position] {
                continue;
            }

            // For each node we can reach, see if we can find a way with
            // a lower cost, where the cost is calculated by
            // the pdr of the whole path for reaching the current node
            // + the probability that the packet will reach the next drop
            // but it will drop there
            for neighbor in self.graph[position].iter_ones() {
                // a non-drone node cant be used in the middle of the path
                if neighbor != dest_id && self.types[neighbor] != NodeType::Drone {
                    continue;
                }

                let next_pdr = pdr + (1.0 - pdr) * self.get_observed_pdr(neighbor);
                if next_pdr < dist[neighbor] {
                    dist[neighbor] = next_pdr;
                    prev[neighbor] = Some(position);
                    heap.push(State {
                        pdr: next_pdr,
                        position: neighbor,
                    });
                }
            }
        }

        Err(RoutingError::NoPathFound)
    }

    /// Resets the topology by clearing all edges and resetting node types.
    pub fn reset(&mut self) {
        self.graph = [BitArray::new([0; 32]); NETWORK_SIZE];
        self.types = [NodeType::Drone; NETWORK_SIZE];
        //todo!("UPDATE THE TREND?");

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

    pub fn observe_success(&mut self, node: NodeId) {
        self.observed_trend[node as usize].success += 1.0;
    }

    pub fn observe_failure(&mut self, node: NodeId) {
        self.observed_trend[node as usize].failure += 1.0;
    }

    fn get_observed_pdr(&self, node_id: usize) -> f64 {
        let trend = &self.observed_trend[node_id];
        if trend.success + trend.failure > 0.0 {
            trend.failure / (trend.success + trend.failure)
        } else {
            0.0
        }
    }
}

impl Display for Topology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut res = String::new();

        for (i, node) in self.graph.iter().enumerate() {
            if node.count_ones() > 0 {
                let observed_pdr = self.get_observed_pdr(i);

                res.push_str(&format!(
                    "Node {} [OBSERVED DROP RATE: {}] : ",
                    i, observed_pdr
                ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use wg_2024::packet::NodeType;

    #[test]
    fn test_topology_initialization() {
        let topo = Topology::new(1);
        assert_eq!(topo.graph.len(), NETWORK_SIZE);
        assert_eq!(topo.types.len(), NETWORK_SIZE);
    }

    #[test]
    fn test_insert_and_remove_edge() {
        let mut topo = Topology::new(2);
        topo.insert_edge((0, NodeType::Drone), (1, NodeType::Client));
        assert!(topo.graph[0].get(1).unwrap());
        assert!(topo.graph[1].get(0).unwrap());
        assert_eq!(topo.types[1], NodeType::Client);

        topo.remove_edge(0, 1);
        assert!(!topo.graph[0].get(1).unwrap());
        assert!(!topo.graph[1].get(0).unwrap());
    }

    #[test]
    fn test_bfs_no_path_with_isolated_nodes() {
        let mut topo = Topology::new(2);
        topo.insert_edge((0, NodeType::Drone), (1, NodeType::Client));
        topo.insert_edge((2, NodeType::Server), (3, NodeType::Drone));

        let result = topo.bfs(0, 3);
        assert!(matches!(result, Err(RoutingError::NoPathFound)));
    }

    #[test]
    fn test_observed_pdr_calculation() {
        let mut topo = Topology::new(1);
        topo.observe_success(0);
        topo.observe_failure(0);
        topo.observe_success(1);
        topo.observe_failure(1);

        let pdr_node_0 = topo.get_observed_pdr(0);
        let pdr_node_1 = topo.get_observed_pdr(1);

        assert_eq!(pdr_node_0, 1.0 / 2.0);
        assert_eq!(pdr_node_1, 1.0 / 2.0);
    }

    #[test]
    fn test_valid_path_with_drones() {
        let mut topo = Topology::new(4);
        topo.insert_edge((0, NodeType::Client), (1, NodeType::Drone));
        topo.insert_edge((1, NodeType::Drone), (2, NodeType::Drone));
        topo.insert_edge((2, NodeType::Drone), (3, NodeType::Server));

        let path = topo.dijkstra(0, 3).expect("Path should exist");
        assert_eq!(path, vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_path_with_mixed_nodes() {
        let mut topo = Topology::new(3);
        topo.insert_edge((0, NodeType::Client), (1, NodeType::Drone));
        topo.insert_edge((1, NodeType::Drone), (2, NodeType::Drone));
        topo.insert_edge((2, NodeType::Drone), (3, NodeType::Server));
        topo.insert_edge((3, NodeType::Server), (4, NodeType::Client));

        // Not valid: Server -> Client with no Drone in the middle
        let result = topo.dijkstra(0, 4);
        assert!(matches!(result, Err(RoutingError::NoPathFound)));
    }

    #[test]
    fn test_bfs_valid_path() {
        let mut topo = Topology::new(3);
        topo.insert_edge((0, NodeType::Client), (1, NodeType::Drone));
        topo.insert_edge((1, NodeType::Drone), (2, NodeType::Drone));
        topo.insert_edge((2, NodeType::Drone), (3, NodeType::Server));

        let path = topo.bfs(0, 3).expect("Path should exist");
        assert_eq!(path, vec![0, 1, 2, 3]);
    }
}
