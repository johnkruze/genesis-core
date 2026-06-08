#include <metal_stdlib>
using namespace metal;

struct BodyData {
    float3 position;
    float3 velocity;
    float mass;
};

// Simple Symplectic Euler or Kick-Drift algorithm kernel
// We use a 1D grid where each thread processes one body
kernel void nbody_symplectic(
    device BodyData* bodies [[ buffer(0) ]],
    constant float& dt [[ buffer(1) ]],
    constant uint& num_bodies [[ buffer(2) ]],
    uint id [[ thread_position_in_grid ]]
) {
    if (id >= num_bodies) {
        return;
    }

    BodyData me = bodies[id];
    float3 force = float3(0.0);
    float g = 6.67430e-20; // G in km³/(kg·s²) — matching CPU rubble.rs units

    // Compute forces (O(N^2) naive parallel)
    for (uint i = 0; i < num_bodies; i++) {
        if (i == id) continue;
        
        BodyData other = bodies[i];
        float3 r = other.position - me.position;
        float dist_sq = length_squared(r) + 1e-4; // Softening
        float dist = sqrt(dist_sq);
        float dist_cubed = dist_sq * dist;
        
        force += (g * me.mass * other.mass / dist_cubed) * r;
    }
    
    // Kick
    float3 acc = force / me.mass;
    me.velocity += acc * dt;
    
    // Drift (Sync point handled implicitly by host loop in simple integrations,
    // but here we just do a single symplectic step per kernel launch)
    me.position += me.velocity * dt;
    
    bodies[id] = me;
}
