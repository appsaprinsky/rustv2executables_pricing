use serde::{Deserialize, Serialize};
use chrono::{DateTime, TimeDelta, Utc};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Customer {
    pub id: i64,
    pub lat: f64,
    pub lng: f64,
    pub capacity: f64,
    #[serde(with = "datetime_serde")]
    pub window_start: DateTime<Utc>,
    #[serde(with = "datetime_serde")]
    pub window_end: DateTime<Utc>,
}

mod datetime_serde {
    use chrono::{DateTime, TimeZone, Utc};
    use serde::{self, Deserialize, Deserializer, Serializer};
    
    pub fn serialize<S>(date: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&date.to_rfc3339())
    }
    
    pub fn deserialize<'de, D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Warehouse {
    pub id: i64,
    pub lat: f64,
    pub lng: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputData {
    pub planning_date: String,
    pub customers: Vec<Customer>,
    pub warehouses: Vec<Warehouse>,
    pub dual_values: std::collections::HashMap<String, f64>,
    pub max_stops: usize,
    pub max_capacity: f64,
    pub cost_per_km: f64,
    pub speed_kmh: f64,
    pub service_time: i64,
    pub departure_hour: u32,
    pub allow_violate_time_window: bool, // Add this field
    pub penalties: PenaltyParams,  // Add this
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenaltyParams {
    pub waiting_per_minute: f64,
    pub late_arrival_per_minute: f64,
    pub late_service_per_minute: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathResult {
    pub path: Vec<String>,
    pub reduced_cost: f64,
    pub cost: f64,
    pub capacity: f64,
}

#[derive(Debug, Clone)]
pub struct EdgeData {
    pub cost: f64,
    pub travel_time: TimeDelta,
    pub reduced_cost: f64,
}