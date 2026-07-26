//! Deterministic compilation of semantic fixtures into immutable response plans.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sim::canonical::{CanonicalRequest, canonical_json};
use crate::sim::digest::{digest_fields, pick};
use crate::sim::script::{
    Interface, OutcomeVariant, SemanticCase, SemanticFixture, StructuredOutput, TerminalStatus,
};

pub const PLAN_VERSION: &str = "semantic-plan-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectionControls {
    pub case: Option<String>,
    pub variant: Option<String>,
    pub dataset_revision: String,
    pub scenario_revision: String,
    pub model_profile_revision: String,
    pub simulation_seed: Option<u64>,
}

impl Default for SelectionControls {
    fn default() -> Self {
        Self {
            case: None,
            variant: None,
            dataset_revision: "semantic-builtins-v1".to_string(),
            scenario_revision: "default-v1".to_string(),
            model_profile_revision: "builtin-v1".to_string(),
            simulation_seed: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SemanticCapabilities {
    pub reasoning_efforts: BTreeSet<String>,
    pub structured_output: bool,
}

impl From<&crate::models::ModelProfile> for SemanticCapabilities {
    fn from(profile: &crate::models::ModelProfile) -> Self {
        Self {
            reasoning_efforts: profile
                .capabilities
                .reasoning_efforts
                .iter()
                .cloned()
                .collect(),
            structured_output: profile.capabilities.structured_output,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticResponsePlan {
    pub version: String,
    pub fixture_id: String,
    pub case_id: String,
    pub variant_id: String,
    pub interface: Interface,
    pub model: String,
    pub output: Vec<SemanticOutput>,
    pub terminal: TerminalStatus,
    pub usage: SemanticUsage,
    pub plan_digest: String,
    pub explanation: PlanExplanation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SemanticOutput {
    Reasoning {
        summary: Option<String>,
        trace: Option<String>,
        encrypted: Option<String>,
    },
    Text {
        text: String,
    },
    Refusal {
        text: String,
    },
    Structured {
        json: String,
        value: Value,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticUsage {
    pub reasoning_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanExplanation {
    pub match_kind: SemanticMatchKind,
    pub fixture_id: String,
    pub case_id: String,
    pub variant_id: String,
    pub variant_reason: VariantReason,
    pub canonical_request_digest: String,
    pub turn_shapes: Vec<RedactedTurn>,
    pub effective_controls: EffectiveControls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveControls {
    pub dataset_revision: String,
    pub scenario_revision: String,
    pub model_profile_revision: String,
    pub simulation_seed: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticMatchKind {
    Explicit,
    Exact,
    DigestFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariantReason {
    Explicit,
    ExactEffort,
    DeclaredFallback,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedTurn {
    pub role: String,
    pub content_kind: String,
    pub content_bytes: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("semantic case '{0}' was not found")]
    UnknownCase(String),
    #[error("semantic variant '{variant}' was not found in case '{case}'")]
    UnknownVariant { case: String, variant: String },
    #[error("semantic case '{0}' is incompatible with the request")]
    IncompatibleCase(String),
    #[error("semantic variant '{variant}' is incompatible with effort '{effort}'")]
    IncompatibleVariant { variant: String, effort: String },
    #[error("model '{model}' does not support reasoning effort '{effort}'")]
    UnsupportedEffort { model: String, effort: String },
    #[error("model '{0}' does not support structured output")]
    UnsupportedStructuredOutput(String),
    #[error("no semantic fixture matches the request")]
    NoMatch,
    #[error("semantic request matches multiple equal-priority cases: {0}")]
    AmbiguousMatch(String),
    #[error("case '{case}' has no variant for effort '{effort}' and no fallback")]
    MissingEffortVariant { case: String, effort: String },
    #[error("structured fixture bytes are invalid JSON in {fixture}/{case}/{variant}")]
    InvalidStructuredOutput {
        fixture: String,
        case: String,
        variant: String,
    },
}

pub fn compile(
    fixtures: &[SemanticFixture],
    request: &CanonicalRequest,
    controls: &SelectionControls,
    capabilities: &SemanticCapabilities,
) -> Result<SemanticResponsePlan, PlanError> {
    if let Some(effort) = &request.reasoning_effort
        && !capabilities.reasoning_efforts.contains(effort)
    {
        return Err(PlanError::UnsupportedEffort {
            model: request.model.clone(),
            effort: effort.clone(),
        });
    }

    let (fixture, case, match_kind) = select_case(fixtures, request, controls)?;
    if fixture.requirements.structured_output && !capabilities.structured_output {
        return Err(PlanError::UnsupportedStructuredOutput(
            request.model.clone(),
        ));
    }
    let (variant_id, variant, variant_reason) = resolve_variant(case, request, controls)?;
    let output = output_nodes(fixture, case, variant_id, variant)?;
    let usage = SemanticUsage {
        reasoning_tokens: variant.reasoning_tokens.unwrap_or(0),
    };
    let body = PlanBody {
        version: PLAN_VERSION,
        fixture_id: &fixture.id,
        case_id: &case.id,
        variant_id,
        interface: request.interface,
        model: &request.model,
        output: &output,
        terminal: case.terminal,
        usage: &usage,
        controls,
    };
    let body_json = canonical_json(&serde_json::to_value(&body).expect("plan body serializes"));
    let digest = digest_fields([
        PLAN_VERSION,
        request.canonical_json().as_str(),
        body_json.as_str(),
    ]);
    let plan_digest = format!("{digest:016x}");
    let explanation = PlanExplanation {
        match_kind,
        fixture_id: fixture.id.clone(),
        case_id: case.id.clone(),
        variant_id: variant_id.to_string(),
        variant_reason,
        canonical_request_digest: format!("{:016x}", request.digest()),
        turn_shapes: redact_turns(request),
        effective_controls: EffectiveControls {
            dataset_revision: controls.dataset_revision.clone(),
            scenario_revision: controls.scenario_revision.clone(),
            model_profile_revision: controls.model_profile_revision.clone(),
            simulation_seed: controls.simulation_seed,
        },
    };

    Ok(SemanticResponsePlan {
        version: PLAN_VERSION.to_string(),
        fixture_id: fixture.id.clone(),
        case_id: case.id.clone(),
        variant_id: variant_id.to_string(),
        interface: request.interface,
        model: request.model.clone(),
        output,
        terminal: case.terminal,
        usage,
        plan_digest,
        explanation,
    })
}

#[derive(Serialize)]
struct PlanBody<'a> {
    version: &'a str,
    fixture_id: &'a str,
    case_id: &'a str,
    variant_id: &'a str,
    interface: Interface,
    model: &'a str,
    output: &'a [SemanticOutput],
    terminal: TerminalStatus,
    usage: &'a SemanticUsage,
    controls: &'a SelectionControls,
}

fn select_case<'a>(
    fixtures: &'a [SemanticFixture],
    request: &CanonicalRequest,
    controls: &SelectionControls,
) -> Result<(&'a SemanticFixture, &'a SemanticCase, SemanticMatchKind), PlanError> {
    if let Some(wanted) = &controls.case {
        let mut parts = wanted.split('/');
        let fixture_id = parts.next().unwrap_or_default();
        let case_id = parts.next();
        if parts.next().is_some() {
            return Err(PlanError::UnknownCase(wanted.clone()));
        }
        for fixture in fixtures {
            if fixture.id != fixture_id {
                continue;
            }
            let case = match case_id {
                Some(case_id) => fixture.cases.iter().find(|case| case.id == case_id),
                None if fixture.cases.len() == 1 => fixture.cases.first(),
                None => None,
            };
            let case = case.ok_or_else(|| PlanError::UnknownCase(wanted.clone()))?;
            if !fixture_compatible(fixture, request) || !case_matches(case, request) {
                return Err(PlanError::IncompatibleCase(wanted.clone()));
            }
            return Ok((fixture, case, SemanticMatchKind::Explicit));
        }
        return Err(PlanError::UnknownCase(wanted.clone()));
    }

    let mut exact: Vec<_> = fixtures
        .iter()
        .filter(|fixture| fixture_matches(fixture, request))
        .flat_map(|fixture| {
            fixture
                .cases
                .iter()
                .filter(|case| case_matches(case, request))
                .map(move |case| (fixture, case))
        })
        .collect();
    exact.sort_by(|(left_fixture, left_case), (right_fixture, right_case)| {
        (&left_fixture.id, &left_case.id).cmp(&(&right_fixture.id, &right_case.id))
    });
    match exact.as_slice() {
        [(fixture, case)] => Ok((fixture, case, SemanticMatchKind::Exact)),
        [] => {
            let digest = request.digest();
            let mut compatible: Vec<_> = fixtures
                .iter()
                .filter(|fixture| fixture_compatible(fixture, request))
                .flat_map(|fixture| {
                    fixture
                        .cases
                        .iter()
                        .filter(|case| case_matches(case, request))
                        .map(move |case| (fixture, case))
                })
                .collect();
            compatible.sort_by(|(left_fixture, left_case), (right_fixture, right_case)| {
                (&left_fixture.id, &left_case.id).cmp(&(&right_fixture.id, &right_case.id))
            });
            let (fixture, case) = compatible
                .get(pick(digest, compatible.len()))
                .copied()
                .ok_or(PlanError::NoMatch)?;
            Ok((fixture, case, SemanticMatchKind::DigestFallback))
        }
        matches => Err(PlanError::AmbiguousMatch(
            matches
                .iter()
                .map(|(fixture, case)| format!("{}/{}", fixture.id, case.id))
                .collect::<Vec<_>>()
                .join(", "),
        )),
    }
}

fn fixture_matches(fixture: &SemanticFixture, request: &CanonicalRequest) -> bool {
    if !fixture_compatible(fixture, request) {
        return false;
    }
    let Some(turns) = request.match_turns() else {
        return false;
    };
    turns.len() == fixture.match_spec.turns.len()
        && turns.iter().zip(&fixture.match_spec.turns).all(
            |((request_role, request_text), fixture_turn)| {
                request_role == &fixture_turn.role && request_text == &fixture_turn.text
            },
        )
}

fn fixture_compatible(fixture: &SemanticFixture, request: &CanonicalRequest) -> bool {
    fixture.interfaces.contains(&request.interface)
        && (fixture.match_spec.models.is_empty()
            || fixture.match_spec.models.contains(&request.model))
}

fn case_matches(case: &SemanticCase, request: &CanonicalRequest) -> bool {
    (case.constraints.interfaces.is_empty()
        || case.constraints.interfaces.contains(&request.interface))
        && (case.constraints.efforts.is_empty()
            || request
                .reasoning_effort
                .as_ref()
                .is_none_or(|effort| case.constraints.efforts.contains(effort)))
        && case
            .constraints
            .response_format
            .as_ref()
            .is_none_or(|name| response_format_name(request) == Some(name.as_str()))
}

fn response_format_name(request: &CanonicalRequest) -> Option<&str> {
    let format = request.response_format.as_ref()?;
    if format["type"] == "json_schema" {
        format["json_schema"]["name"]
            .as_str()
            .or_else(|| format["name"].as_str())
    } else {
        format["type"].as_str()
    }
}

fn resolve_variant<'a>(
    case: &'a SemanticCase,
    request: &CanonicalRequest,
    controls: &SelectionControls,
) -> Result<(&'a str, &'a OutcomeVariant, VariantReason), PlanError> {
    if let Some(explicit) = &controls.variant {
        if let Some(effort) = &request.reasoning_effort
            && case.constraints.efforts.contains(explicit)
            && explicit != effort
        {
            return Err(PlanError::IncompatibleVariant {
                variant: explicit.clone(),
                effort: effort.clone(),
            });
        }
        return case
            .variants
            .get_key_value(explicit)
            .map(|(id, variant)| (id.as_str(), variant, VariantReason::Explicit))
            .ok_or_else(|| PlanError::UnknownVariant {
                case: case.id.clone(),
                variant: explicit.clone(),
            });
    }
    if let Some(effort) = &request.reasoning_effort {
        if let Some((id, variant)) = case.variants.get_key_value(effort) {
            return Ok((id, variant, VariantReason::ExactEffort));
        }
        if let Some(fallback) = &case.fallback_variant {
            let variant = case
                .variants
                .get(fallback)
                .expect("lint guarantees the fallback variant");
            return Ok((fallback, variant, VariantReason::DeclaredFallback));
        }
        return Err(PlanError::MissingEffortVariant {
            case: case.id.clone(),
            effort: effort.clone(),
        });
    }
    let variant = case
        .variants
        .get(&case.default_variant)
        .expect("lint guarantees the default variant");
    Ok((&case.default_variant, variant, VariantReason::Default))
}

pub(crate) fn output_nodes(
    fixture: &SemanticFixture,
    case: &SemanticCase,
    variant_id: &str,
    variant: &OutcomeVariant,
) -> Result<Vec<SemanticOutput>, PlanError> {
    let mut output = Vec::new();
    if variant.reasoning_summary.is_some()
        || variant.reasoning_trace.is_some()
        || variant.reasoning_encrypted.is_some()
    {
        output.push(SemanticOutput::Reasoning {
            summary: variant.reasoning_summary.clone(),
            trace: variant.reasoning_trace.clone(),
            encrypted: variant.reasoning_encrypted.clone(),
        });
    }
    if let Some(text) = &variant.answer {
        output.push(SemanticOutput::Text { text: text.clone() });
    } else if let Some(text) = &variant.refusal {
        output.push(SemanticOutput::Refusal { text: text.clone() });
    } else if let Some(StructuredOutput { json }) = &variant.structured_output {
        let value = serde_json::from_str(json).map_err(|_| PlanError::InvalidStructuredOutput {
            fixture: fixture.id.clone(),
            case: case.id.clone(),
            variant: variant_id.to_string(),
        })?;
        output.push(SemanticOutput::Structured {
            json: json.clone(),
            value,
        });
    }
    Ok(output)
}

fn redact_turns(request: &CanonicalRequest) -> Vec<RedactedTurn> {
    request
        .turns
        .iter()
        .map(|turn| {
            let (content_kind, content_bytes) = match &turn.content {
                crate::sim::canonical::CanonicalContent::Empty => ("empty", 0),
                crate::sim::canonical::CanonicalContent::Text(text) => ("text", text.len()),
                crate::sim::canonical::CanonicalContent::Parts(parts) => {
                    ("parts", canonical_json(parts).len())
                }
            };
            RedactedTurn {
                role: turn.role.clone(),
                content_kind: content_kind.to_string(),
                content_bytes,
            }
        })
        .collect()
}

pub fn explain(plan: &SemanticResponsePlan) -> Value {
    serde_json::to_value(&plan.explanation).expect("plan explanation serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ChatCompletionRequest, ChatCompletionRequestMessage, ChatRole, MessageContent,
    };
    use crate::request_types::{JsonSchemaFormat, ReasoningEffort, ResponseFormat};
    use crate::sim::canonical::CanonicalRequest;
    use crate::sim::script::builtin_fixtures;

    fn reasoning_request(effort: ReasoningEffort) -> CanonicalRequest {
        CanonicalRequest::from_chat(&ChatCompletionRequest {
            model: "mock-reasoner".to_string(),
            messages: vec![ChatCompletionRequestMessage {
                role: ChatRole::User,
                content: Some(MessageContent::Text(
                    "Which release should ship?".to_string(),
                )),
                name: None,
                tool_call_id: None,
                tool_calls: None,
                function_call: None,
                audio: None,
                refusal: None,
            }],
            reasoning_effort: Some(effort),
            ..Default::default()
        })
    }

    fn capabilities() -> SemanticCapabilities {
        SemanticCapabilities {
            reasoning_efforts: ["low", "medium", "high"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            structured_output: true,
        }
    }

    #[test]
    fn exact_effort_resolves_an_authored_variant() {
        let plan = compile(
            &builtin_fixtures(),
            &reasoning_request(ReasoningEffort::High),
            &SelectionControls::default(),
            &capabilities(),
        )
        .unwrap();
        assert_eq!(plan.fixture_id, "reasoning-effort");
        assert_eq!(plan.variant_id, "high");
        assert_eq!(plan.usage.reasoning_tokens, 28);
        assert_eq!(plan.explanation.variant_reason, VariantReason::ExactEffort);
        assert_eq!(plan.output[0].type_name(), "reasoning");
        assert_eq!(plan.output[1].type_name(), "text");
    }

    #[test]
    fn unsupported_effort_fails_before_selection() {
        let error = compile(
            &builtin_fixtures(),
            &reasoning_request(ReasoningEffort::Xhigh),
            &SelectionControls::default(),
            &capabilities(),
        )
        .unwrap_err();
        assert!(matches!(error, PlanError::UnsupportedEffort { .. }));
    }

    #[test]
    fn explicit_selection_is_stable_and_unknown_ids_fail() {
        let controls = SelectionControls {
            case: Some("reasoning-effort/release-decision".to_string()),
            variant: Some("low".to_string()),
            ..Default::default()
        };
        let first = compile(
            &builtin_fixtures(),
            &reasoning_request(ReasoningEffort::Low),
            &controls,
            &capabilities(),
        )
        .unwrap();
        let second = compile(
            &builtin_fixtures(),
            &reasoning_request(ReasoningEffort::Low),
            &controls,
            &capabilities(),
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.variant_id, "low");
        assert_eq!(first.explanation.match_kind, SemanticMatchKind::Explicit);

        let incompatible = compile(
            &builtin_fixtures(),
            &reasoning_request(ReasoningEffort::High),
            &controls,
            &capabilities(),
        )
        .unwrap_err();
        assert!(matches!(
            incompatible,
            PlanError::IncompatibleVariant { .. }
        ));

        let error = compile(
            &builtin_fixtures(),
            &reasoning_request(ReasoningEffort::High),
            &SelectionControls {
                case: Some("missing".to_string()),
                variant: None,
                ..Default::default()
            },
            &capabilities(),
        )
        .unwrap_err();
        assert!(matches!(error, PlanError::UnknownCase(_)));
    }

    #[test]
    fn explain_output_contains_no_prompt_text() {
        let plan = compile(
            &builtin_fixtures(),
            &reasoning_request(ReasoningEffort::Low),
            &SelectionControls::default(),
            &capabilities(),
        )
        .unwrap();
        let encoded = explain(&plan).to_string();
        assert!(!encoded.contains("Which release should ship?"));
        assert!(encoded.contains("content_bytes"));
        assert!(encoded.contains(&plan.explanation.canonical_request_digest));
    }

    #[test]
    fn structured_output_keeps_authored_bytes_and_parsed_value() {
        let request = CanonicalRequest::from_chat(&ChatCompletionRequest {
            model: "mock-gpt-4o".to_string(),
            messages: vec![ChatCompletionRequestMessage {
                role: ChatRole::User,
                content: Some(MessageContent::Text("Report release status.".to_string())),
                name: None,
                tool_call_id: None,
                tool_calls: None,
                function_call: None,
                audio: None,
                refusal: None,
            }],
            response_format: Some(ResponseFormat::JsonSchema {
                json_schema: JsonSchemaFormat {
                    name: "release-status".to_string(),
                    description: None,
                    schema: None,
                    strict: Some(true),
                },
            }),
            ..Default::default()
        });
        let plan = compile(
            &builtin_fixtures(),
            &request,
            &SelectionControls::default(),
            &capabilities(),
        )
        .unwrap();
        let SemanticOutput::Structured { json, value } = &plan.output[0] else {
            panic!("expected structured output")
        };
        assert_eq!(json, "{\"status\":\"green\",\"blockers\":0}");
        assert_eq!(
            value,
            &serde_json::json!({"status": "green", "blockers": 0})
        );
    }

    #[test]
    fn simulation_context_participates_in_plan_identity() {
        let request = reasoning_request(ReasoningEffort::Medium);
        let baseline = compile(
            &builtin_fixtures(),
            &request,
            &SelectionControls::default(),
            &capabilities(),
        )
        .unwrap();
        let changed = compile(
            &builtin_fixtures(),
            &request,
            &SelectionControls {
                scenario_revision: "slow-v2".to_string(),
                ..Default::default()
            },
            &capabilities(),
        )
        .unwrap();
        assert_ne!(baseline.plan_digest, changed.plan_digest);
        assert_eq!(
            changed.explanation.effective_controls.scenario_revision,
            "slow-v2"
        );
    }

    #[test]
    fn fallback_is_independent_of_fixture_enumeration() {
        let mut fixtures = builtin_fixtures();
        let basic = fixtures
            .iter()
            .find(|fixture| fixture.id == "basic-text")
            .unwrap()
            .clone();
        let mut second = basic.clone();
        second.id = "second-text".to_string();
        fixtures = vec![second, basic];

        let request = CanonicalRequest::from_chat(&ChatCompletionRequest {
            model: "mock-gpt-4o".to_string(),
            messages: vec![
                ChatCompletionRequestMessage {
                    role: ChatRole::Developer,
                    content: Some(MessageContent::Text("Different instruction.".to_string())),
                    name: None,
                    tool_call_id: None,
                    tool_calls: None,
                    function_call: None,
                    audio: None,
                    refusal: None,
                },
                ChatCompletionRequestMessage {
                    role: ChatRole::User,
                    content: Some(MessageContent::Text("Unknown prompt.".to_string())),
                    name: None,
                    tool_call_id: None,
                    tool_calls: None,
                    function_call: None,
                    audio: None,
                    refusal: None,
                },
            ],
            ..Default::default()
        });
        let first = compile(
            &fixtures,
            &request,
            &SelectionControls::default(),
            &capabilities(),
        )
        .unwrap();
        fixtures.reverse();
        let second = compile(
            &fixtures,
            &request,
            &SelectionControls::default(),
            &capabilities(),
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.explanation.match_kind,
            SemanticMatchKind::DigestFallback
        );
    }

    impl SemanticOutput {
        fn type_name(&self) -> &'static str {
            match self {
                SemanticOutput::Reasoning { .. } => "reasoning",
                SemanticOutput::Text { .. } => "text",
                SemanticOutput::Refusal { .. } => "refusal",
                SemanticOutput::Structured { .. } => "structured",
            }
        }
    }
}
