//! Deterministic compilation of schema-v2 fixtures into a portable JSON artifact.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use tiktoken_rs::CoreBPE;

use crate::models::ModelCatalogue;
use crate::sim::canonical::canonical_json;
use crate::sim::digest::digest_fields;
use crate::sim::plan::{SelectionControls, SemanticCapabilities, SemanticOutput, output_nodes};
use crate::sim::script::{
    Interface, SCHEMA_VERSION, SemanticCase, SemanticFixture, SemanticFixtureError, SemanticRole,
};

pub const COMPILER_VERSION: &str = "semantic-compiler-v2";
pub const SELECTION_VERSION: &str = "semantic-selection-v1";
pub const TOKENIZER_REVISION: &str = "cl100k_base@tiktoken-rs-0.12.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticArtifact {
    pub schema_version: u16,
    pub compiler_version: String,
    pub selection_version: String,
    pub tokenizer_revision: String,
    pub source_digest: String,
    pub fixtures: Vec<SemanticFixture>,
    pub variants: Vec<CompiledVariant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledVariant {
    pub selector: String,
    pub interfaces: Vec<Interface>,
    pub output: Vec<SemanticOutput>,
    pub reasoning_tokens: u32,
    pub visible_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompatibilityReport {
    pub compatible: bool,
    pub baseline_digest: String,
    pub candidate_digest: String,
    pub fallback_assignment_changes: Vec<String>,
    pub fallback_candidate_set_changes: Vec<FallbackCandidateSetChange>,
    pub changed_variant_bytes: Vec<String>,
    pub removed_variants: Vec<String>,
    pub added_variants: Vec<String>,
    pub legacy_byte_changes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FallbackCandidateSetChange {
    pub added_case: String,
    pub affected_baseline_case: String,
    pub fallback_class: FallbackClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FallbackClass {
    pub interfaces: Vec<Interface>,
    pub any_model: bool,
    pub models: Vec<String>,
    pub reasoning_effort: Option<String>,
    pub response_format: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error(transparent)]
    Fixture(#[from] SemanticFixtureError),
    #[error("semantic fixture id '{0}' is duplicated")]
    DuplicateFixture(String),
    #[error("semantic fixture '{fixture}' references unknown model '{model}'")]
    UnknownModel { fixture: String, model: String },
    #[error("model '{model}' does not support reasoning required by '{fixture}'")]
    UnsupportedReasoning { fixture: String, model: String },
    #[error("model '{model}' does not support effort '{effort}' required by '{fixture}/{case}'")]
    UnsupportedEffort {
        fixture: String,
        case: String,
        model: String,
        effort: String,
    },
    #[error("model '{model}' does not support structured output required by '{fixture}'")]
    UnsupportedStructuredOutput { fixture: String, model: String },
    #[error("ambiguous equal-priority semantic cases: {left} and {right}")]
    AmbiguousCases { left: String, right: String },
    #[error("semantic variant could not compile: {0}")]
    Variant(String),
}

impl SemanticArtifact {
    pub fn compile(
        fixtures: &[SemanticFixture],
        models: &ModelCatalogue,
        tokenizer: &CoreBPE,
    ) -> Result<Self, ArtifactError> {
        let fixtures = normalized_fixtures(fixtures)?;
        validate_models(&fixtures, models)?;
        reject_ambiguity(&fixtures)?;

        let source_json = canonical_json(
            &serde_json::to_value(&fixtures).expect("semantic fixtures are serializable"),
        );
        let source_digest = format!(
            "{:016x}",
            digest_fields(["semantic-source-v1", source_json.as_str()])
        );
        let variants = compile_variants(&fixtures, tokenizer)?;
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            compiler_version: COMPILER_VERSION.to_string(),
            selection_version: SELECTION_VERSION.to_string(),
            tokenizer_revision: TOKENIZER_REVISION.to_string(),
            source_digest,
            fixtures,
            variants,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(self).expect("semantic artifact is serializable");
        bytes.push(b'\n');
        bytes
    }

    pub fn digest(&self) -> String {
        let bytes = self.to_bytes();
        format!(
            "{:016x}",
            crate::sim::digest::Digest::new().field(&bytes).finish()
        )
    }

    pub fn selection_controls(
        &self,
        case: Option<String>,
        variant: Option<String>,
    ) -> SelectionControls {
        SelectionControls {
            case,
            variant,
            dataset_revision: self.source_digest.clone(),
            ..Default::default()
        }
    }

    pub fn compatibility_report(&self, candidate: &Self) -> CompatibilityReport {
        let baseline_fallbacks = fallback_assignments(&self.fixtures);
        let candidate_fallbacks = fallback_assignments(&candidate.fixtures);
        let fallback_assignment_changes =
            changed_map_keys(&baseline_fallbacks, &candidate_fallbacks);
        let fallback_candidate_set_changes =
            fallback_candidate_set_changes(&self.fixtures, &candidate.fixtures);
        let baseline_variants = variant_bytes(self);
        let candidate_variants = variant_bytes(candidate);
        let removed_variants = baseline_variants
            .keys()
            .filter(|key| !candidate_variants.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        let added_variants = candidate_variants
            .keys()
            .filter(|key| !baseline_variants.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        let changed_variant_bytes = changed_map_keys(&baseline_variants, &candidate_variants);
        let legacy_byte_changes = changed_variant_bytes
            .iter()
            .chain(removed_variants.iter())
            .filter(|selector| selector.starts_with("legacy-"))
            .cloned()
            .collect::<Vec<_>>();
        let compatible = fallback_assignment_changes.is_empty()
            && fallback_candidate_set_changes.is_empty()
            && changed_variant_bytes.is_empty()
            && removed_variants.is_empty()
            && legacy_byte_changes.is_empty();
        CompatibilityReport {
            compatible,
            baseline_digest: self.digest(),
            candidate_digest: candidate.digest(),
            fallback_assignment_changes,
            fallback_candidate_set_changes,
            changed_variant_bytes,
            removed_variants,
            added_variants,
            legacy_byte_changes,
        }
    }
}

fn fallback_candidate_set_changes(
    baseline: &[SemanticFixture],
    candidate: &[SemanticFixture],
) -> Vec<FallbackCandidateSetChange> {
    let baseline_cases = baseline
        .iter()
        .flat_map(|fixture| {
            fixture
                .cases
                .iter()
                .map(move |case| (case_selector(fixture, case), fixture, case))
        })
        .collect::<Vec<_>>();
    let baseline_selectors = baseline_cases
        .iter()
        .map(|(selector, _, _)| selector.as_str())
        .collect::<BTreeSet<_>>();
    candidate
        .iter()
        .flat_map(|fixture| {
            let baseline_selectors = &baseline_selectors;
            fixture.cases.iter().filter_map(move |case| {
                let selector = case_selector(fixture, case);
                (!baseline_selectors.contains(selector.as_str()))
                    .then_some((selector, fixture, case))
            })
        })
        .flat_map(|(added_case, candidate_fixture, candidate_case)| {
            baseline_cases
                .iter()
                .filter_map(move |(selector, fixture, case)| {
                    fallback_class(fixture, case, candidate_fixture, candidate_case).map(
                        |fallback_class| FallbackCandidateSetChange {
                            added_case: added_case.clone(),
                            affected_baseline_case: selector.clone(),
                            fallback_class,
                        },
                    )
                })
        })
        .collect()
}

fn case_selector(fixture: &SemanticFixture, case: &SemanticCase) -> String {
    format!("{}/{}", fixture.id, case.id)
}

fn fallback_class(
    left_fixture: &SemanticFixture,
    left_case: &SemanticCase,
    right_fixture: &SemanticFixture,
    right_case: &SemanticCase,
) -> Option<FallbackClass> {
    let interfaces = effective_interfaces(left_fixture, left_case)
        .intersection(&effective_interfaces(right_fixture, right_case))
        .copied()
        .collect::<Vec<_>>();
    if interfaces.is_empty() {
        return None;
    }
    let any_model =
        left_fixture.match_spec.models.is_empty() && right_fixture.match_spec.models.is_empty();
    let models = if left_fixture.match_spec.models.is_empty() {
        right_fixture.match_spec.models.clone()
    } else if right_fixture.match_spec.models.is_empty() {
        left_fixture.match_spec.models.clone()
    } else {
        left_fixture
            .match_spec
            .models
            .iter()
            .filter(|model| right_fixture.match_spec.models.contains(*model))
            .cloned()
            .collect()
    };
    if !any_model && models.is_empty() {
        return None;
    }
    let response_format = match (
        left_case.constraints.response_format.as_ref(),
        right_case.constraints.response_format.as_ref(),
    ) {
        (Some(left), Some(right)) if left != right => return None,
        (Some(value), _) | (_, Some(value)) => Some(value.clone()),
        (None, None) => None,
    };
    Some(FallbackClass {
        interfaces,
        any_model,
        models,
        reasoning_effort: None,
        response_format,
    })
}

fn fallback_assignments(fixtures: &[SemanticFixture]) -> BTreeMap<String, String> {
    fixtures
        .iter()
        .flat_map(|fixture| {
            fixture.cases.iter().map(move |case| {
                (
                    format!("{}/{}", fixture.id, case.id),
                    canonical_json(&serde_json::json!({
                        "match_signature": match_signature(fixture, case),
                        "default": case.default_variant,
                        "fallback": case.fallback_variant,
                    })),
                )
            })
        })
        .collect()
}

fn variant_bytes(artifact: &SemanticArtifact) -> BTreeMap<String, String> {
    artifact
        .variants
        .iter()
        .map(|variant| {
            (
                variant.selector.clone(),
                canonical_json(
                    &serde_json::to_value(&variant.output).expect("compiled output serializes"),
                ),
            )
        })
        .collect()
}

fn changed_map_keys<T: PartialEq>(
    baseline: &BTreeMap<String, T>,
    candidate: &BTreeMap<String, T>,
) -> Vec<String> {
    baseline
        .iter()
        .filter(|(key, value)| {
            candidate
                .get(*key)
                .is_some_and(|candidate| candidate != *value)
        })
        .map(|(key, _)| key.clone())
        .collect()
}

pub fn builtin_artifact() -> Arc<SemanticArtifact> {
    static ARTIFACT: OnceLock<Arc<SemanticArtifact>> = OnceLock::new();
    ARTIFACT
        .get_or_init(|| {
            Arc::new(
                SemanticArtifact::compile(
                    &crate::sim::script::builtin_fixtures(),
                    &ModelCatalogue::builtin(),
                    &tiktoken_rs::cl100k_base().expect("built-in tokenizer is valid"),
                )
                .expect("embedded semantic fixtures compile"),
            )
        })
        .clone()
}

fn normalized_fixtures(
    fixtures: &[SemanticFixture],
) -> Result<Vec<SemanticFixture>, ArtifactError> {
    let mut fixtures = fixtures.to_vec();
    for fixture in &mut fixtures {
        fixture.lint()?;
        fixture.tags.sort();
        fixture.interfaces.sort();
        fixture.match_spec.models.sort();
        fixture.cases.sort_by(|left, right| left.id.cmp(&right.id));
        for case in &mut fixture.cases {
            case.constraints.interfaces.sort();
            case.constraints.efforts.sort();
        }
    }
    fixtures.sort_by(|left, right| left.id.cmp(&right.id));
    for pair in fixtures.windows(2) {
        if pair[0].id == pair[1].id {
            return Err(ArtifactError::DuplicateFixture(pair[0].id.clone()));
        }
    }
    Ok(fixtures)
}

fn validate_models(
    fixtures: &[SemanticFixture],
    models: &ModelCatalogue,
) -> Result<(), ArtifactError> {
    for fixture in fixtures {
        for model in &fixture.match_spec.models {
            let profile = models
                .profile(model)
                .ok_or_else(|| ArtifactError::UnknownModel {
                    fixture: fixture.id.clone(),
                    model: model.clone(),
                })?;
            let capabilities = SemanticCapabilities::from(profile);
            if fixture.requirements.reasoning && !profile.capabilities.reasoning {
                return Err(ArtifactError::UnsupportedReasoning {
                    fixture: fixture.id.clone(),
                    model: model.clone(),
                });
            }
            if fixture.requirements.structured_output && !capabilities.structured_output {
                return Err(ArtifactError::UnsupportedStructuredOutput {
                    fixture: fixture.id.clone(),
                    model: model.clone(),
                });
            }
            for case in &fixture.cases {
                for effort in &case.constraints.efforts {
                    if !capabilities.reasoning_efforts.contains(effort) {
                        return Err(ArtifactError::UnsupportedEffort {
                            fixture: fixture.id.clone(),
                            case: case.id.clone(),
                            model: model.clone(),
                            effort: effort.clone(),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn reject_ambiguity(fixtures: &[SemanticFixture]) -> Result<(), ArtifactError> {
    let selectors: Vec<_> = fixtures
        .iter()
        .flat_map(|fixture| fixture.cases.iter().map(move |case| (fixture, case)))
        .collect();
    for left_index in 0..selectors.len() {
        for right_index in left_index + 1..selectors.len() {
            let (left_fixture, left_case) = selectors[left_index];
            let (right_fixture, right_case) = selectors[right_index];
            if cases_overlap(left_fixture, left_case, right_fixture, right_case) {
                return Err(ArtifactError::AmbiguousCases {
                    left: format!("{}/{}", left_fixture.id, left_case.id),
                    right: format!("{}/{}", right_fixture.id, right_case.id),
                });
            }
        }
    }
    Ok(())
}

fn cases_overlap(
    left_fixture: &SemanticFixture,
    left_case: &SemanticCase,
    right_fixture: &SemanticFixture,
    right_case: &SemanticCase,
) -> bool {
    if left_fixture.match_spec.turns != right_fixture.match_spec.turns {
        return false;
    }
    overlaps(
        &effective_interfaces(left_fixture, left_case),
        &effective_interfaces(right_fixture, right_case),
    ) && wildcard_overlaps(
        &left_fixture.match_spec.models,
        &right_fixture.match_spec.models,
    ) && wildcard_overlaps(
        &left_case.constraints.efforts,
        &right_case.constraints.efforts,
    ) && (left_case.constraints.response_format.is_none()
        || right_case.constraints.response_format.is_none()
        || left_case.constraints.response_format == right_case.constraints.response_format)
}

fn effective_interfaces(fixture: &SemanticFixture, case: &SemanticCase) -> BTreeSet<Interface> {
    if case.constraints.interfaces.is_empty() {
        fixture.interfaces.iter().copied().collect()
    } else {
        case.constraints.interfaces.iter().copied().collect()
    }
}

fn overlaps<T: Ord>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> bool {
    left.intersection(right).next().is_some()
}

fn wildcard_overlaps<T: Ord + Clone>(left: &[T], right: &[T]) -> bool {
    left.is_empty()
        || right.is_empty()
        || overlaps(
            &left.iter().cloned().collect(),
            &right.iter().cloned().collect(),
        )
}

fn compile_variants(
    fixtures: &[SemanticFixture],
    tokenizer: &CoreBPE,
) -> Result<Vec<CompiledVariant>, ArtifactError> {
    let mut compiled = Vec::new();
    for fixture in fixtures {
        for case in &fixture.cases {
            let interfaces: Vec<_> = effective_interfaces(fixture, case).into_iter().collect();
            for (variant_id, variant) in &case.variants {
                let selector = format!("{}/{variant_id}", case.id);
                let output = output_nodes(fixture, case, variant_id, variant)
                    .map_err(|error| ArtifactError::Variant(error.to_string()))?;
                let visible_tokens = output
                    .iter()
                    .map(|node| match node {
                        SemanticOutput::Text { text } | SemanticOutput::Refusal { text } => {
                            tokenizer.encode_with_special_tokens(text).len() as u32
                        }
                        SemanticOutput::Structured { json, .. } => {
                            tokenizer.encode_with_special_tokens(json).len() as u32
                        }
                        SemanticOutput::Reasoning { .. } => 0,
                    })
                    .sum();
                let derived_reasoning = output.iter().find_map(|node| match node {
                    SemanticOutput::Reasoning { summary, trace, .. } => trace
                        .as_ref()
                        .or(summary.as_ref())
                        .map(|text| tokenizer.encode_with_special_tokens(text).len() as u32),
                    _ => None,
                });
                compiled.push(CompiledVariant {
                    selector: format!("{}/{}", fixture.id, selector),
                    interfaces: interfaces.clone(),
                    output,
                    reasoning_tokens: variant.reasoning_tokens.or(derived_reasoning).unwrap_or(0),
                    visible_tokens,
                });
            }
        }
    }
    compiled.sort_by(|left, right| left.selector.cmp(&right.selector));
    Ok(compiled)
}

pub fn match_signature(fixture: &SemanticFixture, case: &SemanticCase) -> String {
    #[derive(Serialize)]
    struct Signature<'a> {
        turns: Vec<(SemanticRole, &'a str)>,
        models: &'a [String],
        interfaces: Vec<Interface>,
        efforts: &'a [String],
        response_format: &'a Option<String>,
    }
    let signature = Signature {
        turns: fixture
            .match_spec
            .turns
            .iter()
            .map(|turn| (turn.role, turn.text.as_str()))
            .collect(),
        models: &fixture.match_spec.models,
        interfaces: effective_interfaces(fixture, case).into_iter().collect(),
        efforts: &case.constraints.efforts,
        response_format: &case.constraints.response_format,
    };
    canonical_json(&serde_json::to_value(signature).expect("match signature serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{builtin_rows, builtin_sets};
    use crate::model::split_reasoning;
    use crate::sim::script::builtin_fixtures;
    use tiktoken_rs::cl100k_base;

    fn artifact(fixtures: &[SemanticFixture]) -> SemanticArtifact {
        SemanticArtifact::compile(
            fixtures,
            &ModelCatalogue::builtin(),
            &cl100k_base().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn artifact_bytes_are_independent_of_source_enumeration() {
        let fixtures = builtin_fixtures();
        let expected = artifact(&fixtures);
        let mut reversed = fixtures;
        reversed.reverse();
        let actual = artifact(&reversed);
        assert_eq!(expected, actual);
        assert_eq!(expected.to_bytes(), actual.to_bytes());
        assert_eq!(expected.digest(), actual.digest());
        assert!(expected.compatibility_report(&actual).compatible);
    }

    #[test]
    fn artifact_metadata_and_variant_order_are_stable() {
        let artifact = artifact(&builtin_fixtures());
        assert_eq!(artifact.schema_version, 2);
        assert_eq!(artifact.compiler_version, COMPILER_VERSION);
        assert_eq!(artifact.selection_version, SELECTION_VERSION);
        assert_eq!(artifact.tokenizer_revision, TOKENIZER_REVISION);
        assert!(
            artifact
                .variants
                .windows(2)
                .all(|pair| pair[0].selector < pair[1].selector)
        );
        assert!(artifact.variants.iter().any(|variant| {
            variant.selector == "reasoning-effort/release-decision/high"
                && variant.reasoning_tokens == 28
        }));
    }

    #[test]
    fn representative_legacy_imports_preserve_payload_bytes() {
        let legacy = builtin_sets();
        let semantic = builtin_fixtures();

        let markdown = legacy.iter().find(|set| set.id == "conv-markdown").unwrap();
        let imported = semantic
            .iter()
            .find(|fixture| fixture.id == "legacy-markdown")
            .unwrap();
        assert_eq!(
            imported.cases[0].variants["imported"].answer,
            markdown.turns[1].content
        );

        let reasoning = legacy
            .iter()
            .find(|set| set.id == "conv-reasoning")
            .unwrap();
        let (trace, answer) = split_reasoning(reasoning.turns[1].content.as_deref().unwrap());
        let imported = semantic
            .iter()
            .find(|fixture| fixture.id == "legacy-reasoning")
            .unwrap();
        assert_eq!(
            imported.cases[0].variants["imported"].reasoning_trace,
            trace
        );
        assert_eq!(imported.cases[0].variants["imported"].answer, Some(answer));

        let structured = legacy
            .iter()
            .find(|set| set.id == "conv-json-schema")
            .unwrap();
        let imported = semantic
            .iter()
            .find(|fixture| fixture.id == "legacy-structured-output")
            .unwrap();
        assert_eq!(
            imported.cases[0].variants["imported"]
                .structured_output
                .as_ref()
                .unwrap()
                .json,
            structured.turns[1].content.as_deref().unwrap()
        );
    }

    #[test]
    fn semantic_imports_never_enter_the_legacy_dataset() {
        let rows = builtin_rows();
        assert!(
            rows.iter()
                .all(|row| !row.conversation_id.starts_with("legacy-"))
        );
    }

    #[test]
    fn overlapping_cases_fail_compilation() {
        let mut fixtures = builtin_fixtures();
        let mut duplicate = fixtures[0].clone();
        duplicate.id = "ambiguous-copy".to_string();
        fixtures.push(duplicate);
        let error = SemanticArtifact::compile(
            &fixtures,
            &ModelCatalogue::builtin(),
            &cl100k_base().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(error, ArtifactError::AmbiguousCases { .. }));
    }

    #[test]
    fn compatibility_report_allows_additions_and_detects_contract_drift() {
        let baseline = artifact(&builtin_fixtures());
        let mut added = builtin_fixtures();
        let basic = added
            .iter_mut()
            .find(|fixture| fixture.id == "basic-text")
            .unwrap();
        let additional = basic.cases[0].variants["default"].clone();
        basic.cases[0]
            .variants
            .insert("additional".to_string(), additional);
        let candidate = artifact(&added);
        let report = baseline.compatibility_report(&candidate);
        assert!(report.compatible);
        assert_eq!(report.added_variants, ["basic-text/concise/additional"]);

        let mut changed = builtin_fixtures();
        changed
            .iter_mut()
            .find(|fixture| fixture.id == "legacy-markdown")
            .unwrap()
            .cases[0]
            .variants
            .get_mut("imported")
            .unwrap()
            .answer = Some("changed".to_string());
        let report = baseline.compatibility_report(&artifact(&changed));
        assert!(!report.compatible);
        assert_eq!(
            report.legacy_byte_changes,
            ["legacy-markdown/deployment-checklist/imported"]
        );
    }

    #[test]
    fn compatibility_report_rejects_fallback_candidate_set_growth() {
        let fixtures = builtin_fixtures();
        let baseline = artifact(&fixtures);
        let mut candidate_fixtures = fixtures.clone();
        let mut added = fixtures
            .iter()
            .find(|fixture| fixture.id == "basic-text")
            .unwrap()
            .clone();
        added.id = "additional-text".to_string();
        added.match_spec.turns.last_mut().unwrap().text = "An unmatched prompt.".to_string();
        candidate_fixtures.push(added);

        let report = baseline.compatibility_report(&artifact(&candidate_fixtures));

        assert!(!report.compatible);
        assert!(!report.fallback_candidate_set_changes.is_empty());
        assert!(
            report
                .fallback_candidate_set_changes
                .iter()
                .all(|change| change.added_case == "additional-text/concise")
        );
        assert!(report.fallback_candidate_set_changes.iter().any(|change| {
            change
                .fallback_class
                .interfaces
                .contains(&Interface::Responses)
        }));
        assert!(report.fallback_candidate_set_changes.iter().any(|change| {
            change.affected_baseline_case == "basic-text/concise"
                && change.fallback_class.models == ["mock-gpt-4o", "mock-gpt-4o-mini"]
        }));
    }

    #[test]
    fn compatibility_report_allows_cases_outside_existing_fallback_classes() {
        let mut baseline_fixture = builtin_fixtures()
            .into_iter()
            .find(|fixture| fixture.id == "basic-text")
            .unwrap();
        baseline_fixture.interfaces = vec![Interface::ChatCompletions];
        baseline_fixture.cases[0].constraints.interfaces = vec![Interface::ChatCompletions];
        let baseline = artifact(&[baseline_fixture.clone()]);
        let mut added = baseline_fixture.clone();
        added.id = "responses-only-text".to_string();
        added.interfaces = vec![Interface::Responses];
        added.cases[0].constraints.interfaces = vec![Interface::Responses];
        added.match_spec.turns.last_mut().unwrap().text = "An unmatched prompt.".to_string();

        let report = baseline.compatibility_report(&artifact(&[baseline_fixture, added]));

        assert!(report.compatible);
        assert!(report.fallback_candidate_set_changes.is_empty());
        assert_eq!(
            report.added_variants,
            ["responses-only-text/concise/default"]
        );
    }
}
