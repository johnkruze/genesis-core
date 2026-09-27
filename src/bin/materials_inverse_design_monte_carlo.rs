use std::time::Instant;
use genesis_core::proof::{self, ProofChain};
use genesis_core::output;
use genesis_core::rng::Rng;
use genesis_core::physics::materials::{MaterialSampleState, MaterialInverseParams};
use serde::Serialize;
use std::sync::Arc;
use arrow::array::{Float64Array, StringArray, BooleanArray};
use arrow::record_batch::RecordBatch;
use arrow::datatypes::{Schema, Field, DataType};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::file::properties::WriterProperties;
use parquet::basic::Compression;

/// Frozen Ti-64 box. Density is the existing `density_kg_m3` column in g/cm3
/// (divide by 1000). Yield is the existing `yield_strength_mpa` column.
/// The third gate is the bin's existing pass: `is_yield_failed == false`.
const TI64_DENSITY_MAX_G_CM3: f64 = 4.50;
const TI64_YIELD_MIN_MPA: f64 = 830.0;
const TI64_THIRD_GATE: &str = "is_yield_failed == false";

const DENSITY_LO_KG_M3: f64 = 1800.0;
const DENSITY_HI_KG_M3: f64 = 4500.0;
const LOAD_LO_KN: f64 = 200.0;
const LOAD_HI_KN: f64 = 1400.0;
const ALIGN_LO: f64 = 0.0;
const ALIGN_HI: f64 = 1.0;
const YIELD_LO: f64 = 150.0;
const YIELD_HI_ENGINE: f64 = 500.0;
const YIELD_HI_ROLL: f64 = 1200.0;

/// Elemental densities for the mass-fraction mixture, g/cm3.
/// Ti 4.51 Donachie; Al 2.70; V 6.11.
const RHO_TI_G_CM3: f64 = 4.51;
const RHO_AL_G_CM3: f64 = 2.70;
const RHO_V_G_CM3: f64 = 6.11;

/// Frozen load case for the composition search. 800 kN is the midpoint of
/// the existing load sweep. Alignment 0 is the misaligned end of the existing knob.
const COMPOSITION_LOAD_KN: f64 = 800.0;
const COMPOSITION_ALIGNMENT: f64 = 0.0;
const COMPOSITION_SEED: u64 = 0x434F_4D50_4F53_0001;

struct DrawCase {
    name: &'static str,
    generator: &'static str,
    density_max_g_cm3: f64,
    yield_min_mpa: f64,
    yield_lo: f64,
    yield_hi: f64,
    note: &'static str,
}

const CAMPAIGN: DrawCase = DrawCase {
    name: "campaign",
    generator: "G^G Eigenvector Inverse Material Design v1.0 — campaign",
    density_max_g_cm3: 4.50,
    yield_min_mpa: 400.0,
    yield_lo: YIELD_LO,
    yield_hi: YIELD_HI_ENGINE,
    note: "density and yield are draws from the existing sampler",
};

const YIELD_ROLL: DrawCase = DrawCase {
    name: "yield_roll",
    generator: "G^G Eigenvector Inverse Material Design v1.0 — yield_roll",
    density_max_g_cm3: 4.50,
    yield_min_mpa: 830.0,
    yield_lo: YIELD_LO,
    yield_hi: YIELD_HI_ROLL,
    note: "yield_strength_mpa is sampled. This roll is not an alloy",
};

struct CatalogSlot {
    slot: &'static str,
    w_ti: f64,
    w_al: f64,
    w_v: f64,
    temper: &'static str,
    yield_mpa: f64,
    yield_basis: &'static str,
}

/// Published slots. Yield is the tabulated number. Density is not stored here.
const CATALOG: &[CatalogSlot] = &[
    CatalogSlot {
        slot: "cp_ti_grade2",
        w_ti: 1.0,
        w_al: 0.0,
        w_v: 0.0,
        temper: "annealed",
        yield_mpa: 280.0,
        yield_basis: "Donachie minimum, ASTM grade 2",
    },
    CatalogSlot {
        slot: "cp_ti_grade4",
        w_ti: 1.0,
        w_al: 0.0,
        w_v: 0.0,
        temper: "annealed",
        yield_mpa: 480.0,
        yield_basis: "Donachie minimum, ASTM grade 4",
    },
    CatalogSlot {
        slot: "ti64_annealed_specmin",
        w_ti: 0.90,
        w_al: 0.06,
        w_v: 0.04,
        temper: "annealed",
        yield_mpa: 828.0,
        yield_basis: "ASTM B348 Grade 5 minimum, 828 MPa",
    },
    CatalogSlot {
        slot: "ti64_annealed_typical",
        w_ti: 0.90,
        w_al: 0.06,
        w_v: 0.04,
        temper: "annealed",
        yield_mpa: 880.0,
        yield_basis: "MatWeb mtp641 Allvac typical, 880 MPa",
    },
];

#[derive(Debug, Serialize)]
struct MaterialRunResult {
    id: u32,
    short_id: String,
    density_kg_m3: f64,
    applied_load_kn: f64,
    yield_strength_mpa: f64,
    eigenvector_alignment_score: f64,
    sigma_xx_mpa: f64,
    sigma_yy_mpa: f64,
    sigma_zz_mpa: f64,
    tau_xy_mpa: f64,
    tau_xz_mpa: f64,
    tau_yz_mpa: f64,
    principal_stress_1_mpa: f64,
    principal_stress_2_mpa: f64,
    principal_stress_3_mpa: f64,
    von_mises_stress_mpa: f64,
    safety_margin: f64,
    is_yield_failed: bool,
    proof_hash: String,
}

fn run_single_material(
    id: u32,
    rng: &mut Rng,
) -> MaterialRunResult {
    run_draw(id, rng, YIELD_LO, YIELD_HI_ENGINE)
}

fn run_draw(
    id: u32,
    rng: &mut Rng,
    yield_lo: f64,
    yield_hi: f64,
) -> MaterialRunResult {
    let short_id = output::short_id(rng);

    // Sweep density [1800, 4500) kg/m3, load [200, 1400) kN, alignment [0, 1).
    // Yield bounds are the caller's prior.
    let density = rng.range(DENSITY_LO_KG_M3, DENSITY_HI_KG_M3);
    let yield_mpa = rng.range(yield_lo, yield_hi);
    let load_kn = rng.range(LOAD_LO_KN, LOAD_HI_KN);
    let alignment = rng.range(ALIGN_LO, ALIGN_HI);

    let params = MaterialInverseParams::default();
    let mut state = MaterialSampleState::new(density, yield_mpa, load_kn, alignment);

    state.step(&params, 0.1);

    // Full 3D Cauchy tensor from stepped physics
    let t = state.stress_tensor;
    let sigma_xx = t.sigma_xx;
    let sigma_yy = t.sigma_yy;
    let sigma_zz = t.sigma_zz;
    let tau_xy = t.tau_xy;
    let tau_xz = t.tau_xz;
    let tau_yz = t.tau_yz;

    // True closed-form 3D Cauchy eigensolve (principal stresses lambda_1 >= lambda_2 >= lambda_3)
    let (principals, _eigenvectors) = t.solve_principal_eigensystem();
    let p1 = principals[0];
    let p2 = principals[1];
    let p3 = principals[2];

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_f64(density);
    proof.feed_f64(load_kn);
    proof.feed_f64(yield_mpa);
    proof.feed_f64(alignment);
    proof.feed_f64(state.von_mises_stress_mpa);
    proof.feed_f64(p1);
    proof.feed_f64(p2);
    proof.feed_f64(p3);
    proof.feed_f64(tau_yz);

    MaterialRunResult {
        id,
        short_id,
        density_kg_m3: density,
        applied_load_kn: load_kn,
        yield_strength_mpa: yield_mpa,
        eigenvector_alignment_score: alignment,
        sigma_xx_mpa: sigma_xx,
        sigma_yy_mpa: sigma_yy,
        sigma_zz_mpa: sigma_zz,
        tau_xy_mpa: tau_xy,
        tau_xz_mpa: tau_xz,
        tau_yz_mpa: tau_yz,
        principal_stress_1_mpa: p1,
        principal_stress_2_mpa: p2,
        principal_stress_3_mpa: p3,
        von_mises_stress_mpa: state.von_mises_stress_mpa,
        safety_margin: state.safety_margin,
        is_yield_failed: state.is_yield_failed,
        proof_hash: proof.seal(),
    }
}

fn write_parquet_dataset(path: &str, results: &[MaterialRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("density_kg_m3", DataType::Float64, false),
        Field::new("applied_load_kn", DataType::Float64, false),
        Field::new("yield_strength_mpa", DataType::Float64, false),
        Field::new("eigenvector_alignment_score", DataType::Float64, false),
        Field::new("sigma_xx_mpa", DataType::Float64, false),
        Field::new("sigma_yy_mpa", DataType::Float64, false),
        Field::new("sigma_zz_mpa", DataType::Float64, false),
        Field::new("tau_xy_mpa", DataType::Float64, false),
        Field::new("tau_xz_mpa", DataType::Float64, false),
        Field::new("tau_yz_mpa", DataType::Float64, false),
        Field::new("principal_stress_1_mpa", DataType::Float64, false),
        Field::new("principal_stress_2_mpa", DataType::Float64, false),
        Field::new("principal_stress_3_mpa", DataType::Float64, false),
        Field::new("von_mises_stress_mpa", DataType::Float64, false),
        Field::new("safety_margin", DataType::Float64, false),
        Field::new("is_yield_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
    ]));

    let traj_ids: StringArray = results.iter().map(|r| Some(format!("material_{}", r.short_id))).collect();
    let densities: Float64Array = results.iter().map(|r| Some(r.density_kg_m3)).collect();
    let loads: Float64Array = results.iter().map(|r| Some(r.applied_load_kn)).collect();
    let yields: Float64Array = results.iter().map(|r| Some(r.yield_strength_mpa)).collect();
    let alignments: Float64Array = results.iter().map(|r| Some(r.eigenvector_alignment_score)).collect();
    let s_xx: Float64Array = results.iter().map(|r| Some(r.sigma_xx_mpa)).collect();
    let s_yy: Float64Array = results.iter().map(|r| Some(r.sigma_yy_mpa)).collect();
    let s_zz: Float64Array = results.iter().map(|r| Some(r.sigma_zz_mpa)).collect();
    let t_xy: Float64Array = results.iter().map(|r| Some(r.tau_xy_mpa)).collect();
    let t_xz: Float64Array = results.iter().map(|r| Some(r.tau_xz_mpa)).collect();
    let t_yz: Float64Array = results.iter().map(|r| Some(r.tau_yz_mpa)).collect();
    let p1s: Float64Array = results.iter().map(|r| Some(r.principal_stress_1_mpa)).collect();
    let p2s: Float64Array = results.iter().map(|r| Some(r.principal_stress_2_mpa)).collect();
    let p3s: Float64Array = results.iter().map(|r| Some(r.principal_stress_3_mpa)).collect();
    let stresses: Float64Array = results.iter().map(|r| Some(r.von_mises_stress_mpa)).collect();
    let margins: Float64Array = results.iter().map(|r| Some(r.safety_margin)).collect();
    let failures: BooleanArray = results.iter().map(|r| Some(r.is_yield_failed)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(densities),
            Arc::new(loads),
            Arc::new(yields),
            Arc::new(alignments),
            Arc::new(s_xx),
            Arc::new(s_yy),
            Arc::new(s_zz),
            Arc::new(t_xy),
            Arc::new(t_xz),
            Arc::new(t_yz),
            Arc::new(p1s),
            Arc::new(p2s),
            Arc::new(p3s),
            Arc::new(stresses),
            Arc::new(margins),
            Arc::new(failures),
            Arc::new(proofs),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), "G^G Eigenvector Inverse Material Design v1.0".to_string()),
        ]))
        .build();

    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .expect("Failed to create Parquet ArrowWriter");
    writer.write(&batch).expect("Failed to write Parquet batch");
    writer.close().expect("Failed to close Parquet writer");

    Ok(())
}

fn density_g_cm3(density_kg_m3: f64) -> f64 {
    density_kg_m3 / 1000.0
}

fn ti64_passes(row: &MaterialRunResult) -> (bool, bool, bool, bool) {
    let density_ok = density_g_cm3(row.density_kg_m3) <= TI64_DENSITY_MAX_G_CM3;
    let yield_ok = row.yield_strength_mpa >= TI64_YIELD_MIN_MPA;
    let third_ok = !row.is_yield_failed;
    (density_ok, yield_ok, third_ok, density_ok && yield_ok && third_ok)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaseKind {
    Ti64Box,
    Campaign,
    YieldRoll,
    Composition,
}

struct CaseCli {
    n: u32,
    parquet: String,
    overwrite: bool,
    kind: CaseKind,
}

fn case_parquet_name(kind: CaseKind) -> &'static str {
    match kind {
        CaseKind::Ti64Box => "materials_ti64_box.parquet",
        CaseKind::Campaign => "materials_campaign_box.parquet",
        CaseKind::YieldRoll => "materials_yield_roll.parquet",
        CaseKind::Composition => "materials_composition_search.parquet",
    }
}

fn default_case_parquet(kind: CaseKind) -> String {
    format!(
        "{}/../../data/{}",
        env!("CARGO_MANIFEST_DIR"),
        case_parquet_name(kind)
    )
}

fn parse_case_kind(value: &str) -> Result<CaseKind, String> {
    match value {
        "ti64_box" => Ok(CaseKind::Ti64Box),
        "campaign" => Ok(CaseKind::Campaign),
        "yield_roll" => Ok(CaseKind::YieldRoll),
        "composition" => Ok(CaseKind::Composition),
        other => Err(format!(
            "--case {other} is not campaign, yield_roll, composition, or ti64_box"
        )),
    }
}

fn parse_case_cli(args: &[String]) -> Result<CaseCli, String> {
    let mut n: Option<u32> = None;
    let mut parquet: Option<String> = None;
    let mut overwrite = false;
    let mut kind: Option<CaseKind> = None;
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--overwrite" => {
                overwrite = true;
                i += 1;
            }
            "--parquet" => {
                parquet = Some(
                    args.get(i + 1)
                        .cloned()
                        .ok_or_else(|| "--parquet requires a value".to_string())?,
                );
                i += 2;
            }
            "--n" => {
                let value = args
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| "--n requires a value".to_string())?;
                n = Some(
                    value
                        .parse::<u32>()
                        .map_err(|e| format!("--n {value}: {e}"))?,
                );
                i += 2;
            }
            "--case" => {
                let value = args
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| "--case requires a value".to_string())?;
                kind = Some(parse_case_kind(&value)?);
                i += 2;
            }
            other => {
                if n.is_none() {
                    if let Ok(value) = other.parse::<u32>() {
                        n = Some(value);
                        i += 1;
                        continue;
                    }
                }
                return Err(format!("unknown argument {other}"));
            }
        }
    }
    let kind = kind.ok_or_else(|| "--case is required".to_string())?;
    Ok(CaseCli {
        n: n.unwrap_or(2_500),
        parquet: parquet.unwrap_or_else(|| default_case_parquet(kind)),
        overwrite,
        kind,
    })
}

fn write_ti64_parquet(path: &str, results: &[MaterialRunResult], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("density_kg_m3", DataType::Float64, false),
        Field::new("applied_load_kn", DataType::Float64, false),
        Field::new("yield_strength_mpa", DataType::Float64, false),
        Field::new("eigenvector_alignment_score", DataType::Float64, false),
        Field::new("sigma_xx_mpa", DataType::Float64, false),
        Field::new("sigma_yy_mpa", DataType::Float64, false),
        Field::new("sigma_zz_mpa", DataType::Float64, false),
        Field::new("tau_xy_mpa", DataType::Float64, false),
        Field::new("tau_xz_mpa", DataType::Float64, false),
        Field::new("tau_yz_mpa", DataType::Float64, false),
        Field::new("principal_stress_1_mpa", DataType::Float64, false),
        Field::new("principal_stress_2_mpa", DataType::Float64, false),
        Field::new("principal_stress_3_mpa", DataType::Float64, false),
        Field::new("von_mises_stress_mpa", DataType::Float64, false),
        Field::new("safety_margin", DataType::Float64, false),
        Field::new("is_yield_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
        Field::new("case_name", DataType::Utf8, false),
        Field::new("density_g_cm3", DataType::Float64, false),
        Field::new("spec_density_max_g_cm3", DataType::Float64, false),
        Field::new("spec_yield_min_mpa", DataType::Float64, false),
        Field::new("spec_third_gate", DataType::Utf8, false),
        Field::new("passes_density_gate", DataType::Boolean, false),
        Field::new("passes_yield_gate", DataType::Boolean, false),
        Field::new("passes_third_gate", DataType::Boolean, false),
        Field::new("is_feasible", DataType::Boolean, false),
    ]));

    let flags: Vec<(bool, bool, bool, bool)> = results.iter().map(ti64_passes).collect();
    let traj_ids: StringArray = results.iter().map(|r| Some(format!("material_{}", r.short_id))).collect();
    let densities: Float64Array = results.iter().map(|r| Some(r.density_kg_m3)).collect();
    let loads: Float64Array = results.iter().map(|r| Some(r.applied_load_kn)).collect();
    let yields: Float64Array = results.iter().map(|r| Some(r.yield_strength_mpa)).collect();
    let alignments: Float64Array = results.iter().map(|r| Some(r.eigenvector_alignment_score)).collect();
    let s_xx: Float64Array = results.iter().map(|r| Some(r.sigma_xx_mpa)).collect();
    let s_yy: Float64Array = results.iter().map(|r| Some(r.sigma_yy_mpa)).collect();
    let s_zz: Float64Array = results.iter().map(|r| Some(r.sigma_zz_mpa)).collect();
    let t_xy: Float64Array = results.iter().map(|r| Some(r.tau_xy_mpa)).collect();
    let t_xz: Float64Array = results.iter().map(|r| Some(r.tau_xz_mpa)).collect();
    let t_yz: Float64Array = results.iter().map(|r| Some(r.tau_yz_mpa)).collect();
    let p1s: Float64Array = results.iter().map(|r| Some(r.principal_stress_1_mpa)).collect();
    let p2s: Float64Array = results.iter().map(|r| Some(r.principal_stress_2_mpa)).collect();
    let p3s: Float64Array = results.iter().map(|r| Some(r.principal_stress_3_mpa)).collect();
    let stresses: Float64Array = results.iter().map(|r| Some(r.von_mises_stress_mpa)).collect();
    let margins: Float64Array = results.iter().map(|r| Some(r.safety_margin)).collect();
    let failures: BooleanArray = results.iter().map(|r| Some(r.is_yield_failed)).collect();
    let proofs: StringArray = results.iter().map(|r| Some(r.proof_hash.clone())).collect();
    let cases: StringArray = results.iter().map(|_| Some("ti64_box")).collect();
    let density_gcc: Float64Array = results.iter().map(|r| Some(density_g_cm3(r.density_kg_m3))).collect();
    let spec_density: Float64Array = results.iter().map(|_| Some(TI64_DENSITY_MAX_G_CM3)).collect();
    let spec_yield: Float64Array = results.iter().map(|_| Some(TI64_YIELD_MIN_MPA)).collect();
    let spec_third: StringArray = results.iter().map(|_| Some(TI64_THIRD_GATE)).collect();
    let pass_density: BooleanArray = flags.iter().map(|f| Some(f.0)).collect();
    let pass_yield: BooleanArray = flags.iter().map(|f| Some(f.1)).collect();
    let pass_third: BooleanArray = flags.iter().map(|f| Some(f.2)).collect();
    let feasible: BooleanArray = flags.iter().map(|f| Some(f.3)).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(traj_ids),
            Arc::new(densities),
            Arc::new(loads),
            Arc::new(yields),
            Arc::new(alignments),
            Arc::new(s_xx),
            Arc::new(s_yy),
            Arc::new(s_zz),
            Arc::new(t_xy),
            Arc::new(t_xz),
            Arc::new(t_yz),
            Arc::new(p1s),
            Arc::new(p2s),
            Arc::new(p3s),
            Arc::new(stresses),
            Arc::new(margins),
            Arc::new(failures),
            Arc::new(proofs),
            Arc::new(cases),
            Arc::new(density_gcc),
            Arc::new(spec_density),
            Arc::new(spec_yield),
            Arc::new(spec_third),
            Arc::new(pass_density),
            Arc::new(pass_yield),
            Arc::new(pass_third),
            Arc::new(feasible),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G Eigenvector Inverse Material Design v1.0 — ti64_box".to_string(),
            ),
            parquet::file::metadata::KeyValue::new("case_name".to_string(), "ti64_box".to_string()),
            parquet::file::metadata::KeyValue::new(
                "spec_density_g_cm3".to_string(),
                format!("<= {TI64_DENSITY_MAX_G_CM3}"),
            ),
            parquet::file::metadata::KeyValue::new(
                "spec_yield_mpa".to_string(),
                format!(">= {TI64_YIELD_MIN_MPA}"),
            ),
            parquet::file::metadata::KeyValue::new("spec_third_gate".to_string(), TI64_THIRD_GATE.to_string()),
        ]))
        .build();

    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .expect("Failed to create Parquet ArrowWriter");
    writer.write(&batch).expect("Failed to write Parquet batch");
    writer.close().expect("Failed to close Parquet writer");

    Ok(())
}

fn run_ti64_box(cli: &CaseCli) {
    if std::path::Path::new(&cli.parquet).exists() && !cli.overwrite {
        eprintln!(
            "refusing to write {}: file exists. Pass --overwrite to replace it.",
            cli.parquet
        );
        std::process::exit(1);
    }

    println!("case: ti64_box");
    println!("spec_density_g_cm3: <= {TI64_DENSITY_MAX_G_CM3}  (density_kg_m3 / 1000)");
    println!("spec_yield_mpa: >= {TI64_YIELD_MIN_MPA}  (column yield_strength_mpa)");
    println!("spec_third_gate: {TI64_THIRD_GATE}");
    println!("spec_third_gate_law: von_mises_stress_mpa <= yield_strength_mpa");
    println!("best_row_rule: max safety_margin, then min id");
    println!("n: {}", cli.n);

    let mut rng = Rng::new(0x4D41_5445_5249_414C);
    let start = Instant::now();
    let mut results = Vec::with_capacity(cli.n as usize);
    for i in 0..cli.n {
        results.push(run_single_material(i, &mut rng));
    }

    let mut run_chain = ProofChain::new();
    run_chain.seed(b"ti64_box");
    run_chain.feed_str("density_g_cm3<=");
    run_chain.feed_f64(TI64_DENSITY_MAX_G_CM3);
    run_chain.feed_str("yield_strength_mpa>=");
    run_chain.feed_f64(TI64_YIELD_MIN_MPA);
    run_chain.feed_str(TI64_THIRD_GATE);
    for row in &results {
        run_chain.feed_str(&row.proof_hash);
    }
    let run_proof = run_chain.seal();

    write_ti64_parquet(&cli.parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let n_feasible = results.iter().filter(|r| ti64_passes(r).3).count();
    let empty_set = n_feasible == 0;
    let best = results.iter().min_by(|a, b| {
        b.safety_margin
            .total_cmp(&a.safety_margin)
            .then_with(|| a.id.cmp(&b.id))
    });

    println!("n: {}", cli.n);
    println!("n_feasible: {n_feasible}");
    println!("empty_set: {empty_set}");
    match best {
        Some(row) => {
            let feasible = ti64_passes(row).3;
            println!(
                "best_row: id={} trajectory_id=material_{} density_kg_m3={:.6} density_g_cm3={:.6} yield_strength_mpa={:.6} safety_margin={:.6} von_mises_stress_mpa={:.6} eigenvector_alignment_score={:.6} applied_load_kn={:.6} is_yield_failed={} is_feasible={}",
                row.id,
                row.short_id,
                row.density_kg_m3,
                density_g_cm3(row.density_kg_m3),
                row.yield_strength_mpa,
                row.safety_margin,
                row.von_mises_stress_mpa,
                row.eigenvector_alignment_score,
                row.applied_load_kn,
                row.is_yield_failed,
                feasible
            );
        }
        None => println!("best_row: none"),
    }
    println!("seal: {run_proof}");
    println!("parquet: {}", cli.parquet);
    println!("wall_s: {:.3}", start.elapsed().as_secs_f64());
}

fn gate_passes(
    density_kg_m3: f64,
    yield_strength_mpa: f64,
    is_yield_failed: bool,
    density_max_g_cm3: f64,
    yield_min_mpa: f64,
) -> (bool, bool, bool, bool) {
    let density_ok = density_g_cm3(density_kg_m3) <= density_max_g_cm3;
    let yield_ok = yield_strength_mpa >= yield_min_mpa;
    let third_ok = !is_yield_failed;
    (density_ok, yield_ok, third_ok, density_ok && yield_ok && third_ok)
}

fn refuse_existing(path: &str, overwrite: bool) {
    if std::path::Path::new(path).exists() && !overwrite {
        eprintln!("refusing to write {path}: file exists. Pass --overwrite to replace it.");
        std::process::exit(1);
    }
}

fn write_draw_parquet(
    path: &str,
    results: &[MaterialRunResult],
    run_proof: &str,
    spec: &DrawCase,
) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("density_kg_m3", DataType::Float64, false),
        Field::new("applied_load_kn", DataType::Float64, false),
        Field::new("yield_strength_mpa", DataType::Float64, false),
        Field::new("eigenvector_alignment_score", DataType::Float64, false),
        Field::new("sigma_xx_mpa", DataType::Float64, false),
        Field::new("sigma_yy_mpa", DataType::Float64, false),
        Field::new("sigma_zz_mpa", DataType::Float64, false),
        Field::new("tau_xy_mpa", DataType::Float64, false),
        Field::new("tau_xz_mpa", DataType::Float64, false),
        Field::new("tau_yz_mpa", DataType::Float64, false),
        Field::new("principal_stress_1_mpa", DataType::Float64, false),
        Field::new("principal_stress_2_mpa", DataType::Float64, false),
        Field::new("principal_stress_3_mpa", DataType::Float64, false),
        Field::new("von_mises_stress_mpa", DataType::Float64, false),
        Field::new("safety_margin", DataType::Float64, false),
        Field::new("is_yield_failed", DataType::Boolean, false),
        Field::new("proof_hash", DataType::Utf8, false),
        Field::new("case_name", DataType::Utf8, false),
        Field::new("density_g_cm3", DataType::Float64, false),
        Field::new("spec_density_max_g_cm3", DataType::Float64, false),
        Field::new("spec_yield_min_mpa", DataType::Float64, false),
        Field::new("spec_third_gate", DataType::Utf8, false),
        Field::new("yield_draw_lo_mpa", DataType::Float64, false),
        Field::new("yield_draw_hi_mpa", DataType::Float64, false),
        Field::new("passes_density_gate", DataType::Boolean, false),
        Field::new("passes_yield_gate", DataType::Boolean, false),
        Field::new("passes_third_gate", DataType::Boolean, false),
        Field::new("is_feasible", DataType::Boolean, false),
    ]));

    let flags: Vec<(bool, bool, bool, bool)> = results
        .iter()
        .map(|r| {
            gate_passes(
                r.density_kg_m3,
                r.yield_strength_mpa,
                r.is_yield_failed,
                spec.density_max_g_cm3,
                spec.yield_min_mpa,
            )
        })
        .collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(results.iter().map(|r| Some(format!("material_{}", r.short_id))).collect::<StringArray>()),
            Arc::new(results.iter().map(|r| Some(r.density_kg_m3)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.applied_load_kn)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.yield_strength_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.eigenvector_alignment_score)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.sigma_xx_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.sigma_yy_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.sigma_zz_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.tau_xy_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.tau_xz_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.tau_yz_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.principal_stress_1_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.principal_stress_2_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.principal_stress_3_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.von_mises_stress_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.safety_margin)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|r| Some(r.is_yield_failed)).collect::<BooleanArray>()),
            Arc::new(results.iter().map(|r| Some(r.proof_hash.clone())).collect::<StringArray>()),
            Arc::new(results.iter().map(|_| Some(spec.name)).collect::<StringArray>()),
            Arc::new(results.iter().map(|r| Some(density_g_cm3(r.density_kg_m3))).collect::<Float64Array>()),
            Arc::new(results.iter().map(|_| Some(spec.density_max_g_cm3)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|_| Some(spec.yield_min_mpa)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|_| Some(TI64_THIRD_GATE)).collect::<StringArray>()),
            Arc::new(results.iter().map(|_| Some(spec.yield_lo)).collect::<Float64Array>()),
            Arc::new(results.iter().map(|_| Some(spec.yield_hi)).collect::<Float64Array>()),
            Arc::new(flags.iter().map(|f| Some(f.0)).collect::<BooleanArray>()),
            Arc::new(flags.iter().map(|f| Some(f.1)).collect::<BooleanArray>()),
            Arc::new(flags.iter().map(|f| Some(f.2)).collect::<BooleanArray>()),
            Arc::new(flags.iter().map(|f| Some(f.3)).collect::<BooleanArray>()),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new("generator".to_string(), spec.generator.to_string()),
            parquet::file::metadata::KeyValue::new("case_name".to_string(), spec.name.to_string()),
            parquet::file::metadata::KeyValue::new("note".to_string(), spec.note.to_string()),
            parquet::file::metadata::KeyValue::new(
                "spec_density_g_cm3".to_string(),
                format!("<= {}", spec.density_max_g_cm3),
            ),
            parquet::file::metadata::KeyValue::new(
                "spec_yield_mpa".to_string(),
                format!(">= {}", spec.yield_min_mpa),
            ),
            parquet::file::metadata::KeyValue::new("spec_third_gate".to_string(), TI64_THIRD_GATE.to_string()),
            parquet::file::metadata::KeyValue::new(
                "yield_draw".to_string(),
                format!("[{}, {})", spec.yield_lo, spec.yield_hi),
            ),
        ]))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .expect("Failed to create Parquet ArrowWriter");
    writer.write(&batch).expect("Failed to write Parquet batch");
    writer.close().expect("Failed to close Parquet writer");
    Ok(())
}

fn print_best_draw(row: &MaterialRunResult, feasible: bool) {
    println!(
        "best_row: id={} trajectory_id=material_{} density_kg_m3={:.6} density_g_cm3={:.6} yield_strength_mpa={:.6} safety_margin={:.6} von_mises_stress_mpa={:.6} eigenvector_alignment_score={:.6} applied_load_kn={:.6} is_yield_failed={} is_feasible={}",
        row.id,
        row.short_id,
        row.density_kg_m3,
        density_g_cm3(row.density_kg_m3),
        row.yield_strength_mpa,
        row.safety_margin,
        row.von_mises_stress_mpa,
        row.eigenvector_alignment_score,
        row.applied_load_kn,
        row.is_yield_failed,
        feasible
    );
}

fn run_draw_case(cli: &CaseCli, spec: &DrawCase) {
    refuse_existing(&cli.parquet, cli.overwrite);
    println!("case: {}", spec.name);
    println!("note: {}", spec.note);
    println!(
        "prior_density_kg_m3: [{DENSITY_LO_KG_M3}, {DENSITY_HI_KG_M3})"
    );
    println!("prior_yield_mpa: [{}, {})", spec.yield_lo, spec.yield_hi);
    println!("prior_load_kn: [{LOAD_LO_KN}, {LOAD_HI_KN})");
    println!("prior_alignment: [{ALIGN_LO}, {ALIGN_HI})");
    println!("spec_density_g_cm3: <= {}", spec.density_max_g_cm3);
    println!("spec_yield_mpa: >= {}", spec.yield_min_mpa);
    println!("spec_third_gate: {TI64_THIRD_GATE}");
    println!("spec_third_gate_law: von_mises_stress_mpa <= yield_strength_mpa");
    println!("best_row_rule: max safety_margin, then min id");
    println!("n: {}", cli.n);

    let mut rng = Rng::new(0x4D41_5445_5249_414C);
    let start = Instant::now();
    let mut results = Vec::with_capacity(cli.n as usize);
    for i in 0..cli.n {
        results.push(run_draw(i, &mut rng, spec.yield_lo, spec.yield_hi));
    }

    let mut run_chain = ProofChain::new();
    run_chain.seed(spec.name.as_bytes());
    run_chain.feed_str("yield_draw");
    run_chain.feed_f64(spec.yield_lo);
    run_chain.feed_f64(spec.yield_hi);
    run_chain.feed_str("density_g_cm3<=");
    run_chain.feed_f64(spec.density_max_g_cm3);
    run_chain.feed_str("yield_strength_mpa>=");
    run_chain.feed_f64(spec.yield_min_mpa);
    run_chain.feed_str(TI64_THIRD_GATE);
    for row in &results {
        run_chain.feed_str(&row.proof_hash);
    }
    let run_proof = run_chain.seal();
    write_draw_parquet(&cli.parquet, &results, &run_proof, spec)
        .expect("Failed to write Parquet dataset");

    let n_feasible = results
        .iter()
        .filter(|r| {
            gate_passes(
                r.density_kg_m3,
                r.yield_strength_mpa,
                r.is_yield_failed,
                spec.density_max_g_cm3,
                spec.yield_min_mpa,
            )
            .3
        })
        .count();
    let best = results.iter().min_by(|a, b| {
        b.safety_margin
            .total_cmp(&a.safety_margin)
            .then_with(|| a.id.cmp(&b.id))
    });
    println!("n: {}", cli.n);
    println!("n_feasible: {n_feasible}");
    println!("empty_set: {}", n_feasible == 0);
    match best {
        Some(row) => {
            let feasible = gate_passes(
                row.density_kg_m3,
                row.yield_strength_mpa,
                row.is_yield_failed,
                spec.density_max_g_cm3,
                spec.yield_min_mpa,
            )
            .3;
            print_best_draw(row, feasible);
        }
        None => println!("best_row: none"),
    }
    println!("seal: {run_proof}");
    println!("parquet: {}", cli.parquet);
    println!("wall_s: {:.3}", start.elapsed().as_secs_f64());
}

fn mixture_density_g_cm3(slot: &CatalogSlot) -> f64 {
    slot.w_ti * RHO_TI_G_CM3 + slot.w_al * RHO_AL_G_CM3 + slot.w_v * RHO_V_G_CM3
}

struct CompositionRow {
    result: MaterialRunResult,
    slot: &'static str,
    w_ti: f64,
    w_al: f64,
    w_v: f64,
    temper: &'static str,
    yield_basis: &'static str,
}

fn run_composition_row(id: u32, rng: &mut Rng) -> CompositionRow {
    let short_id = output::short_id(rng);
    let slot = &CATALOG[rng.index(CATALOG.len())];
    let density_gcc = mixture_density_g_cm3(slot);
    let density = density_gcc * 1000.0;
    let yield_mpa = slot.yield_mpa;
    let load_kn = COMPOSITION_LOAD_KN;
    let alignment = COMPOSITION_ALIGNMENT;

    let params = MaterialInverseParams::default();
    let mut state = MaterialSampleState::new(density, yield_mpa, load_kn, alignment);
    state.step(&params, 0.1);

    let t = state.stress_tensor;
    let (principals, _eigenvectors) = t.solve_principal_eigensystem();

    let mut proof = ProofChain::new();
    proof.seed(&id.to_le_bytes());
    proof.feed_str(slot.slot);
    proof.feed_str(slot.temper);
    proof.feed_str(slot.yield_basis);
    proof.feed_f64(slot.w_ti);
    proof.feed_f64(slot.w_al);
    proof.feed_f64(slot.w_v);
    proof.feed_f64(density);
    proof.feed_f64(load_kn);
    proof.feed_f64(yield_mpa);
    proof.feed_f64(alignment);
    proof.feed_f64(state.von_mises_stress_mpa);
    proof.feed_f64(principals[0]);
    proof.feed_f64(principals[1]);
    proof.feed_f64(principals[2]);
    proof.feed_f64(t.tau_yz);

    CompositionRow {
        result: MaterialRunResult {
            id,
            short_id,
            density_kg_m3: density,
            applied_load_kn: load_kn,
            yield_strength_mpa: yield_mpa,
            eigenvector_alignment_score: alignment,
            sigma_xx_mpa: t.sigma_xx,
            sigma_yy_mpa: t.sigma_yy,
            sigma_zz_mpa: t.sigma_zz,
            tau_xy_mpa: t.tau_xy,
            tau_xz_mpa: t.tau_xz,
            tau_yz_mpa: t.tau_yz,
            principal_stress_1_mpa: principals[0],
            principal_stress_2_mpa: principals[1],
            principal_stress_3_mpa: principals[2],
            von_mises_stress_mpa: state.von_mises_stress_mpa,
            safety_margin: state.safety_margin,
            is_yield_failed: state.is_yield_failed,
            proof_hash: proof.seal(),
        },
        slot: slot.slot,
        w_ti: slot.w_ti,
        w_al: slot.w_al,
        w_v: slot.w_v,
        temper: slot.temper,
        yield_basis: slot.yield_basis,
    }
}

fn write_composition_parquet(path: &str, rows: &[CompositionRow], run_proof: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let schema = Arc::new(Schema::new(vec![
        Field::new("trajectory_id", DataType::Utf8, false),
        Field::new("slot", DataType::Utf8, false),
        Field::new("w_ti", DataType::Float64, false),
        Field::new("w_al", DataType::Float64, false),
        Field::new("w_v", DataType::Float64, false),
        Field::new("temper", DataType::Utf8, false),
        Field::new("yield_basis", DataType::Utf8, false),
        Field::new("density_law", DataType::Utf8, false),
        Field::new("density_kg_m3", DataType::Float64, false),
        Field::new("density_g_cm3", DataType::Float64, false),
        Field::new("applied_load_kn", DataType::Float64, false),
        Field::new("yield_strength_mpa", DataType::Float64, false),
        Field::new("eigenvector_alignment_score", DataType::Float64, false),
        Field::new("von_mises_stress_mpa", DataType::Float64, false),
        Field::new("safety_margin", DataType::Float64, false),
        Field::new("is_yield_failed", DataType::Boolean, false),
        Field::new("sigma_xx_mpa", DataType::Float64, false),
        Field::new("sigma_yy_mpa", DataType::Float64, false),
        Field::new("sigma_zz_mpa", DataType::Float64, false),
        Field::new("tau_xy_mpa", DataType::Float64, false),
        Field::new("tau_xz_mpa", DataType::Float64, false),
        Field::new("tau_yz_mpa", DataType::Float64, false),
        Field::new("principal_stress_1_mpa", DataType::Float64, false),
        Field::new("principal_stress_2_mpa", DataType::Float64, false),
        Field::new("principal_stress_3_mpa", DataType::Float64, false),
        Field::new("proof_hash", DataType::Utf8, false),
        Field::new("case_name", DataType::Utf8, false),
        Field::new("spec_density_max_g_cm3", DataType::Float64, false),
        Field::new("spec_yield_min_mpa", DataType::Float64, false),
        Field::new("spec_third_gate", DataType::Utf8, false),
        Field::new("passes_density_gate", DataType::Boolean, false),
        Field::new("passes_yield_gate", DataType::Boolean, false),
        Field::new("passes_third_gate", DataType::Boolean, false),
        Field::new("is_feasible", DataType::Boolean, false),
    ]));
    let flags: Vec<(bool, bool, bool, bool)> = rows
        .iter()
        .map(|r| {
            gate_passes(
                r.result.density_kg_m3,
                r.result.yield_strength_mpa,
                r.result.is_yield_failed,
                TI64_DENSITY_MAX_G_CM3,
                TI64_YIELD_MIN_MPA,
            )
        })
        .collect();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(rows.iter().map(|r| Some(format!("material_{}", r.result.short_id))).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.slot)).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.w_ti)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.w_al)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.w_v)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.temper)).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.yield_basis)).collect::<StringArray>()),
            Arc::new(rows.iter().map(|_| Some("mass_fraction_mixture")).collect::<StringArray>()),
            Arc::new(rows.iter().map(|r| Some(r.result.density_kg_m3)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(density_g_cm3(r.result.density_kg_m3))).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.applied_load_kn)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.yield_strength_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.eigenvector_alignment_score)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.von_mises_stress_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.safety_margin)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.is_yield_failed)).collect::<BooleanArray>()),
            Arc::new(rows.iter().map(|r| Some(r.result.sigma_xx_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.sigma_yy_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.sigma_zz_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.tau_xy_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.tau_xz_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.tau_yz_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.principal_stress_1_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.principal_stress_2_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.principal_stress_3_mpa)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|r| Some(r.result.proof_hash.clone())).collect::<StringArray>()),
            Arc::new(rows.iter().map(|_| Some("composition")).collect::<StringArray>()),
            Arc::new(rows.iter().map(|_| Some(TI64_DENSITY_MAX_G_CM3)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|_| Some(TI64_YIELD_MIN_MPA)).collect::<Float64Array>()),
            Arc::new(rows.iter().map(|_| Some(TI64_THIRD_GATE)).collect::<StringArray>()),
            Arc::new(flags.iter().map(|f| Some(f.0)).collect::<BooleanArray>()),
            Arc::new(flags.iter().map(|f| Some(f.1)).collect::<BooleanArray>()),
            Arc::new(flags.iter().map(|f| Some(f.2)).collect::<BooleanArray>()),
            Arc::new(flags.iter().map(|f| Some(f.3)).collect::<BooleanArray>()),
        ],
    ).expect("Failed to create RecordBatch");

    let file = std::fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![
            parquet::file::metadata::KeyValue::new("cryptographic_seal".to_string(), run_proof.to_string()),
            parquet::file::metadata::KeyValue::new(
                "generator".to_string(),
                "G^G Eigenvector Inverse Material Design v1.0 — composition".to_string(),
            ),
            parquet::file::metadata::KeyValue::new("case_name".to_string(), "composition".to_string()),
            parquet::file::metadata::KeyValue::new(
                "note".to_string(),
                "yield is the published slot value; density is the elemental mixture".to_string(),
            ),
            parquet::file::metadata::KeyValue::new("density_law".to_string(), "mass_fraction_mixture".to_string()),
            parquet::file::metadata::KeyValue::new(
                "element_density_g_cm3".to_string(),
                format!("Ti {RHO_TI_G_CM3}, Al {RHO_AL_G_CM3}, V {RHO_V_G_CM3}"),
            ),
            parquet::file::metadata::KeyValue::new(
                "frozen_load_kn".to_string(),
                COMPOSITION_LOAD_KN.to_string(),
            ),
            parquet::file::metadata::KeyValue::new(
                "frozen_alignment".to_string(),
                COMPOSITION_ALIGNMENT.to_string(),
            ),
            parquet::file::metadata::KeyValue::new("spec_density_g_cm3".to_string(), "<= 4.5".to_string()),
            parquet::file::metadata::KeyValue::new("spec_yield_mpa".to_string(), ">= 830".to_string()),
            parquet::file::metadata::KeyValue::new("spec_third_gate".to_string(), TI64_THIRD_GATE.to_string()),
        ]))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .expect("Failed to create Parquet ArrowWriter");
    writer.write(&batch).expect("Failed to write Parquet batch");
    writer.close().expect("Failed to close Parquet writer");
    Ok(())
}

fn run_composition(cli: &CaseCli) {
    refuse_existing(&cli.parquet, cli.overwrite);
    println!("case: composition");
    println!("note: yield is the published slot value; density is the elemental mixture; not a discovered alloy");
    println!("decision_variables: catalog slot (w_ti, w_al, w_v, temper)");
    println!("density_law: mass_fraction_mixture  Ti {RHO_TI_G_CM3}  Al {RHO_AL_G_CM3}  V {RHO_V_G_CM3}");
    println!("frozen_load_kn: {COMPOSITION_LOAD_KN}");
    println!("frozen_alignment: {COMPOSITION_ALIGNMENT}");
    println!("spec_density_g_cm3: <= 4.50");
    println!("spec_yield_mpa: >= 830");
    println!("spec_third_gate: {TI64_THIRD_GATE}");
    println!("spec_third_gate_law: von_mises_stress_mpa <= yield_strength_mpa");
    println!("best_row_rule: max safety_margin, then min id");
    println!("n: {}", cli.n);
    for slot in CATALOG {
        let rho = mixture_density_g_cm3(slot);
        println!(
            "catalog: slot={} w_ti={} w_al={} w_v={} temper={} yield_mpa={} density_g_cm3={:.6} basis={}",
            slot.slot, slot.w_ti, slot.w_al, slot.w_v, slot.temper, slot.yield_mpa, rho, slot.yield_basis
        );
    }

    let mut rng = Rng::new(COMPOSITION_SEED);
    let start = Instant::now();
    let mut rows = Vec::with_capacity(cli.n as usize);
    for i in 0..cli.n {
        rows.push(run_composition_row(i, &mut rng));
    }

    let mut run_chain = ProofChain::new();
    run_chain.seed(b"composition");
    run_chain.feed_str("density_g_cm3<=");
    run_chain.feed_f64(TI64_DENSITY_MAX_G_CM3);
    run_chain.feed_str("yield_strength_mpa>=");
    run_chain.feed_f64(TI64_YIELD_MIN_MPA);
    run_chain.feed_str(TI64_THIRD_GATE);
    run_chain.feed_f64(COMPOSITION_LOAD_KN);
    run_chain.feed_f64(COMPOSITION_ALIGNMENT);
    for row in &rows {
        run_chain.feed_str(&row.result.proof_hash);
    }
    let run_proof = run_chain.seal();
    write_composition_parquet(&cli.parquet, &rows, &run_proof)
        .expect("Failed to write Parquet dataset");

    let feasible_flags: Vec<bool> = rows
        .iter()
        .map(|r| {
            gate_passes(
                r.result.density_kg_m3,
                r.result.yield_strength_mpa,
                r.result.is_yield_failed,
                TI64_DENSITY_MAX_G_CM3,
                TI64_YIELD_MIN_MPA,
            )
            .3
        })
        .collect();
    let n_feasible = feasible_flags.iter().filter(|f| **f).count();
    let best = rows.iter().min_by(|a, b| {
        b.result
            .safety_margin
            .total_cmp(&a.result.safety_margin)
            .then_with(|| a.result.id.cmp(&b.result.id))
    });

    println!("n: {}", cli.n);
    println!("n_feasible: {n_feasible}");
    println!("empty_set: {}", n_feasible == 0);
    for slot in CATALOG {
        let n_slot = rows.iter().filter(|r| r.slot == slot.slot).count();
        let n_slot_ok = rows
            .iter()
            .zip(feasible_flags.iter())
            .filter(|(r, ok)| r.slot == slot.slot && **ok)
            .count();
        println!("slot_count: {} n={n_slot} n_feasible={n_slot_ok}", slot.slot);
    }
    match best {
        Some(row) => {
            let feasible = gate_passes(
                row.result.density_kg_m3,
                row.result.yield_strength_mpa,
                row.result.is_yield_failed,
                TI64_DENSITY_MAX_G_CM3,
                TI64_YIELD_MIN_MPA,
            )
            .3;
            println!(
                "best_row: id={} trajectory_id=material_{} slot={} temper={} w_ti={} w_al={} w_v={} density_g_cm3={:.6} yield_strength_mpa={:.6} von_mises_stress_mpa={:.6} safety_margin={:.6} applied_load_kn={:.6} is_yield_failed={} is_feasible={} basis={}",
                row.result.id,
                row.result.short_id,
                row.slot,
                row.temper,
                row.w_ti,
                row.w_al,
                row.w_v,
                density_g_cm3(row.result.density_kg_m3),
                row.result.yield_strength_mpa,
                row.result.von_mises_stress_mpa,
                row.result.safety_margin,
                row.result.applied_load_kn,
                row.result.is_yield_failed,
                feasible,
                row.yield_basis
            );
        }
        None => println!("best_row: none"),
    }
    println!("seal: {run_proof}");
    println!("parquet: {}", cli.parquet);
    println!("wall_s: {:.3}", start.elapsed().as_secs_f64());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--case") {
        match parse_case_cli(&args) {
            Ok(cli) => match cli.kind {
                CaseKind::Ti64Box => run_ti64_box(&cli),
                CaseKind::Campaign => run_draw_case(&cli, &CAMPAIGN),
                CaseKind::YieldRoll => run_draw_case(&cli, &YIELD_ROLL),
                CaseKind::Composition => run_composition(&cli),
            },
            Err(err) => {
                eprintln!("{err}");
                std::process::exit(2);
            }
        }
        return;
    }

    let n_trajectories: u32 = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_500);

    let out_parquet = args.iter().position(|a| a == "--parquet")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "../../data/materials_inverse_design_eigenvectors.parquet".to_string());

    println!("====================================================================");
    println!("  G^G KERNEL: ADVANCED MATERIALS INVERSE DESIGN SWEEP");
    println!("  Target Trajectories: {}", n_trajectories);
    println!("  Simulating Cauchy Stress Tensors, Von Mises Yield & Eigenvector Alignment...");
    println!("====================================================================\n");

    let mut rng = Rng::new(0x4D41_5445_5249_414C);
    let start = Instant::now();

    let mut results = Vec::with_capacity(n_trajectories as usize);
    for i in 0..n_trajectories {
        results.push(run_single_material(i, &mut rng));
    }

    let proof_hashes: Vec<_> = results.iter().map(|r| r.proof_hash.clone()).collect();
    let run_proof = proof::seal_run(&proof_hashes);

    write_parquet_dataset(&out_parquet, &results, &run_proof)
        .expect("Failed to write Parquet dataset");

    let passed_runs = results.iter().filter(|r| !r.is_yield_failed).count();
    let failed_runs = n_trajectories as usize - passed_runs;

    println!("====================================================================");
    println!("  MATERIALS INVERSE DESIGN SWEEP COMPLETE");
    println!("  Total Trajectories Simulated: {}", n_trajectories);
    println!("  Structural Inverse Design Passes:   {} ({:.1}%)", passed_runs, (passed_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Yield Limit Exceeded Failures:     {} ({:.1}%)", failed_runs, (failed_runs as f64 / n_trajectories as f64) * 100.0);
    println!("  Master SHA-256 Run Proof:           {}", run_proof);
    println!("  Simulation Time:                    {:?}", start.elapsed());
    println!("  Parquet Dataset Written To:          {}", out_parquet);
    println!("====================================================================\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(density_kg_m3: f64, yield_strength_mpa: f64, is_yield_failed: bool) -> MaterialRunResult {
        MaterialRunResult {
            id: 0,
            short_id: "t".to_string(),
            density_kg_m3,
            applied_load_kn: 0.0,
            yield_strength_mpa,
            eigenvector_alignment_score: 0.0,
            sigma_xx_mpa: 0.0,
            sigma_yy_mpa: 0.0,
            sigma_zz_mpa: 0.0,
            tau_xy_mpa: 0.0,
            tau_xz_mpa: 0.0,
            tau_yz_mpa: 0.0,
            principal_stress_1_mpa: 0.0,
            principal_stress_2_mpa: 0.0,
            principal_stress_3_mpa: 0.0,
            von_mises_stress_mpa: 0.0,
            safety_margin: 0.0,
            is_yield_failed,
            proof_hash: String::new(),
        }
    }

    #[test]
    fn ti64_box_is_frozen() {
        assert!((TI64_DENSITY_MAX_G_CM3 - 4.50).abs() < 1e-12);
        assert!((TI64_YIELD_MIN_MPA - 830.0).abs() < 1e-12);
        assert_eq!(TI64_THIRD_GATE, "is_yield_failed == false");
        let (d, y, t, ok) = ti64_passes(&row(4500.0, 830.0, false));
        assert!(d && y && t && ok);
        assert!(!ti64_passes(&row(4500.1, 830.0, false)).3);
        assert!(!ti64_passes(&row(4500.0, 829.999, false)).3);
        assert!(!ti64_passes(&row(4500.0, 830.0, true)).3);
        assert!((density_g_cm3(4500.0) - 4.50).abs() < 1e-12);
    }

    #[test]
    fn campaign_box_sits_inside_the_engine_prior() {
        assert!(CAMPAIGN.yield_min_mpa >= CAMPAIGN.yield_lo);
        assert!(CAMPAIGN.yield_min_mpa < CAMPAIGN.yield_hi);
        assert!((CAMPAIGN.yield_lo - 150.0).abs() < 1e-12);
        assert!((CAMPAIGN.yield_hi - 500.0).abs() < 1e-12);
        assert!((CAMPAIGN.yield_min_mpa - 400.0).abs() < 1e-12);
        assert!((CAMPAIGN.density_max_g_cm3 - 4.50).abs() < 1e-12);
        let (_d, _y, _t, ok) = gate_passes(4400.0, 400.0, false, CAMPAIGN.density_max_g_cm3, CAMPAIGN.yield_min_mpa);
        assert!(ok);
        assert!(!gate_passes(4400.0, 399.0, false, CAMPAIGN.density_max_g_cm3, CAMPAIGN.yield_min_mpa).3);
    }

    #[test]
    fn yield_roll_widens_the_draw_and_keeps_830() {
        assert!((YIELD_ROLL.yield_lo - 150.0).abs() < 1e-12);
        assert!((YIELD_ROLL.yield_hi - 1200.0).abs() < 1e-12);
        assert!((YIELD_ROLL.yield_min_mpa - 830.0).abs() < 1e-12);
        assert!(YIELD_ROLL.yield_min_mpa > YIELD_HI_ENGINE);
        assert!(YIELD_ROLL.yield_min_mpa < YIELD_ROLL.yield_hi);
    }

    #[test]
    fn composition_map_is_frozen() {
        assert_eq!(CATALOG.len(), 4);
        for slot in CATALOG {
            let sum = slot.w_ti + slot.w_al + slot.w_v;
            assert!((sum - 1.0).abs() < 1e-12, "{}", slot.slot);
        }
        let ti64 = CATALOG.iter().find(|s| s.slot == "ti64_annealed_typical").unwrap();
        let specmin = CATALOG.iter().find(|s| s.slot == "ti64_annealed_specmin").unwrap();
        let rho = mixture_density_g_cm3(ti64);
        assert!(rho <= TI64_DENSITY_MAX_G_CM3);
        assert!(ti64.yield_mpa >= TI64_YIELD_MIN_MPA);
        assert!(specmin.yield_mpa < TI64_YIELD_MIN_MPA);
        assert!((COMPOSITION_LOAD_KN - 800.0).abs() < 1e-12);
        assert!((COMPOSITION_ALIGNMENT - 0.0).abs() < 1e-12);
        let cp = CATALOG.iter().find(|s| s.slot == "cp_ti_grade2").unwrap();
        assert!(mixture_density_g_cm3(cp) > TI64_DENSITY_MAX_G_CM3);
    }
}
