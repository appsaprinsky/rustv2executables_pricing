import subprocess
import json
from pathlib import Path
from typing import List, Dict, Optional
import pandas as pd

class RustPricingExecutor:
    def __init__(self, rust_bin_path: str = "./target/release/vrp_pricing"):
        self.rust_bin_path = Path(rust_bin_path).absolute()
        
        if not self.rust_bin_path.exists():
            self.build_rust_binary()

    def build_rust_binary(self):
        """Compile the Rust binary"""
        subprocess.run(["cargo", "build", "--release"], check=True)

    def find_negative_path(
        self,
        customers: pd.DataFrame,
        warehouses: pd.DataFrame,
        dual_values: Dict[str, float],
        max_stops: int = 5,
        max_capacity: float = 20,
        cost_per_km: float = 10,
        speed_kmh: float = 50,
        service_time: int = 15,
    ) -> Optional[Dict]:
        """Execute Rust binary with JSON input/output"""
        
        # Prepare input data
        input_data = {
            "customers": self._convert_customers(customers),
            "warehouses": self._convert_warehouses(warehouses),
            "dual_values": dual_values,
            "max_stops": max_stops,
            "max_capacity": max_capacity,
            "cost_per_km": cost_per_km,
            "speed_kmh": speed_kmh,
            "service_time": service_time,
        }
        
        # Execute Rust binary
        result = subprocess.run(
            [str(self.rust_bin_path), "solve", "-"],
            input=json.dumps(input_data).encode(),
            capture_output=True,
            check=True,
        )
        
        # Parse output
        output = result.stdout.decode().strip()
        if not output or output == "null":
            return None
            
        return json.loads(output)

    def _convert_customers(self, df: pd.DataFrame) -> List[Dict]:
        return [
            {
                "id": int(row['ID']),
                "lat": row['LAT'],
                "lng": row['LOG'],
                "capacity": row['CAPACITY'],
                "window_start": row['WSTART'].isoformat() + "Z",
                "window_end": row['WEND'].isoformat() + "Z",
            }
            for _, row in df.iterrows()
        ]

    def _convert_warehouses(self, df: pd.DataFrame) -> List[Dict]:
        return [
            {
                "id": int(row['ID']),
                "lat": row['LAT'],
                "lng": row['LOG'],
            }
            for _, row in df.iterrows()
        ]