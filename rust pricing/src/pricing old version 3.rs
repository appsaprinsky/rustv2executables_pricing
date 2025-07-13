use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{EdgeRef, IntoEdges, IntoNodeReferences};
use chrono::{DateTime, TimeDelta, Utc};
use std::collections::{HashMap, VecDeque};
use std::f64::consts::PI;
use tempfile::NamedTempFile;
use std::process::Command;
use std::io::Write;
use crate::models::{Customer, Warehouse, EdgeData, PathResult};
use std::collections::BinaryHeap;
use std::cmp::Ordering;

const EARTH_RADIUS_KM: f64 = 6371.0;
const MAX_EDGE_DISTANCE_KM: f64 = 1200.0;


#[derive(Debug)]
struct QueueItem {
    cost: f64,
    time: DateTime<Utc>,
    capacity: f64,
    path: Vec<String>,
    last_node: NodeIndex,
    last_direction: Option<f64>,
}

impl PartialEq for QueueItem {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}

impl Eq for QueueItem {}

impl PartialOrd for QueueItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        other.cost.partial_cmp(&self.cost)
    }
}

impl Ord for QueueItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}


pub struct PricingProblem {
    graph: DiGraph<String, EdgeData>,
    node_indices: HashMap<String, NodeIndex>,
    customers: HashMap<String, Customer>,
    warehouses: HashMap<String, Warehouse>,
    max_stops: usize,
    max_capacity: f64,
    cost_per_km: f64,
    speed_kmh: f64,
    service_time: i64,
    planning_date: String, 
}

impl PricingProblem {
    pub fn new(
        customers: Vec<Customer>,
        warehouses: Vec<Warehouse>,
        dual_values: &HashMap<String, f64>,
        max_stops: usize,
        max_capacity: f64,
        cost_per_km: f64,
        speed_kmh: f64,
        service_time: i64,
        planning_date: String, 
    ) -> Self {
        let mut graph = DiGraph::new();
        let mut node_indices = HashMap::new();
        let mut customer_map = HashMap::new();
        let mut warehouse_map = HashMap::new();

        // Add warehouses
        for wh in warehouses {
            let node_id = format!("W_{}", wh.id);
            let idx = graph.add_node(node_id.clone());
            node_indices.insert(node_id.clone(), idx);
            warehouse_map.insert(node_id, wh);
        }

        // Add customers
        for cust in customers {
            let node_id = format!("C_{}", cust.id);
            let idx = graph.add_node(node_id.clone());
            node_indices.insert(node_id.clone(), idx);
            customer_map.insert(node_id, cust);
        }

        // Build edges
        let mut pricing = Self {
            graph,
            node_indices,
            customers: customer_map,
            warehouses: warehouse_map,
            max_stops,
            max_capacity,
            cost_per_km,
            speed_kmh,
            service_time,
            planning_date,
        };

        pricing.build_edges(dual_values);
        pricing
    }

    fn calculate_with_executable(&self, path: &[String], departure: DateTime<Utc>) -> Result<f64, String> {
            // Prepare input
            let input = serde_json::json!({
                "locations": self.all_locations(),
                "path": path,
                "departure": departure.to_rfc3339(),
                "cost_per_km": self.cost_per_km,
                "speed_kmh": self.speed_kmh,
                "service_minutes": self.service_time,
                "max_capacity": self.max_capacity,
                "max_stops": self.max_stops
            });
            println!("Input for calculator: {}", input);

            // Create temp file
            let mut file = NamedTempFile::new().map_err(|e| e.to_string())?;
            file.write_all(input.to_string().as_bytes()).map_err(|e| e.to_string())?;

            // Execute with proper --input flag
            let output = Command::new("./rust_trip_calculator")
                .arg("--input")
                .arg(file.path())
                .output()
                .map_err(|e| e.to_string())?;

            if !output.status.success() {
                return Err(format!(
                    "Calculator error ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ));
            }

            // Parse output
            let result: serde_json::Value = serde_json::from_slice(&output.stdout)
                .map_err(|e| e.to_string())?;

            result["total_cost"].as_f64()
                .ok_or("Missing total_cost in calculator output".to_string())
        }

    fn all_locations(&self) -> Vec<serde_json::Value> {
        let mut locations = Vec::new();
        
        // Add warehouses
        for (id, wh) in &self.warehouses {
            locations.push(serde_json::json!({
                "id": id,
                "lat": wh.lat,
                "lng": wh.lng,
            }));
        }
        
        // Add customers
        for (id, cust) in &self.customers {
            locations.push(serde_json::json!({
                "id": id,
                "lat": cust.lat,
                "lng": cust.lng,
                "window_start": cust.window_start,
                "window_end": cust.window_end,
                "capacity": cust.capacity,
            }));
        }
        
        locations
    }

    fn haversine_distance(&self, p1: (f64, f64), p2: (f64, f64)) -> f64 {
        let (lat1, lon1) = (p1.0.to_radians(), p1.1.to_radians());
        let (lat2, lon2) = (p2.0.to_radians(), p2.1.to_radians());
        
        let dlat = lat2 - lat1;
        let dlon = lon2 - lon1;
        
        let a = (dlat/2.0).sin().powi(2) + 
                lat1.cos() * lat2.cos() * (dlon/2.0).sin().powi(2);
        let c = 2.0 * a.sqrt().atan2((1.0-a).sqrt());
        
        EARTH_RADIUS_KM * c
    }

    // Modified build_edges with geographic constraints
    fn build_edges(&mut self, dual_values: &HashMap<String, f64>) {
        let warehouse_nodes: Vec<_> = self.warehouses.keys().cloned().collect();
        let customer_nodes: Vec<_> = self.customers.keys().cloned().collect();

        // Warehouse to customer edges
        for wh_node in &warehouse_nodes {
            let wh_coords = self.get_coords(wh_node);
            for cust_node in &customer_nodes {
                let cust_coords = self.get_coords(cust_node);
                let distance = self.haversine_distance(wh_coords, cust_coords);
                if distance <= MAX_EDGE_DISTANCE_KM {
                    self.add_edge(wh_node, cust_node, dual_values);
                }
            }
        }

        // Customer to warehouse edges
        for cust_node in &customer_nodes {
            let cust_coords = self.get_coords(cust_node);
            for wh_node in &warehouse_nodes {
                let wh_coords = self.get_coords(wh_node);
                let distance = self.haversine_distance(cust_coords, wh_coords);
                if distance <= MAX_EDGE_DISTANCE_KM {
                    self.add_edge(cust_node, wh_node, dual_values);
                }
            }
        }

        // Customer to customer edges with direction constraints
        for (i, cust1) in customer_nodes.iter().enumerate() {
            let coords1 = self.get_coords(cust1);
            for cust2 in customer_nodes.iter().skip(i + 1) {
                let coords2 = self.get_coords(cust2);
                let distance = self.haversine_distance(coords1, coords2);
                if distance <= MAX_EDGE_DISTANCE_KM {
                    // Only add edges that make geographic sense
                    self.add_edge(cust1, cust2, dual_values);
                    self.add_edge(cust2, cust1, dual_values);
                }
            }
        }
    }

    // Helper function to calculate bearing between two points
    fn calculate_bearing(&self, p1: (f64, f64), p2: (f64, f64)) -> f64 {
        let (lat1, lon1) = (p1.0.to_radians(), p1.1.to_radians());
        let (lat2, lon2) = (p2.0.to_radians(), p2.1.to_radians());
        
        let y = (lon2 - lon1).sin() * lat2.cos();
        let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * (lon2 - lon1).cos();
        y.atan2(x).to_degrees()
    }

    // Helper function to calculate angle difference (0-180 degrees)
    fn angle_difference(&self, a1: f64, a2: f64) -> f64 {
        let diff = (a2 - a1).abs() % 360.0;
        diff.min(360.0 - diff)
    }

    fn add_edge(&mut self, u: &str, v: &str, dual_values: &HashMap<String, f64>) {
        let coords_u = self.get_coords(u);
        let coords_v = self.get_coords(v);
        
        let distance_km = self.haversine_distance(coords_u, coords_v);
        let cost = self.cost_per_km * distance_km;
        let travel_time = TimeDelta::minutes((60.0 * distance_km / self.speed_kmh) as i64);
        
        let reduced_cost = if v.starts_with("C_") {
            let cust_id = v.split('_').nth(1).unwrap(); // Extract "20" from "C_20"
            cost - dual_values.get(cust_id).unwrap_or(&0.0)
        } else {
            cost
        };

        let u_idx = self.node_indices[u];
        let v_idx = self.node_indices[v];
        
        self.graph.add_edge(
            u_idx,
            v_idx,
            EdgeData {
                cost,
                travel_time,
                reduced_cost,
            },
        );
    }

    fn get_coords(&self, node: &str) -> (f64, f64) {
        if node.starts_with("W_") {
            let wh = &self.warehouses[node];
            (wh.lat, wh.lng)
        } else {
            let cust = &self.customers[node];
            (cust.lat, cust.lng)
        }
    }

    // Modified find_negative_path with geographic awareness
    pub fn find_negative_path(&self) -> Option<PathResult> {
        let mut best_path = None;
        let mut best_reduced_cost = 0.0;
        let mut queue = BinaryHeap::new();

        for start_wh in self.warehouses.keys() {
            let start_idx = self.node_indices[start_wh];
            let departure_time = DateTime::parse_from_rfc3339(
                format!("{}T08:00:00+06:00", self.planning_date).as_str()
            ).expect("Invalid planning date format").with_timezone(&Utc);

            queue.push(QueueItem {
                cost: 0.0,
                time: departure_time,
                capacity: 0.0,
                path: vec![start_wh.clone()],
                last_node: start_idx,
                last_direction: None,
            });

            while let Some(current) = queue.pop() {
                for edge in self.graph.edges(current.last_node) {
                    let next_idx = edge.target();
                    let next_node = &self.graph[next_idx];
                    let edge_data = edge.weight();

                    // STRICT REQUIREMENT: Only allow returning to starting warehouse
                    if next_node.starts_with("W_") && next_node != start_wh {
                        continue;
                    }

                    // Count customers in current path
                    let customer_count = current.path.iter().filter(|n| n.starts_with("C_")).count();

                    // For customers: check max_stops and no duplicates
                    if next_node.starts_with("C_") {
                        if customer_count >= self.max_stops {
                            continue;
                        }
                        if current.path.contains(next_node) {
                            continue;
                        }
                    }

                    // Calculate new time and capacity
                    let mut arrival_time = current.time + edge_data.travel_time;
                    let new_cap = if next_node.starts_with("C_") {
                        let cust = &self.customers[next_node];
                        
                        // Check time window BEFORE adding service time
                        if arrival_time < cust.window_start {
                            arrival_time = cust.window_start;
                        }
                        if arrival_time > cust.window_end {
                            continue;
                        }

                        // ADD SERVICE TIME
                        let service_end = arrival_time + TimeDelta::minutes(self.service_time);
                        if service_end > cust.window_end {
                            continue;
                        }

                        // Update time to reflect service completion
                        arrival_time = service_end;

                        // Check capacity
                        let new_cap = current.capacity + cust.capacity;
                        if new_cap > self.max_capacity {
                            continue;
                        }
                        new_cap
                    } else {
                        current.capacity
                    };

                    // Calculate new direction (for geographic coherence)
                    let new_direction = if current.path.len() >= 5 {
                        let prev_node = &current.path[current.path.len() - 2];
                        let curr_node = &current.path[current.path.len() - 1];
                        let prev_coords = self.get_coords(prev_node);
                        let curr_coords = self.get_coords(curr_node);
                        let next_coords = self.get_coords(next_node);
                        
                        let dir1 = self.calculate_bearing(prev_coords, curr_coords);
                        let dir2 = self.calculate_bearing(curr_coords, next_coords);
                        let angle_diff = self.angle_difference(dir1, dir2);
                        
                        // Skip if the turn is too sharp (> 90 degrees)
                        // let distance = self.haversine_distance(curr_coords, next_coords);
                        if angle_diff > 90.0 {
                            continue;
                        }
                        Some(dir2)
                    } else {
                        None
                    };

                    // Calculate new cost and path
                    let new_cost = current.cost + edge_data.reduced_cost;
                    let mut new_path = current.path.clone();
                    new_path.push(next_node.clone());

                    // Complete path must return to start warehouse with at least 1 customer
                    if next_node == start_wh && customer_count >= 1 {
                        if new_cost < best_reduced_cost {
                            match self.calculate_with_executable(&new_path, departure_time) {
                                Ok(total_cost) => {
                                    best_reduced_cost = new_cost;
                                    best_path = Some(PathResult {
                                        path: new_path,
                                        reduced_cost: new_cost,
                                        cost: total_cost,
                                        capacity: new_cap,
                                    });
                                },
                                Err(e) => eprintln!("Calculator error: {}", e),
                            }
                        }
                        continue;
                    }

                    // Add to queue if feasible
                    queue.push(QueueItem {
                        cost: new_cost,
                        time: arrival_time,
                        capacity: new_cap,
                        path: new_path,
                        last_node: next_idx,
                        last_direction: new_direction,
                    });
                }
            }
        }

        best_path
    }

    fn is_dominated(
        &self,
        node: NodeIndex,
        cost: f64,
        time: DateTime<Utc>,
        capacity: f64,
        labels: &HashMap<NodeIndex, Vec<(f64, DateTime<Utc>, f64, Vec<String>)>>,
    ) -> bool {
        labels.get(&node).map_or(false, |existing_labels| {
            existing_labels.iter().any(|(ec, et, ecap, _)| {
                *ec <= cost && *et <= time && *ecap <= capacity
            })
        })
    }

    fn calculate_path_cost(&self, path: &[String]) -> f64 {
        path.windows(2)
            .map(|pair| {
                let u = &pair[0];
                let v = &pair[1];
                let u_idx = self.node_indices[u];
                let v_idx = self.node_indices[v];
                self.graph.edges_connecting(u_idx, v_idx)
                    .next()
                    .map(|e| e.weight().cost)
                    .unwrap_or(0.0)
            })
            .sum()
    }
}