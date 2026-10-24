use mailroom::rules::config::{Overrides, Preset};
fn main() {
    let directory = std::env::args().nth(1).expect("schema output directory");
    std::fs::create_dir_all(&directory).unwrap();
    for (name, schema) in [
        ("preset", schemars::schema_for!(Preset)),
        ("overrides", schemars::schema_for!(Overrides)),
    ] {
        std::fs::write(
            format!("{directory}/{name}.schema.json"),
            serde_json::to_vec_pretty(&schema).unwrap(),
        )
        .unwrap();
    }
}
