//! Documentation drift: a setting that is not documented does not exist.

use clap::CommandFactory;
use no_llm_api::config::Cli;

fn readme() -> String {
    std::fs::read_to_string("README.md").expect("README.md")
}

#[test]
fn every_environment_variable_is_documented_in_the_readme() {
    let readme = readme();
    let command = Cli::command();
    let mut missing = Vec::new();
    for arg in command.get_arguments() {
        if let Some(env) = arg.get_env().and_then(|value| value.to_str())
            && !readme.contains(env)
        {
            missing.push(env.to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "undocumented environment variables: {missing:?}"
    );
}

#[test]
fn every_flag_is_documented_in_the_readme() {
    let readme = readme();
    let command = Cli::command();
    let mut missing = Vec::new();
    for arg in command.get_arguments() {
        if let Some(long) = arg.get_long() {
            let flag = format!("--{long}");
            if !readme.contains(&flag) {
                missing.push(flag);
            }
        }
    }
    assert!(missing.is_empty(), "undocumented flags: {missing:?}");
}

#[test]
fn every_builtin_scenario_is_documented_in_the_readme() {
    let readme = readme();
    let missing: Vec<&str> = no_llm_api::sim::scenario::Scenario::builtin_names()
        .into_iter()
        .filter(|name| !readme.contains(*name))
        .collect();
    assert!(missing.is_empty(), "undocumented scenarios: {missing:?}");
}

#[test]
fn every_workflow_is_valid_yaml_with_jobs() {
    let dir = std::path::Path::new(".github/workflows");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).expect("workflows directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("yml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("read workflow");
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)
            .unwrap_or_else(|error| panic!("{} is not valid YAML: {error}", path.display()));
        assert!(
            value.get("jobs").is_some(),
            "{} declares no jobs",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 2, "expected a CI and a release workflow");
}

#[test]
fn the_changelog_calls_out_wire_behaviour_separately() {
    let changelog = std::fs::read_to_string("CHANGELOG.md").expect("CHANGELOG.md");
    assert!(
        changelog.contains("### Wire behaviour"),
        "a wire-behaviour section is the point of this changelog"
    );
    assert!(changelog.contains("docs/versioning.md"));
}

#[test]
fn the_changelog_documents_the_current_version() {
    let changelog = std::fs::read_to_string("CHANGELOG.md").expect("CHANGELOG.md");
    let heading = format!("## [{}]", env!("CARGO_PKG_VERSION"));
    assert!(
        changelog.contains(&heading),
        "CHANGELOG.md has no entry for {}",
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn the_readme_does_not_promise_the_removed_compat_env_flag() {
    assert!(
        !readme().contains("ASYNC_OPENAI_COMPAT"),
        "the async-openai contract test no longer needs an env flag"
    );
}
