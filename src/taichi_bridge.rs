use std::ffi::c_void;
use metal::*;
use objc::rc::autoreleasepool;

/// Matches Metal BodyData struct layout (48 bytes, 16-byte aligned float3 fields)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GpuBody {
    pub px: f32, pub py: f32, pub pz: f32, pub _p0: f32, // position (float3 + pad)
    pub vx: f32, pub vy: f32, pub vz: f32, pub _v0: f32, // velocity (float3 + pad)
    pub mass: f32, pub _m0: f32, pub _m1: f32, pub _m2: f32, // mass + pad to 48 bytes
}

pub struct MetalPhysicsBridge {
    device: Device,
    command_queue: CommandQueue,
    pipeline_state: ComputePipelineState,
}

impl MetalPhysicsBridge {
    pub fn new() -> Self {
        autoreleasepool(|| {
            let device = Device::system_default().expect("No Apple Metal GPU found on system. Ensure you are on a Mac.");
            let command_queue = device.new_command_queue();

            let src = include_str!("physics/nbody.metal");
            let options = CompileOptions::new();
            
            let library = device.new_library_with_source(src, &options).expect("Failed to compile Metal N-Body shader.");
            let function = library.get_function("nbody_symplectic", None).expect("Shader function nbody_symplectic not found.");
            
            let pipeline_state = device.new_compute_pipeline_state_with_function(&function).unwrap();

            Self {
                device,
                command_queue,
                pipeline_state,
            }
        })
    }

    /// Allocates an Nx7 tensor (position, velocity, mass) on the Metal GPU
    /// Returns a Boxed Metal Buffer leaked into a raw pointer
    pub fn allocate_orbital_tensor(&self, num_bodies: usize) -> *mut c_void {
        // We use 48-byte structs per body in MSL
        let size_per_body = 48; // aligned struct size
        let buffer = self.device.new_buffer(
            (num_bodies * size_per_body) as u64,
            MTLResourceOptions::StorageModeShared,
        );
        let ptr = Box::into_raw(Box::new(buffer));
        ptr as *mut c_void
    }

    /// Frees the allocated tensor
    pub fn free_orbital_tensor(&self, tensor: *mut c_void) {
        if !tensor.is_null() {
            unsafe {
                let _ = Box::from_raw(tensor as *mut Buffer);
            }
        }
    }

    /// Writes boulder data into the Metal buffer for GPU computation
    pub fn write_bodies(&self, tensor: *mut c_void, bodies: &[GpuBody]) {
        if tensor.is_null() || bodies.is_empty() { return; }
        let buffer: &Buffer = unsafe { &*(tensor as *const Buffer) };
        let ptr = buffer.contents() as *mut GpuBody;
        unsafe {
            std::ptr::copy_nonoverlapping(bodies.as_ptr(), ptr, bodies.len());
        }
    }

    /// Reads boulder data back from the Metal buffer after GPU computation
    pub fn read_bodies(&self, tensor: *mut c_void, bodies: &mut [GpuBody]) {
        if tensor.is_null() || bodies.is_empty() { return; }
        let buffer: &Buffer = unsafe { &*(tensor as *const Buffer) };
        let ptr = buffer.contents() as *const GpuBody;
        unsafe {
            std::ptr::copy_nonoverlapping(ptr, bodies.as_mut_ptr(), bodies.len());
        }
    }

    /// Dispatches the parallel N-Body symplectic integration shader on the Apple Silicon GPU
    pub fn dispatch_nbody_symplectic(&self, tensor: *mut c_void, num_bodies: usize, dt: f64) {
        if tensor.is_null() || num_bodies == 0 { return; }

        autoreleasepool(|| {
            let buffer: Box<Buffer> = unsafe { Box::from_raw(tensor as *mut Buffer) };
            
            let command_buffer = self.command_queue.new_command_buffer();
            let compute_encoder = command_buffer.new_compute_command_encoder();
            
            compute_encoder.set_compute_pipeline_state(&self.pipeline_state);
            compute_encoder.set_buffer(0, Some(&buffer), 0);
            
            let dt_f32 = dt as f32;
            compute_encoder.set_bytes(1, 4, &dt_f32 as *const f32 as *const c_void);
            
            let n_u32 = num_bodies as u32;
            compute_encoder.set_bytes(2, 4, &n_u32 as *const u32 as *const c_void);
            
            let grid_size = MTLSize::new(num_bodies as u64, 1, 1);
            let threadgroup_size = MTLSize::new(std::cmp::min(self.pipeline_state.max_total_threads_per_threadgroup(), num_bodies as u64), 1, 1);
            
            compute_encoder.dispatch_threads(grid_size, threadgroup_size);
            
            compute_encoder.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed(); // Synchronize with CPU unifying memory
            
            // Leak it back so the caller still owns the tensor pointer
            let _ = Box::into_raw(buffer);
        });
    }
}

impl Drop for MetalPhysicsBridge {
    fn drop(&mut self) {
        // Any cleanup if required
    }
}
