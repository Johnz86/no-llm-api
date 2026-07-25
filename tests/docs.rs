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
fn the_readme_does_not_promise_the_removed_compat_env_flag() {
    assert!(
        !readme().contains("ASYNC_OPENAI_COMPAT"),
        "the async-openai contract test no longer needs an env flag"
    );
}
