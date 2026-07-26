//! Turns human-authorable YAML fixture sets into a parquet dataset, and lints them.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use no_llm_api::dataset::write_dataset;
use no_llm_api::fixtures::{builtin_sets, load_dir};
use no_llm_api::model::ChatCompletionRequest;
use no_llm_api::models::ModelCatalogue;
use no_llm_api::sim::artifact::SemanticArtifact;
use no_llm_api::sim::canonical::CanonicalRequest;
use no_llm_api::sim::plan::{SemanticCapabilities, SemanticResponsePlan, compile, explain};
use no_llm_api::sim::script::{builtin_fixtures, load_dir as load_semantic_dir};
use tiktoken_rs::cl100k_base;

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
    /// Check schema-v2 semantic fixtures without compiling legacy parquet.
    LintSemantic {
        #[arg(long, default_value = "fixtures/v2")]
        input: PathBuf,
    },
    /// Compile schema-v2 fixtures into a deterministic JSON artifact.
    BuildSemantic {
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        models: Option<PathBuf>,
        #[arg(long, default_value = "data/semantic-fixtures.json")]
        output: PathBuf,
        #[arg(long)]
        force: bool,
    },
    /// Explain fixture and variant selection for a Chat request JSON file.
    ExplainSemantic {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        models: Option<PathBuf>,
        #[arg(long)]
        case: Option<String>,
        #[arg(long)]
        variant: Option<String>,
    },
    /// Print the complete deterministic semantic plan for a Chat request.
    SnapshotSemantic {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long)]
        models: Option<PathBuf>,
        #[arg(long)]
        case: Option<String>,
        #[arg(long)]
        variant: Option<String>,
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
        Command::LintSemantic { input } => {
            let fixtures = if input == PathBuf::from("fixtures/v2") {
                builtin_fixtures()
            } else {
                load_semantic_dir(&input)?
            };
            for fixture in &fixtures {
                println!("ok {} ({} cases)", fixture.id, fixture.cases.len());
            }
            println!("{} semantic fixtures pass", fixtures.len());
        }
        Command::BuildSemantic {
            input,
            models,
            output,
            force,
        } => {
            if output.exists() && !force {
                anyhow::bail!(
                    "{} already exists; pass --force to overwrite it",
                    output.display()
                );
            }
            let fixtures = semantic_fixtures(input)?;
            let models = semantic_models(models)?;
            let artifact = SemanticArtifact::compile(&fixtures, &models, &cl100k_base()?)?;
            if let Some(parent) = output.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::write(&output, artifact.to_bytes())
                .with_context(|| format!("writing {}", output.display()))?;
            println!(
                "wrote {} fixtures and {} variants to {} (digest {})",
                artifact.fixtures.len(),
                artifact.variants.len(),
                output.display(),
                artifact.digest()
            );
        }
        Command::ExplainSemantic {
            request,
            input,
            models,
            case,
            variant,
        } => {
            let plan = semantic_plan(&request, input, models, case, variant)?;
            println!("{}", serde_json::to_string_pretty(&explain(&plan))?);
        }
        Command::SnapshotSemantic {
            request,
            input,
            models,
            case,
            variant,
        } => {
            let plan = semantic_plan(&request, input, models, case, variant)?;
            println!("{}", serde_json::to_string_pretty(&plan)?);
        }
    }
    Ok(())
}

fn semantic_fixtures(
    input: Option<PathBuf>,
) -> Result<Vec<no_llm_api::sim::script::SemanticFixture>> {
    match input {
        Some(path) => Ok(load_semantic_dir(&path)?),
        None => Ok(builtin_fixtures()),
    }
}

fn semantic_models(path: Option<PathBuf>) -> Result<ModelCatalogue> {
    match path {
        Some(path) => ModelCatalogue::from_path(&path)
            .with_context(|| format!("loading model catalogue {}", path.display())),
        None => Ok(ModelCatalogue::builtin()),
    }
}

fn semantic_plan(
    request_path: &PathBuf,
    input: Option<PathBuf>,
    models: Option<PathBuf>,
    case: Option<String>,
    variant: Option<String>,
) -> Result<SemanticResponsePlan> {
    let request_text = std::fs::read_to_string(request_path)
        .with_context(|| format!("reading {}", request_path.display()))?;
    let request: ChatCompletionRequest = serde_json::from_str(&request_text)
        .with_context(|| format!("parsing {}", request_path.display()))?;
    let fixtures = semantic_fixtures(input)?;
    let models = semantic_models(models)?;
    let profile = models
        .profile(&request.model)
        .with_context(|| format!("model '{}' is not in the catalogue", request.model))?;
    let artifact = SemanticArtifact::compile(&fixtures, &models, &cl100k_base()?)?;
    let controls = artifact.selection_controls(case, variant);
    Ok(compile(
        &fixtures,
        &CanonicalRequest::from_chat(&request),
        &controls,
        &SemanticCapabilities::from(profile),
    )?)
}

fn sets_from(input: Option<PathBuf>) -> Result<Vec<no_llm_api::fixtures::FixtureSet>> {
    match input {
        Some(path) => Ok(load_dir(&path)?),
        None => Ok(builtin_sets()),
    }
}
