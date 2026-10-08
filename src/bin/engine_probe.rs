#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use cache_cab::live::checkpoint_probe;
use cache_cab::llama::LlamaClient;

#[tokio::main]
async fn main() -> Result<()> {
    let mut endpoint = "http://127.0.0.1:7332".to_owned();
    let mut directory = None;
    let mut output = None;
    for argument in std::env::args().skip(1) {
        if let Some(value) = argument.strip_prefix("--endpoint=") {
            endpoint = value.to_owned();
        } else if let Some(value) = argument.strip_prefix("--checkpoint-dir=") {
            directory = Some(PathBuf::from(value));
        } else if let Some(value) = argument.strip_prefix("--output=") {
            output = Some(PathBuf::from(value));
        } else if argument == "--help" {
            println!("engine_probe --checkpoint-dir=PATH [--endpoint=http://127.0.0.1:7332] [--output=REPORT.json]\nUses and clears slot 0 of a dedicated llama.cpp server. No model download.");
            return Ok(());
        } else {
            bail!("Unknown argument: {argument}");
        }
    }
    let directory =
        directory.context("--checkpoint-dir must match the dedicated engine's --slot-save-path")?;
    let client = LlamaClient::new(&endpoint, Duration::from_secs(120))?;
    let report = checkpoint_probe(client, &directory).await?;
    let serialized = serde_json::to_string_pretty(&report)?;
    if let Some(output) = output {
        std::fs::write(output, format!("{serialized}\n"))?;
    }
    println!("{serialized}");
    Ok(())
}
