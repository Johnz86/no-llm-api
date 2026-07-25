//! Turns human-authorable YAML fixture sets into a parquet dataset, and lints them.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use no_llm_api::dataset::write_dataset;
use no_llm_api::fixtures::{builtin_sets, load_dir};

#[derive(Debug, Parser)]
#[command(
    name = "fixtures",
    about = "Build and lint the YAML fixture sets that back the mock"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write the fixture sets to a parquet dataset.
    Build {
        /// Directory of `*.yaml` fixture sets. Defaults to the built-in ones.
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long, default_value = "data/conversations.parquet")]
        output: PathBuf,
        /// Overwrite an existing dataset.
        #[arg(long)]
        force: bool,
    },
    /// Check the fixture sets without writing anything.
    Lint {
        #[arg(long)]
        input: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Build {
            input,
            output,
            force,
        } => {
            if output.exists() && !force {
                anyhow::bail!(
                    "{} already exists; pass --force to overwrite it",
                    output.display()
                );
            }
            let sets = sets_from(input)?;
            let rows: Vec<_> = sets.iter().flat_map(|set| set.to_rows()).collect();
            if let Some(parent) = output.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            write_dataset(&output, &rows)?;
            println!(
                "wrote {} rows from {} fixture sets to {}",
                rows.len(),
                sets.len(),
                output.display()
            );
        }
        Command::Lint { input } => {
            let sets = sets_from(input)?;
            for set in &sets {
                println!("ok {} ({} turns)", set.id, set.turns.len());
            }
            println!("{} fixture sets pass", sets.len());
        }
    }
    Ok(())
}

fn sets_from(input: Option<PathBuf>) -> Result<Vec<no_llm_api::fixtures::FixtureSet>> {
    match input {
        Some(path) => Ok(load_dir(&path)?),
        None => Ok(builtin_sets()),
    }
}
