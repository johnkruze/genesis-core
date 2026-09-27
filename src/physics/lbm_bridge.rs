//! G^G Genesis Core: Apple Metal GPU Lattice Boltzmann (LBM) CFD Bridge
//!
//! Executes real-time 3D D3Q19 fluid dynamics directly on Apple Silicon
//! Unified Memory Architecture (UMA) with zero PCIe transfer overhead.

use std::ffi::c_void;
use metal::*;
use objc::rc::autoreleasepool;

pub const NODE_FLUID: u32 = 0;
pub const NODE_SOLID: u32 = 1;
pub const NODE_INLET: u32 = 2;
pub const NODE_OUTLET: u32 = 3;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LbmParams {
    pub nx: u32,
    pub ny: u32,
    pub nz: u32,
    pub omega: f32,
    pub u_inlet_x: f32,
    pub u_inlet_y: f32,
    pub u_inlet_z: f32,
    pub rho_inlet: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AerodynamicForces {
    pub drag_force_n: f64,
    pub lift_force_n: f64,
    pub lateral_force_n: f64,
    pub drag_coefficient_cd: f64,
    pub lift_coefficient_cl: f64,
}

pub struct MetalLbmBridge {
    _device: Device,
    command_queue: CommandQueue,
    pipeline_state: ComputePipelineState,
    
    // Grid dimensions
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    total_nodes: usize,

    // GPU Buffers (Shared memory on Apple Silicon UMA)
    f_in_buffer: Buffer,
    f_out_buffer: Buffer,
    flags_buffer: Buffer,
    macro_buffer: Buffer,
    force_buffer: Buffer,
    params: LbmParams,
}

impl MetalLbmBridge {
    pub fn new(nx: usize, ny: usize, nz: usize, viscosity: f32, u_inlet: f32) -> Result<Self, String> {
        autoreleasepool(|| {
            let device = Device::system_default()
                .ok_or_else(|| "No Apple Metal GPU found on system. Ensure you are on a Mac.".to_string())?;
            let command_queue = device.new_command_queue();

            let src = include_str!("lbm_d3q19.metal");
            let options = CompileOptions::new();

            let library = device.new_library_with_source(src, &options)
                .map_err(|e| format!("Failed to compile LBM Metal Shader: {}", e))?;
            let function = library.get_function("lbm_d3q19_step", None)
                .map_err(|e| format!("Shader function lbm_d3q19_step not found: {}", e))?;

            let pipeline_state = device.new_compute_pipeline_state_with_function(&function)
                .map_err(|e| format!("Failed to create compute pipeline: {}", e))?;

            let total_nodes = nx * ny * nz;
            let dist_size = (19 * total_nodes * std::mem::size_of::<f32>()) as u64;
            let flags_size = (total_nodes * std::mem::size_of::<u32>()) as u64;
            let macro_size = (total_nodes * 4 * std::mem::size_of::<f32>()) as u64;
            let force_size = (4 * std::mem::size_of::<i32>()) as u64;

            let f_in_buffer = device.new_buffer(dist_size, MTLResourceOptions::StorageModeShared);
            let f_out_buffer = device.new_buffer(dist_size, MTLResourceOptions::StorageModeShared);
            let flags_buffer = device.new_buffer(flags_size, MTLResourceOptions::StorageModeShared);
            let macro_buffer = device.new_buffer(macro_size, MTLResourceOptions::StorageModeShared);
            let force_buffer = device.new_buffer(force_size, MTLResourceOptions::StorageModeShared);

            // Relaxation parameter: tau = 3 * nu + 0.5, omega = 1 / tau
            let tau = 3.0 * viscosity + 0.5;
            let omega = 1.0 / tau.max(0.51);

            let params = LbmParams {
                nx: nx as u32,
                ny: ny as u32,
                nz: nz as u32,
                omega,
                u_inlet_x: u_inlet,
                u_inlet_y: 0.0,
                u_inlet_z: 0.0,
                rho_inlet: 1.0,
            };

            let mut bridge = Self {
                _device: device,
                command_queue,
                pipeline_state,
                nx,
                ny,
                nz,
                total_nodes,
                f_in_buffer,
                f_out_buffer,
                flags_buffer,
                macro_buffer,
                force_buffer,
                params,
            };

            bridge.initialize_equilibrium();
            Ok(bridge)
        })
    }

    /// Initializes distribution functions to uniform resting equilibrium
    fn initialize_equilibrium(&mut self) {
        let weights = [
            1.0f32 / 3.0,
            1.0 / 18.0, 1.0 / 18.0, 1.0 / 18.0, 1.0 / 18.0, 1.0 / 18.0, 1.0 / 18.0,
            1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0,
            1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0, 1.0 / 36.0,
        ];

        let f_ptr = self.f_in_buffer.contents() as *mut f32;
        let flags_ptr = self.flags_buffer.contents() as *mut u32;

        unsafe {
            for dir in 0..19 {
                let w = weights[dir];
                for i in 0..self.total_nodes {
                    *f_ptr.add(i + dir * self.total_nodes) = w;
                }
            }

            // Set inlet (x=0) and outlet (x=nx-1)
            for z in 0..self.nz {
                for y in 0..self.ny {
                    let inlet_idx = 0 + y * self.nx + z * self.nx * self.ny;
                    let outlet_idx = (self.nx - 1) + y * self.nx + z * self.nx * self.ny;
                    *flags_ptr.add(inlet_idx) = NODE_INLET;
                    *flags_ptr.add(outlet_idx) = NODE_OUTLET;
                }
            }
        }
    }

    /// Voxelizes a NACA 0012 symmetric airfoil shape into the grid
    pub fn voxelize_airfoil(&mut self, chord_nodes: usize, angle_of_attack_deg: f64) {
        let flags_ptr = self.flags_buffer.contents() as *mut u32;
        let cx = (self.nx as f64) * 0.25;
        let cy = (self.ny as f64) * 0.50;
        let cz_min = self.nz / 4;
        let cz_max = 3 * self.nz / 4;
        let chord = chord_nodes as f64;
        let rad = angle_of_attack_deg.to_radians();
        let cos_a = rad.cos();
        let sin_a = rad.sin();

        for z in cz_min..cz_max {
            for y in 0..self.ny {
                for x in 0..self.nx {
                    // Coordinate relative to leading edge
                    let dx = x as f64 - cx;
                    let dy = y as f64 - cy;

                    // Rotate by angle of attack
                    let x_rot = dx * cos_a + dy * sin_a;
                    let y_rot = -dx * sin_a + dy * cos_a;

                    if x_rot >= 0.0 && x_rot <= chord {
                        let x_norm = x_rot / chord;
                        // NACA 0012 thickness distribution equation
                        let yt = 0.60 * (0.2969 * x_norm.sqrt()
                            - 0.1260 * x_norm
                            - 0.3516 * x_norm.powi(2)
                            + 0.2843 * x_norm.powi(3)
                            - 0.1015 * x_norm.powi(4)) * chord;

                        if y_rot.abs() <= yt {
                            let idx = x + y * self.nx + z * self.nx * self.ny;
                            unsafe {
                                *flags_ptr.add(idx) = NODE_SOLID;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Dispatches N Lattice Boltzmann collision & streaming steps on the Apple GPU
    pub fn step(&mut self, num_steps: usize) {
        autoreleasepool(|| {
            for _ in 0..num_steps {
                let command_buffer = self.command_queue.new_command_buffer();
                let compute_encoder = command_buffer.new_compute_command_encoder();

                compute_encoder.set_compute_pipeline_state(&self.pipeline_state);
                compute_encoder.set_buffer(0, Some(&self.f_in_buffer), 0);
                compute_encoder.set_buffer(1, Some(&self.f_out_buffer), 0);
                compute_encoder.set_buffer(2, Some(&self.flags_buffer), 0);
                compute_encoder.set_buffer(3, Some(&self.macro_buffer), 0);
                compute_encoder.set_buffer(4, Some(&self.force_buffer), 0);

                let params_ptr = &self.params as *const LbmParams as *const c_void;
                compute_encoder.set_bytes(5, std::mem::size_of::<LbmParams>() as u64, params_ptr);

                let grid_size = MTLSize {
                    width: self.nx as u64,
                    height: self.ny as u64,
                    depth: self.nz as u64,
                };

                let threadgroup_size = MTLSize {
                    width: 8,
                    height: 8,
                    depth: 4,
                };

                compute_encoder.dispatch_threads(grid_size, threadgroup_size);
                compute_encoder.end_encoding();

                command_buffer.commit();
                command_buffer.wait_until_completed();

                // Ping-pong buffers
                std::mem::swap(&mut self.f_in_buffer, &mut self.f_out_buffer);
            }
        });
    }

    /// Extracts total integrated aerodynamic forces and non-dimensional coefficients
    pub fn get_aerodynamic_forces(&self) -> AerodynamicForces {
        let force_ptr = self.force_buffer.contents() as *const i32;
        let (fx_raw, fy_raw, fz_raw) = unsafe {
            (*force_ptr, *force_ptr.add(1), *force_ptr.add(2))
        };

        // Unscale fixed-point atomic integer
        let fx = (fx_raw as f64) / 10000.0;
        let fy = (fy_raw as f64) / 10000.0;
        let fz = (fz_raw as f64) / 10000.0;

        let u_inf = self.params.u_inlet_x as f64;
        let rho_inf = self.params.rho_inlet as f64;
        let dynamic_pressure = 0.5 * rho_inf * u_inf * u_inf;
        let ref_area = (self.ny as f64) * 0.25 * (self.nz as f64) * 0.5;

        let cd = if dynamic_pressure > 1e-6 && ref_area > 1e-6 {
            (fx / (dynamic_pressure * ref_area)).abs()
        } else {
            0.0
        };

        let cl = if dynamic_pressure > 1e-6 && ref_area > 1e-6 {
            fz / (dynamic_pressure * ref_area)
        } else {
            0.0
        };

        AerodynamicForces {
            drag_force_n: fx,
            lift_force_n: fz,
            lateral_force_n: fy,
            drag_coefficient_cd: (cd * 1000.0).round() / 1000.0,
            lift_coefficient_cl: (cl * 1000.0).round() / 1000.0,
        }
    }
}
