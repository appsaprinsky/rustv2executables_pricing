use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::{EdgeRef, IntoEdges, IntoNodeReferences};
use chrono::{DateTime, TimeDelta, Utc};
use std::collections::{HashMap, VecDeque};
use std::f64::consts::PI;
use tempfile::NamedTempFile;
use std::process::Command;
use std::io::Write;
use crate::models::{Customer, Warehouse, EdgeData, PathResult};

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

        // println!("Starting pricing problem");

        for start_wh in self.warehouses.keys() {
            // println!("\nStarting from warehouse: {}", start_wh);
            let start_idx = self.node_indices[start_wh];
            let departure_time = DateTime::parse_from_rfc3339(
            format!("{}T08:00:00+06:00", self.planning_date).as_str()
            ).expect("Invalid planning date format").with_timezone(&Utc);

            let mut labels: HashMap<NodeIndex, Vec<(f64, DateTime<Utc>, f64, Vec<String>)>> = HashMap::new();
            labels.insert(start_idx, vec![(0.0, departure_time, 0.0, vec![start_wh.clone()])]);
            
            let mut queue = VecDeque::new();
            queue.push_back((0.0, departure_time, 0.0, vec![start_wh.clone()]));

            while let Some((current_cost, current_time, current_cap, current_path)) = queue.pop_front() {
                // println!("\nExploring path: {:?} (cost: {})", current_path, current_cost);
                let last_node = current_path.last().unwrap();
                let last_idx = self.node_indices[last_node];

                for edge in self.graph.edges(last_idx) {
                    let next_idx = edge.target();
                    let next_node = &self.graph[next_idx];
                    let edge_data = edge.weight();

                    // println!("  Considering edge to {}", next_node);

                    // STRICT REQUIREMENT: Only allow returning to starting warehouse
                    if next_node.starts_with("W_") && next_node != start_wh {
                        // println!("  SKIP: Not starting warehouse");
                        continue;
                    }

                    // Count customers in current path
                    let customer_count = current_path.iter().filter(|n| n.starts_with("C_")).count();

                    // For customers: check max_stops and no duplicates
                    if next_node.starts_with("C_") {
                        if customer_count >= self.max_stops {
                            // println!("  SKIP: Max stops reached ({}/{})", customer_count, self.max_stops);
                            continue;
                        }
                        if current_path.contains(next_node) {
                            // println!("  SKIP: Already visited customer");
                            continue;
                        }
                    }

                    // Calculate new time and capacity
                    let mut arrival_time = current_time + edge_data.travel_time;

                    let new_cap = if next_node.starts_with("C_") {
                        // ARRIVING AT CUSTOMER
                        // println!("  ARRIVAL at customer: {}", arrival_time);
                        
                        // Check time window BEFORE adding service time
                        let cust = &self.customers[next_node];
                        if arrival_time < cust.window_start {
                            arrival_time = cust.window_start;
                            // println!("  ADJUSTED to window start: {}", arrival_time);
                        }
                        if arrival_time > cust.window_end {
                            // println!("  SKIP: Arrival {} after window end {}", arrival_time, cust.window_end);
                            continue;
                        }

                        // ADD SERVICE TIME
                        let service_end = arrival_time + TimeDelta::minutes(self.service_time);
                        // println!("  SERVICE from {} to {}", arrival_time, service_end);
                        
                        // Check if service completes before window closes
                        if service_end > cust.window_end {
                            // println!("  SKIP: Service ends after window ({} > {})", 
                                // service_end, cust.window_end);
                            continue;
                        }

                        // Update time to reflect service completion
                        arrival_time = service_end;

                        // Check capacity
                        let new_cap = current_cap + cust.capacity;
                        if new_cap > self.max_capacity {
                            // println!("  SKIP: Exceeds capacity ({} > {})", new_cap, self.max_capacity);
                            continue;
                        }
                        new_cap
                    } else {
                        // Moving between warehouses or from warehouse to customer
                        current_cap
                    };

                    // Calculate new cost and path
                    let new_cost = current_cost + edge_data.reduced_cost;
                    let mut new_path = current_path.clone();
                    new_path.push(next_node.clone());

                    // println!("  New path candidate: {:?}", new_path);
                    // println!("  New reduced cost: {}", new_cost);

                    // Complete path must return to start warehouse with at least 1 customer
                    if next_node == start_wh && customer_count >= 1 {
                        // println!("  COMPLETE PATH FOUND");
                        if new_cost < best_reduced_cost {
                            // println!("  NEW BEST PATH! Cost: {}", new_cost);
                            match self.calculate_with_executable(&new_path, departure_time) {
                                Ok(total_cost) => {
                                    best_reduced_cost = new_cost;
                                    best_path = Some(PathResult {
                                        path: new_path.clone(),
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
                        // println!("  ADDING TO QUEUE");
                        labels.entry(next_idx)
                            .or_default()
                            .push((new_cost, arrival_time, new_cap, new_path.clone()));
                        queue.push_back((new_cost, arrival_time, new_cap, new_path));
                    } else {
                        // println!("  SKIP: Dominated by existing path");
                    }
                }
            }
        }

        // println!("\nFinal best path: {:?}", best_path);
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