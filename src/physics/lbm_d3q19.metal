#include <metal_stdlib>
using namespace metal;

// =====================================================================
// ZERO-TRUST PHYSICS: D3Q19 LATTICE BOLTZMANN METHOD (LBM) CFD KERNEL
// =====================================================================
// Matrix-free, meshless Navier-Stokes solver optimized for Apple Silicon
// Unified Memory Architecture (UMA).
//
// Discrete Velocity Directions (D3Q19):
// 0: (0,0,0)
// 1..6: Face neighbors (±1,0,0), (0,±1,0), (0,0,±1)
// 7..18: Edge neighbors (±1,±1,0), (±1,0,±1), (0,±1,±1)
// =====================================================================

constant int3 CX[19] = {
    int3( 0,  0,  0),
    int3( 1,  0,  0), int3(-1,  0,  0),
    int3( 0,  1,  0), int3( 0, -1,  0),
    int3( 0,  0,  1), int3( 0,  0, -1),
    int3( 1,  1,  0), int3(-1, -1,  0),
    int3( 1, -1,  0), int3(-1,  1,  0),
    int3( 1,  0,  1), int3(-1,  0, -1),
    int3( 1,  0, -1), int3(-1,  0,  1),
    int3( 0,  1,  1), int3( 0, -1, -1),
    int3( 0,  1, -1), int3( 0, -1,  1)
};

constant int OPPOSITE[19] = {
    0,
    2, 1,
    4, 3,
    6, 5,
    8, 7,
    10, 9,
    12, 11,
    14, 13,
    16, 15,
    18, 17
};

constant float WEIGHTS[19] = {
    1.0f / 3.0f,
    1.0f / 18.0f, 1.0f / 18.0f,
    1.0f / 18.0f, 1.0f / 18.0f,
    1.0f / 18.0f, 1.0f / 18.0f,
    1.0f / 36.0f, 1.0f / 36.0f,
    1.0f / 36.0f, 1.0f / 36.0f,
    1.0f / 36.0f, 1.0f / 36.0f,
    1.0f / 36.0f, 1.0f / 36.0f,
    1.0f / 36.0f, 1.0f / 36.0f,
    1.0f / 36.0f, 1.0f / 36.0f
};

// Node types
constant uint NODE_FLUID    = 0;
constant uint NODE_SOLID    = 1;
constant uint NODE_INLET    = 2;
constant uint NODE_OUTLET   = 3;

struct LbmParams {
    uint nx;
    uint ny;
    uint nz;
    float omega;       // Relaxation parameter: 1.0 / tau
    float u_inlet_x;   // Inflow velocity x
    float u_inlet_y;   // Inflow velocity y
    float u_inlet_z;   // Inflow velocity z
    float rho_inlet;   // Inflow density
};

// Linear index calculation
inline uint get_node_index(uint x, uint y, uint z, uint nx, uint ny) {
    return x + y * nx + z * nx * ny;
}

inline uint get_dist_index(uint node_idx, uint dir, uint total_nodes) {
    return node_idx + dir * total_nodes;
}

// Equilibrium distribution function (D3Q19)
inline float equilibrium(uint dir, float rho, float3 u) {
    float3 c = float3(CX[dir]);
    float c_dot_u = dot(c, u);
    float u_sq = dot(u, u);
    return WEIGHTS[dir] * rho * (1.0f + 3.0f * c_dot_u + 4.5f * c_dot_u * c_dot_u - 1.5f * u_sq);
}

// =====================================================================
// KERNEL: LBM BGK Collision & Streaming Step
// =====================================================================
kernel void lbm_d3q19_step(
    device const float* f_in         [[buffer(0)]],
    device float*       f_out        [[buffer(1)]],
    device const uint*  flags        [[buffer(2)]],
    device float4*      macro_state  [[buffer(3)]], // (rho, ux, uy, uz)
    device atomic_int*  force_accum  [[buffer(4)]], // Scaled momentum exchange: (Fx, Fy, Fz)
    constant LbmParams& params       [[buffer(5)]],
    uint3 id                         [[thread_position_in_grid]])
{
    uint x = id.x;
    uint y = id.y;
    uint z = id.z;

    if (x >= params.nx || y >= params.ny || z >= params.nz) {
        return;
    }

    uint total_nodes = params.nx * params.ny * params.nz;
    uint node_idx = get_node_index(x, y, z, params.nx, params.ny);
    uint node_type = flags[node_idx];

    // ─── 1. SOLID OBSTACLE (Bounce-Back is handled during streaming from fluid) ───
    if (node_type == NODE_SOLID) {
        macro_state[node_idx] = float4(1.0f, 0.0f, 0.0f, 0.0f);
        return;
    }

    // ─── 2. READ POPULATIONS & COMPUTE MACROSCOPIC PROPERTIES ───
    float f[19];
    float rho = 0.0f;
    float3 momentum = float3(0.0f);

    for (uint i = 0; i < 19; ++i) {
        f[i] = f_in[get_dist_index(node_idx, i, total_nodes)];
        rho += f[i];
        momentum += float3(CX[i]) * f[i];
    }

    float3 u = float3(0.0f);
    if (node_type == NODE_INLET) {
        rho = params.rho_inlet;
        u = float3(params.u_inlet_x, params.u_inlet_y, params.u_inlet_z);
    } else if (node_type == NODE_OUTLET) {
        // Zero gradient on velocity, fixed atmospheric pressure (rho = 1.0)
        rho = 1.0f;
        u = momentum / rho;
    } else {
        // Standard fluid cell
        rho = max(rho, 0.01f);
        u = momentum / rho;
    }

    // Write out macroscopic state for CPU / telemetry
    macro_state[node_idx] = float4(rho, u.x, u.y, u.z);

    // ─── 3. BGK COLLISION OPERATOR ───
    float f_post[19];
    for (uint i = 0; i < 19; ++i) {
        float f_eq = equilibrium(i, rho, u);
        f_post[i] = f[i] - params.omega * (f[i] - f_eq);
    }

    // ─── 4. STREAMING & MOMENTUM EXCHANGE (BOUNCE-BACK) ───
    for (uint i = 0; i < 19; ++i) {
        int3 c = CX[i];
        int target_x = int(x) + c.x;
        int target_y = int(y) + c.y;
        int target_z = int(z) + c.z;

        // Periodic boundaries along Y and Z if needed, or clamped walls
        if (target_x < 0 || target_x >= int(params.nx) ||
            target_y < 0 || target_y >= int(params.ny) ||
            target_z < 0 || target_z >= int(params.nz))
        {
            // Boundary bounce-back
            uint opp = OPPOSITE[i];
            f_out[get_dist_index(node_idx, opp, total_nodes)] = f_post[i];
            continue;
        }

        uint target_idx = get_node_index(uint(target_x), uint(target_y), uint(target_z), params.nx, params.ny);
        uint target_type = flags[target_idx];

        if (target_type == NODE_SOLID) {
            // Halfway bounce-back on obstacle surface
            uint opp = OPPOSITE[i];
            f_out[get_dist_index(node_idx, opp, total_nodes)] = f_post[i];

            // Momentum exchange force accumulation: F = 2 * c_i * f_post[i]
            // Scaled by 10000 for integer atomic fixed-point precision
            float3 force_delta = 2.0f * float3(c) * f_post[i] * 10000.0f;
            atomic_fetch_add_explicit(&force_accum[0], int(force_delta.x), memory_order_relaxed);
            atomic_fetch_add_explicit(&force_accum[1], int(force_delta.y), memory_order_relaxed);
            atomic_fetch_add_explicit(&force_accum[2], int(force_delta.z), memory_order_relaxed);
        } else {
            // Normal fluid-to-fluid streaming
            f_out[get_dist_index(target_idx, i, total_nodes)] = f_post[i];
        }
    }
}
