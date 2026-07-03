use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../core/config.schema.json");
    ody_config::schema::write_config_schema(&out_path)?;
    println!("Schema written to {}", out_path.display());
    Ok(())
}
