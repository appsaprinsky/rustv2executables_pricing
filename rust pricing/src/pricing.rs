use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{EdgeRef, IntoEdges, IntoNodeReferences};
use chrono::{DateTime, TimeDelta, Utc};
use std::collections::{HashMap, VecDeque};
use std::f64::consts::PI;
use tempfile::NamedTempFile;
use std::process::Command;
use std::io::Write;
use crate::models::{Customer, Warehouse, EdgeData, PathResult};
use crate::models::PenaltyParams;

const EARTH_RADIUS_KM: f64 = 6371.0;


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
    departure_hour: u32, 
    allow_violate_time_window: bool,
    penalties: PenaltyParams,  // Add this
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
        departure_hour: u32, 
        allow_violate_time_window: bool,
        penalties: PenaltyParams, 
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
            departure_hour,  
            allow_violate_time_window,
            penalties
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
                "max_stops": self.max_stops,
                "allow_violate_time_window": false, // Default to false for safety
                "penalties": self.penalties  // Pass through penalties
            });
            println!("Input for calculator: {}", input);

            // Create temp file
            let mut file = NamedTempFile::new().map_err(|e| e.to_string())?;
            file.write_all(input.to_string().as_bytes()).map_err(|e| e.to_string())?;

            // Execute with proper --input flag
            let output = Command::new("logisticoptimizerv2apppairingsbackend/pairing_api/optimization/rust_trip_calculator")
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

    fn build_edges(&mut self, dual_values: &HashMap<String, f64>) {
        // Collect keys first to avoid borrowing issues
        let warehouse_nodes: Vec<String> = self.warehouses.keys().cloned().collect();
        let customer_nodes: Vec<String> = self.customers.keys().cloned().collect();

        // Add warehouse<->customer edges
        for wh_node in &warehouse_nodes {
            for cust_node in &customer_nodes {
                self.add_edge(wh_node, cust_node, dual_values);
                self.add_edge(cust_node, wh_node, dual_values);
            }
        }

        // Add customer->customer edges
        for cust1 in &customer_nodes {
            for cust2 in &customer_nodes {
                if cust1 != cust2 {
                    self.add_edge(cust1, cust2, dual_values);
                }
            }
        }
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

    pub fn find_negative_path(&self) -> Option<PathResult> {
        let mut best_path = None;
        let mut best_reduced_cost = 0.0;

        for start_wh in self.warehouses.keys() {
            let start_idx = self.node_indices[start_wh];
            let departure_time = DateTime::parse_from_rfc3339(
            format!("{}T{:02}:00:00+06:00", self.planning_date, self.departure_hour).as_str()
            ).expect("Invalid planning date format").with_timezone(&Utc);

            let mut labels: HashMap<NodeIndex, Vec<(f64, DateTime<Utc>, f64, Vec<String>)>> = HashMap::new();
            labels.insert(start_idx, vec![(0.0, departure_time, 0.0, vec![start_wh.clone()])]);
            
            let mut queue = VecDeque::new();
            queue.push_back((0.0, departure_time, 0.0, vec![start_wh.clone()]));

            while let Some((current_cost, current_time, current_cap, current_path)) = queue.pop_front() {
                let last_node = current_path.last().unwrap();
                let last_idx = self.node_indices[last_node];

                for edge in self.graph.edges(last_idx) {
                    let next_idx = edge.target();
                    let next_node = &self.graph[next_idx];
                    let edge_data = edge.weight();

                    // STRICT REQUIREMENT: Only allow returning to starting warehouse
                    if next_node.starts_with("W_") && next_node != start_wh {
                        continue;
                    }

                    // Count customers in current path
                    let customer_count = current_path.iter().filter(|n| n.starts_with("C_")).count();

                    // For customers: check max_stops and no duplicates
                    if next_node.starts_with("C_") {
                        if customer_count >= self.max_stops {
                            continue;
                        }
                        if current_path.contains(next_node) {
                            continue;
                        }
                    }

                    // Calculate new time and capacity
                    let mut arrival_time = current_time + edge_data.travel_time;
                    let new_cap = if next_node.starts_with("C_") {
                        let cust = &self.customers[next_node];
                        arrival_time = arrival_time.max(cust.window_start);
                        if arrival_time > cust.window_end {
                            continue;
                        }
                        
                        let service_end = arrival_time + TimeDelta::minutes(self.service_time);
                        if service_end > cust.window_end {
                            continue;
                        }

                        let new_cap = current_cap + cust.capacity;
                        if new_cap > self.max_capacity {
                            continue;
                        }
                        
                        arrival_time = service_end;
                        new_cap
                    } else {
                        current_cap
                    };

                    // Calculate new cost and path
                    let new_cost = current_cost + edge_data.reduced_cost;
                    let mut new_path = current_path.clone();
                    new_path.push(next_node.clone());

                    // Complete path must return to start warehouse with at least 1 customer
                    if next_node == start_wh && customer_count >= 1 {
                        if new_cost < best_reduced_cost {
                            // Found candidate path - now optimize its ordering
                            let optimized_path = if self.allow_violate_time_window {
                                self.optimize_path_order(&new_path)
                            } else {
                                new_path.clone() // Skip optimization if we can't violate windows
                            };
                            
                            // Calculate exact cost for optimized path
                            match self.calculate_with_executable(&optimized_path, departure_time) {
                                Ok(total_cost) => {
                                    best_reduced_cost = new_cost;
                                    best_path = Some(PathResult {
                                        path: optimized_path,
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

                    // Continue exploring if not dominated
                    if !self.is_dominated(next_idx, new_cost, arrival_time, new_cap, &labels) {
                        labels.entry(next_idx)
                            .or_default()
                            .push((new_cost, arrival_time, new_cap, new_path.clone()));
                        queue.push_back((new_cost, arrival_time, new_cap, new_path));
                    }
                }
            }
        }

        best_path
    }

    fn optimize_path_order(&self, path: &[String]) -> Vec<String> {
        if path.len() <= 2 {
            return path.to_vec();
        }

        // Extract just the customer nodes (excluding start/end warehouses)
        let mut customers: Vec<_> = path[1..path.len()-1].to_vec();
        let start_wh = path[0].clone();
        let end_wh = path.last().unwrap().clone();

        // Use 2-opt algorithm for better path optimization
        let mut best_path = customers.clone();
        let mut improved = true;
        
        while improved {
            improved = false;
            for i in 0..best_path.len() - 1 {
                for j in i + 1..best_path.len() {
                    let mut new_path = best_path.clone();
                    new_path[i..=j].reverse(); // Perform 2-opt swap
                    
                    // Calculate total distance for both paths
                    let current_dist = self.calculate_path_distance(&start_wh, &best_path, &end_wh);
                    let new_dist = self.calculate_path_distance(&start_wh, &new_path, &end_wh);
                    
                    if new_dist < current_dist {
                        best_path = new_path;
                        improved = true;
                    }
                }
            }
        }

        // Reconstruct full path
        let mut optimized = vec![start_wh];
        optimized.extend(best_path);
        optimized.push(end_wh);
        optimized
    }

    fn calculate_path_distance(&self, start: &str, customers: &[String], end: &str) -> f64 {
        let mut distance = 0.0;
        let mut prev_node = start;
        
        for node in customers {
            distance += self.get_edge_distance(prev_node, node);
            prev_node = node;
        }
        
        distance += self.get_edge_distance(prev_node, end);
        distance
    }

    fn get_edge_distance(&self, u: &str, v: &str) -> f64 {
        let u_idx = self.node_indices[u];
        let v_idx = self.node_indices[v];
        self.graph.edges_connecting(u_idx, v_idx)
            .next()
            .map(|e| e.weight().cost / self.cost_per_km) // Convert cost back to distance
            .unwrap_or(f64::INFINITY)
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