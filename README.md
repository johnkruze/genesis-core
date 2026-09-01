# genesis-core

First-principles physics bins. Dual-regime Monte Carlo. SHA-256 ProofChain on every trajectory. Pure CPU Rust.

Companion runtime: [ztp-runtime](https://github.com/johnkruze/ztp-runtime).

Topic 1 · 3 · 4 generators default to `n=2500`.

```bash
cargo test --lib physics::tesseract --offline
cargo run --release --bin tesseract_monte_carlo -- 2500
cargo run --release --bin materials_inverse_design_monte_carlo -- 2500
cargo run --release --bin autolab_dexterous_grasp_monte_carlo -- 2500
```

## Tesseract

Duffing IMU organ (`src/physics/tesseract.rs`). CPU Euler. Coriolis on the sense axis. Dual-regime (nonlinear drive vs bias floor). `alpha = 0` recovers the linear `DynamicOscillator` plateau. Clock: 100 Hz, 0.60 s, dt = 400 µs.

```bash
cargo run --release --bin tesseract_monte_carlo -- 2500
```
