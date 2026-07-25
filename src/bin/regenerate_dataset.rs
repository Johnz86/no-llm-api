use std::path::PathBuf;

use clap::Parser;

/// Rewrites the bundled sample dataset from the fixture source in `dataset.rs`.
#[derive(Debug, Parser)]
#[command(
    name = "regenerate_dataset",
    about = "Regenerate the bundled parquet fixture"
)]
struct Args {
    /// Where to write the dataset.
    #[arg(long, default_value = "data/conversations.parquet")]
    output: PathBuf,

    /// Overwrite an existing file. Without this the command refuses to act,
    /// rather than silently doing nothing as it used to.
    #[arg(long)]
    force: bool,
}

fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = Args::parse();

    if args.output.exists() && !args.force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite it",
            args.output.display()
        );
    }

    let path = no_llm_api::dataset::write_sample_dataset(&args.output)?;
    println!("wrote {}", path.display());
    Ok(())
}
