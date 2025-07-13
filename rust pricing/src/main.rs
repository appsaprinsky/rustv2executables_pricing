mod models;
mod pricing;

use std::io;
use clap::{Parser, Subcommand};
use serde_json::{from_str, to_string};
use crate::models::{InputData, PathResult};
use crate::pricing::PricingProblem;

#[derive(Parser)]
#[command(name = "VRP Pricing Solver")]
#[command(version = "1.0")]
#[command(about = "Solves VRP pricing problem as standalone executable")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Solve pricing problem from JSON input
    Solve {
        /// Input JSON file or '-' for stdin
        input: String,
        
        /// Output JSON file or '-' for stdout
        #[arg(short, long)]
        output: Option<String>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Solve { input, output } => {
            // Read input
            let input_str = if input == "-" {
                let mut buffer = String::new();
                io::stdin().read_line(&mut buffer)?;
                buffer
            } else {
                std::fs::read_to_string(input)?
            };

            let input_data: InputData = from_str(&input_str)?;

            // Solve problem
            let pricing = PricingProblem::new(
                input_data.customers,
                input_data.warehouses,
                &input_data.dual_values,
                input_data.max_stops,
                input_data.max_capacity,
                input_data.cost_per_km,
                input_data.speed_kmh,
                input_data.service_time,
                input_data.planning_date, 
                input_data.departure_hour, 
                input_data.allow_violate_time_window, 
                input_data.penalties,
            );

            let result = pricing.find_negative_path();

            // Write output
            let output_str = to_string(&result)?;
            
            if let Some(output_path) = output {
                if output_path == "-" {
                    println!("{}", output_str);
                } else {
                    std::fs::write(output_path, output_str)?;
                }
            } else {
                println!("{}", output_str);
            }
        }
    }

    Ok(())
}