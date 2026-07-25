use std::path::Path;

fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    no_llm_api::dataset::ensure_sample_dataset(Path::new("data/conversations.parquet"))?;
    println!("refreshed data/conversations.parquet");
    Ok(())
}
