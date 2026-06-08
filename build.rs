use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=../schema/trajectory.fbs");

    flatc_rust::run(flatc_rust::Args {
        inputs: &[Path::new("../schema/trajectory.fbs")],
        out_dir: Path::new("src/schema/"),
        ..Default::default()
    }).expect("flatc compilation failed");
}
