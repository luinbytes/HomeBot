use homebot_protocol::ProtocolV1Schema;
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../protocol/schema/homebot-v1.schema.json");
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&schemars::schema_for!(ProtocolV1Schema))?
    );
    if env::args().any(|argument| argument == "--check") {
        if fs::read_to_string(&destination)? != rendered {
            return Err("protocol schema is stale; regenerate it".into());
        }
    } else {
        fs::write(destination, rendered)?;
    }
    Ok(())
}
