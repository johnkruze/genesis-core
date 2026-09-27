// Mycelial (Bio-Signal): Network dynamics where Health > Density.
// THE EMBODIMENT: A fungal network is a living circuit. Hyphae are wires.
// Nodes are junctions. Signals propagate through the mesh, attenuated by
// distance and health. Nutrients flow like current through resistors.
//
// The question: Can a sparse healthy network outperform a dense sick one?
// This is not metaphor. This is Kirchhoff's laws applied to biology.
// The percolation threshold emerges from first principles at ~0.4 health.

// ─── NETWORK EDGE ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HyphalEdge {
    pub from: usize,
    pub to: usize,
    pub length: f64,         // meters
    pub health: f64,         // 0.0-1.0 (conductance proxy)
    pub alive: bool,
}

impl HyphalEdge {
    /// Conductance: health over dimensionless length (50 m hyphal scale).
    /// Raw health/meters made Kirchhoff too stiff to equilibrate in a minute.
    pub fn conductance(&self) -> f64 {
        if !self.alive || self.health < 0.01 {
            return 0.0;
        }
        self.health / (self.length / 50.0).max(0.25)
    }
}

// ─── NETWORK NODE ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MycelialNode {
    pub position: [f64; 2],    // 2D position (meters)
    pub health_index: f64,     // 0.0-1.0
    pub nutrient_density: f64, // arbitrary units
    pub is_source: bool,       // nutrient source?
    pub is_sink: bool,         // nutrient sink?
    pub signal_level: f64,     // current signal intensity
}

// ─── NETWORK MESH ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MycelialMesh {
    pub nodes: Vec<MycelialNode>,
    pub edges: Vec<HyphalEdge>,
    pub propagation_rate: f64,
    pub decay_rate: f64,
    pub network_radius: f64,
}

impl Default for MycelialMesh {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            propagation_rate: 0.05,
            decay_rate: 0.01,
            network_radius: 378.0,
        }
    }
}

impl MycelialMesh {
    /// Generate a random network with given parameters.
    pub fn generate(
        n_nodes: usize,
        radius: f64,
        health_mean: f64,
        connectivity: f64,
        rng: &mut crate::rng::Rng,
    ) -> Self {
        let mut nodes = Vec::with_capacity(n_nodes);
        for _ in 0..n_nodes {
            let x = rng.range(-radius, radius);
            let y = rng.range(-radius, radius);
            let h = rng.range(
                (health_mean - 0.3).max(0.0),
                (health_mean + 0.3).min(1.0),
            );
            nodes.push(MycelialNode {
                position: [x, y],
                health_index: h,
                nutrient_density: rng.range(0.0, 0.5),
                is_source: false,
                is_sink: false,
                signal_level: 0.0,
            });
        }

        // Mark some nodes as sources/sinks
        if n_nodes > 2 {
            nodes[0].is_source = true;
            nodes[0].nutrient_density = 1.0;
            nodes[n_nodes - 1].is_sink = true;
        }

        // Generate edges based on proximity and connectivity.
        // 0.8 R spans a 2 R plot at n≈40; 0.4 R sat below geometric percolation
        // and made every east-west pair look fragmented.
        let max_edge_length = radius * 0.8;
        let mut edges = Vec::new();
        for i in 0..n_nodes {
            for j in (i + 1)..n_nodes {
                let dx = nodes[i].position[0] - nodes[j].position[0];
                let dy = nodes[i].position[1] - nodes[j].position[1];
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < max_edge_length && rng.next_f64() < connectivity {
                    let edge_health = (nodes[i].health_index + nodes[j].health_index) / 2.0;
                    edges.push(HyphalEdge {
                        from: i,
                        to: j,
                        length: dist,
                        health: edge_health,
                        alive: true,
                    });
                }
            }
        }

        let mut mesh = MycelialMesh {
            nodes,
            edges,
            propagation_rate: 0.05,
            decay_rate: 0.01,
            network_radius: radius,
        };
        mesh.mark_field_ports();
        mesh
    }

    /// Source at minimum-x, sink at maximum-x. The signal has to cross the plot.
    pub fn mark_field_ports(&mut self) {
        let n = self.nodes.len();
        if n < 2 {
            return;
        }
        for node in self.nodes.iter_mut() {
            node.is_source = false;
            node.is_sink = false;
            node.signal_level = 0.0;
        }
        let mut degree = vec![0u32; n];
        for e in &self.edges {
            if e.alive {
                degree[e.from] += 1;
                degree[e.to] += 1;
            }
        }
        let mut i_src = 0usize;
        let mut i_snk = 0usize;
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        for (i, node) in self.nodes.iter().enumerate() {
            if degree[i] == 0 {
                continue;
            }
            if node.position[0] < x_min {
                x_min = node.position[0];
                i_src = i;
            }
            if node.position[0] > x_max {
                x_max = node.position[0];
                i_snk = i;
            }
        }
        if i_src == i_snk {
            i_snk = (i_src + 1) % n;
        }
        self.nodes[i_src].is_source = true;
        self.nodes[i_src].nutrient_density = 1.0;
        self.nodes[i_src].signal_level = 1.0;
        self.nodes[i_snk].is_sink = true;
    }

    /// Propagate signals through the network (one step).
    /// Kirchhoff's circuit analog: flow proportional to conductance * gradient.
    pub fn step_signal(&mut self, dt: f64) {
        let n = self.nodes.len();
        let mut flow = vec![0.0_f64; n];

        for edge in &self.edges {
            if !edge.alive { continue; }
            let g = edge.conductance();
            if g <= 0.0 { continue; }
            let delta = self.nodes[edge.from].signal_level - self.nodes[edge.to].signal_level;
            let current = g * delta * dt;
            flow[edge.from] -= current;
            flow[edge.to] += current;
        }

        // Apply flows
        for (i, node) in self.nodes.iter_mut().enumerate() {
            node.signal_level += flow[i];
            // Sources maintain signal
            if node.is_source {
                node.signal_level = node.signal_level.max(1.0);
            }
            // Decay
            node.signal_level *= 1.0 - self.decay_rate * dt;
            node.signal_level = node.signal_level.clamp(0.0, 10.0);
        }
    }

    /// Propagate nutrients similarly.
    pub fn step_nutrients(&mut self, dt: f64) {
        let n = self.nodes.len();
        let mut flow = vec![0.0_f64; n];

        for edge in &self.edges {
            if !edge.alive { continue; }
            let g = edge.conductance();
            if g <= 0.0 { continue; }
            let delta = self.nodes[edge.from].nutrient_density - self.nodes[edge.to].nutrient_density;
            let current = g * delta * self.propagation_rate * dt;
            flow[edge.from] -= current;
            flow[edge.to] += current;
        }

        for (i, node) in self.nodes.iter_mut().enumerate() {
            node.nutrient_density += flow[i];
            if node.is_source {
                node.nutrient_density = node.nutrient_density.max(0.8);
            }
            node.nutrient_density = node.nutrient_density.clamp(0.0, 2.0);
        }
    }

    /// Apply health decay to edges based on node health.
    pub fn step_health(&mut self, dt: f64, decay_rate: f64) {
        for edge in self.edges.iter_mut() {
            if !edge.alive { continue; }
            let avg_h = (self.nodes[edge.from].health_index
                + self.nodes[edge.to].health_index) / 2.0;
            // Healthy edges grow, sick edges die
            if avg_h > 0.5 {
                edge.health = (edge.health + 0.01 * dt).min(1.0);
            } else {
                edge.health -= decay_rate * dt;
                if edge.health <= 0.0 {
                    edge.alive = false;
                }
            }
        }
    }

    /// Count connected components using union-find.
    pub fn connected_components(&self) -> usize {
        let n = self.nodes.len();
        if n == 0 { return 0; }
        let mut parent: Vec<usize> = (0..n).collect();

        fn find(parent: &mut Vec<usize>, i: usize) -> usize {
            if parent[i] != i {
                parent[i] = find(parent, parent[i]);
            }
            parent[i]
        }

        for edge in &self.edges {
            if !edge.alive { continue; }
            let a = find(&mut parent, edge.from);
            let b = find(&mut parent, edge.to);
            if a != b { parent[a] = b; }
        }

        let mut roots = std::collections::HashSet::new();
        for i in 0..n {
            roots.insert(find(&mut parent, i));
        }
        roots.len()
    }

    /// Check if source and sink are in the same connected component.
    pub fn source_sink_connected(&self) -> bool {
        let n = self.nodes.len();
        if n < 2 { return false; }
        let mut parent: Vec<usize> = (0..n).collect();

        fn find(parent: &mut Vec<usize>, i: usize) -> usize {
            if parent[i] != i {
                parent[i] = find(parent, parent[i]);
            }
            parent[i]
        }

        for edge in &self.edges {
            if !edge.alive { continue; }
            let a = find(&mut parent, edge.from);
            let b = find(&mut parent, edge.to);
            if a != b { parent[a] = b; }
        }

        let source_idx = self.nodes.iter().position(|n| n.is_source);
        let sink_idx = self.nodes.iter().position(|n| n.is_sink);

        match (source_idx, sink_idx) {
            (Some(s), Some(t)) => find(&mut parent, s) == find(&mut parent, t),
            _ => false,
        }
    }

    /// Measure network performance: signal received at sink / signal sent from source.
    pub fn delivery_ratio(&self) -> f64 {
        let source_signal = self.nodes.iter()
            .filter(|n| n.is_source)
            .map(|n| n.signal_level)
            .sum::<f64>().max(0.001);
        let sink_signal = self.nodes.iter()
            .filter(|n| n.is_sink)
            .map(|n| n.signal_level)
            .sum::<f64>();
        (sink_signal / source_signal).min(1.0)
    }

    /// Average health of all living edges.
    pub fn average_edge_health(&self) -> f64 {
        let alive: Vec<&HyphalEdge> = self.edges.iter().filter(|e| e.alive).collect();
        if alive.is_empty() { return 0.0; }
        alive.iter().map(|e| e.health).sum::<f64>() / alive.len() as f64
    }

    /// Network density: fraction of possible edges that exist.
    pub fn density(&self) -> f64 {
        let n = self.nodes.len();
        if n < 2 { return 0.0; }
        let max_edges = n * (n - 1) / 2;
        let alive_edges = self.edges.iter().filter(|e| e.alive).count();
        alive_edges as f64 / max_edges as f64
    }

    /// Apply a stress event to the network.
    pub fn apply_stress(&mut self, stress_type: &str, intensity: f64, rng: &mut crate::rng::Rng) {
        match stress_type {
            "drought" => {
                // Drought kills edges from the periphery inward
                for edge in self.edges.iter_mut() {
                    if !edge.alive { continue; }
                    if rng.next_f64() < intensity * 0.3 {
                        edge.health *= 0.5;
                        if edge.health < 0.05 { edge.alive = false; }
                    }
                }
                for node in self.nodes.iter_mut() {
                    node.health_index *= 1.0 - intensity * 0.2;
                }
            }
            "toxin" => {
                // Toxin kills a localized region
                let center_x = rng.range(-self.network_radius, self.network_radius);
                let center_y = rng.range(-self.network_radius, self.network_radius);
                let kill_radius = self.network_radius * intensity * 0.3;
                for node in self.nodes.iter_mut() {
                    let dx = node.position[0] - center_x;
                    let dy = node.position[1] - center_y;
                    if (dx * dx + dy * dy).sqrt() < kill_radius {
                        node.health_index *= 0.1;
                    }
                }
            }
            "tilling" => {
                // Tilling physically severs edges
                for edge in self.edges.iter_mut() {
                    if !edge.alive { continue; }
                    if rng.next_f64() < intensity * 0.5 {
                        edge.alive = false;
                    }
                }
            }
            "pathogen" => {
                // Pathogen spreads along edges from a random node
                if self.nodes.is_empty() { return; }
                let start = rng.index(self.nodes.len());
                self.nodes[start].health_index = 0.0;
                // Spread to neighbors
                for edge in self.edges.iter_mut() {
                    if !edge.alive { continue; }
                    if edge.from == start || edge.to == start {
                        let other = if edge.from == start { edge.to } else { edge.from };
                        self.nodes[other].health_index *= 1.0 - intensity * 0.5;
                        edge.health *= 0.3;
                    }
                }
            }
            _ => {}
        }
    }
}
