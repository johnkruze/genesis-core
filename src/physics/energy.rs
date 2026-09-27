//! Energy Grid Physics
//! Simulates nuclear baseload, intermittent solar/wind, demand response, and grid inertia.

pub struct GridSystem {
    pub base_load: f64, // MW
    pub current_demand: f64, // MW
    pub grid_frequency: f64, // Hz (nominal 50/60)
    pub inertia_constant: f64, // MW*s/MVA
    
    pub nuclear_capacity: f64, // MW
    pub solar_capacity: f64, // MW
    pub wind_capacity: f64, // MW
    pub hydro_capacity: f64, // MW
    
    // States
    pub nuclear_output: f64,
    pub solar_output: f64,
    pub wind_output: f64,
    pub hydro_output: f64,
}

impl Default for GridSystem {
    fn default() -> Self {
        Self {
            base_load: 1000.0,
            current_demand: 1200.0,
            grid_frequency: 60.0,
            inertia_constant: 5.0, // Typical for thermal plants
            nuclear_capacity: 800.0,
            solar_capacity: 400.0,
            wind_capacity: 300.0,
            hydro_capacity: 200.0,
            
            nuclear_output: 800.0,
            solar_output: 0.0,
            wind_output: 0.0,
            hydro_output: 100.0,
        }
    }
}

impl GridSystem {
    pub fn step(&mut self, dt_seconds: f64, solar_irradiance: f64, wind_speed: f64, demand_fluctuation: f64) {
        // Intermittent updates
        self.solar_output = (self.solar_capacity * solar_irradiance).clamp(0.0, self.solar_capacity);
        
        // Wind power curve (simplified)
        let cut_in = 3.0; // m/s
        let rated = 12.0; // m/s
        let cut_out = 25.0; // m/s
        
        self.wind_output = if wind_speed < cut_in || wind_speed > cut_out {
            0.0
        } else if wind_speed < rated {
            self.wind_capacity * ((wind_speed - cut_in) / (rated - cut_in)).powi(3)
        } else {
            self.wind_capacity
        };
        
        // Demand fluctuation
        self.current_demand = (self.base_load + demand_fluctuation).max(100.0);
        
        // Dispatch hydro to balance (it has fast ramp rates)
        let total_generation_before_hydro = self.nuclear_output + self.solar_output + self.wind_output;
        let shortfall = self.current_demand - total_generation_before_hydro;
        
        // Try to meet shortfall with hydro
        self.hydro_output = shortfall.clamp(0.0, self.hydro_capacity);
        
        let total_generation = total_generation_before_hydro + self.hydro_output;
        let power_delta = total_generation - self.current_demand;
        
        // Swing equation: Rate of change of frequency
        // df/dt = (Pg - Pd) * f0 / (2 * H * Sbase)
        // Simplified: Sbase ~ total capacity
        let system_capacity = self.nuclear_capacity + self.solar_capacity + self.wind_capacity + self.hydro_capacity;
        let df_dt = (power_delta * 60.0) / (2.0 * self.inertia_constant * system_capacity);
        
        self.grid_frequency += df_dt * dt_seconds;
        self.grid_frequency = self.grid_frequency.clamp(58.0, 62.0); // Extreme bounds
    }

    pub fn is_blackout(&self) -> bool {
        self.grid_frequency < 59.3 || self.grid_frequency > 60.7
    }
}
