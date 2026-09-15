fn main() -> anyhow::Result<()> {
    let schema_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema");
    ody_app_server_protocol::write_schema_fixtures(&schema_root, None)
}
