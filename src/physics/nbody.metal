#include <metal_stdlib>
using namespace metal;

struct BodyData {
    float3 pos;
    float _pad0;
    float3 vel;
    float _pad1;
    float mass;
    float3 _pad2;
};

kernel void nbody_symplectic(
    device BodyData* bodies [[buffer(0)]],
    constant float& dt [[buffer(1)]],
    constant uint& num_bodies [[buffer(2)]],
    uint id [[thread_position_in_grid]]
) {
    if (id >= num_bodies) return;

    float3 pos = bodies[id].pos;
    float3 vel = bodies[id].vel;

    float3 accel = float3(0.0);
    const float G = 6.67430e-11;
    const float eps = 1e-3; // Softening parameter

    for (uint j = 0; j < num_bodies; j++) {
        if (id == j) continue;
        float3 r = bodies[j].pos - pos;
        float dist_sq = dot(r, r) + eps;
        float dist_inv = rsqrt(dist_sq);
        float dist_inv3 = dist_inv * dist_inv * dist_inv;
        accel += G * bodies[j].mass * r * dist_inv3;
    }

    // Symplectic Euler Integration
    vel += accel * dt;
    pos += vel * dt;

    bodies[id].pos = pos;
    bodies[id].vel = vel;
}
