//! Versioned semantic fixture documents for text and reasoning simulations.
//!
//! Schema v2 is deliberately separate from the legacy conversation/parquet
//! format. Parsing and linting it has no effect on runtime selection until a
//! later compiler slice explicitly connects the two representations.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u16 = 2;

pub const BUILTINS: [(&str, &str); 12] = [
    (
        "basic-text",
        include_str!("../../fixtures/v2/basic-text.yaml"),
    ),
    (
        "reasoning-effort",
        include_str!("../../fixtures/v2/reasoning-effort.yaml"),
    ),
    (
        "multi-turn-correction",
        include_str!("../../fixtures/v2/multi-turn-correction.yaml"),
    ),
    (
        "structured-output",
        include_str!("../../fixtures/v2/structured-output.yaml"),
    ),
    (
        "legacy-markdown",
        include_str!("../../fixtures/v2/legacy-markdown.yaml"),
    ),
    (
        "legacy-reasoning",
        include_str!("../../fixtures/v2/legacy-reasoning.yaml"),
    ),
    (
        "legacy-structured-output",
        include_str!("../../fixtures/v2/legacy-structured-output.yaml"),
    ),
    (
        "response-incomplete",
        include_str!("../../fixtures/v2/response-incomplete.yaml"),
    ),
    (
        "response-failed",
        include_str!("../../fixtures/v2/response-failed.yaml"),
    ),
    (
        "response-cancelled",
        include_str!("../../fixtures/v2/response-cancelled.yaml"),
    ),
    (
        "response-refusal",
        include_str!("../../fixtures/v2/response-refusal.yaml"),
    ),
    (
        "response-continuation-parent",
        include_str!("../../fixtures/v2/response-continuation-parent.yaml"),
    ),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticFixture {
    pub schema_version: u16,
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub interfaces: Vec<Interface>,
    #[serde(rename = "match")]
    pub match_spec: MatchSpec,
    #[serde(default)]
    pub requirements: Requirements,
    pub cases: Vec<SemanticCase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interface {
    ChatCompletions,
    Responses,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchSpec {
    #[serde(default)]
    pub models: Vec<String>,
    pub turns: Vec<MatchTurn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchTurn {
    pub role: SemanticRole,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRole {
    System,
    Developer,
    User,
    Assistant,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub structured_output: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticCase {
    pub id: String,
    #[serde(default)]
    pub constraints: CaseConstraints,
    pub default_variant: String,
    #[serde(default)]
    pub fallback_variant: Option<String>,
    pub variants: BTreeMap<String, OutcomeVariant>,
    #[serde(default)]
    pub terminal: TerminalStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseConstraints {
    #[serde(default)]
    pub interfaces: Vec<Interface>,
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default)]
    pub response_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeVariant {
    #[serde(default)]
    pub reasoning_summary: Option<String>,
    #[serde(default)]
    pub reasoning_trace: Option<String>,
    #[serde(default)]
    pub reasoning_encrypted: Option<String>,
    #[serde(default)]
    pub reasoning_tokens: Option<u32>,
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub refusal: Option<String>,
    #[serde(default)]
    pub structured_output: Option<StructuredOutput>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredOutput {
    pub json: String,
    #[serde(default)]
    pub schema: Value,
    #[serde(default)]
    pub negative: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    #[default]
    Completed,
    Incomplete,
    Failed,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub enum SemanticFixtureError {
    #[error("io error reading {0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("invalid semantic fixture yaml in {0}: {1}")]
    Yaml(String, #[source] serde_yaml_ng::Error),
    #[error("semantic fixture '{fixture}' at {path} is invalid: {message}")]
    Lint {
        fixture: String,
        path: String,
        message: String,
    },
}

impl SemanticFixture {
    pub fn from_yaml(name: &str, text: &str) -> Result<Self, SemanticFixtureError> {
        reject_unstable_yaml(name, text)?;
        serde_yaml_ng::from_str(text)
            .map_err(|error| SemanticFixtureError::Yaml(name.to_string(), error))
    }

    pub fn lint(&self) -> Result<(), SemanticFixtureError> {
        if self.schema_version != SCHEMA_VERSION {
            return self.fail(
                "schema_version",
                format!("expected {SCHEMA_VERSION}, got {}", self.schema_version),
            );
        }
        lint_id(self, "id", &self.id)?;
        if self.description.trim().is_empty() {
            return self.fail("description", "must not be empty");
        }
        ensure_unique(self, "tags", &self.tags)?;
        ensure_unique(self, "interfaces", &self.interfaces)?;
        if self.interfaces.is_empty() {
            return self.fail("interfaces", "must contain at least one interface");
        }
        if self.match_spec.turns.is_empty() {
            return self.fail("match.turns", "must contain at least one turn");
        }
        if !self
            .match_spec
            .turns
            .iter()
            .any(|turn| turn.role == SemanticRole::User)
        {
            return self.fail("match.turns", "must contain a user turn");
        }
        for (index, turn) in self.match_spec.turns.iter().enumerate() {
            if turn.text.is_empty() {
                return self.fail(format!("match.turns[{index}].text"), "must not be empty");
            }
        }
        if self.cases.is_empty() {
            return self.fail("cases", "must contain at least one case");
        }

        let mut case_ids = BTreeSet::new();
        for (case_index, case) in self.cases.iter().enumerate() {
            let base = format!("cases[{case_index}]");
            lint_id(self, format!("{base}.id"), &case.id)?;
            if !case_ids.insert(&case.id) {
                return self.fail(format!("{base}.id"), "duplicates another case id");
            }
            ensure_unique(
                self,
                format!("{base}.constraints.interfaces"),
                &case.constraints.interfaces,
            )?;
            ensure_unique(
                self,
                format!("{base}.constraints.efforts"),
                &case.constraints.efforts,
            )?;
            for interface in &case.constraints.interfaces {
                if !self.interfaces.contains(interface) {
                    return self.fail(
                        format!("{base}.constraints.interfaces"),
                        "contains an interface not declared by the fixture",
                    );
                }
            }
            if case.variants.is_empty() {
                return self.fail(format!("{base}.variants"), "must not be empty");
            }
            if !case.variants.contains_key(&case.default_variant) {
                return self.fail(
                    format!("{base}.default_variant"),
                    "does not name an authored variant",
                );
            }
            if let Some(fallback) = &case.fallback_variant
                && !case.variants.contains_key(fallback)
            {
                return self.fail(
                    format!("{base}.fallback_variant"),
                    "does not name an authored variant",
                );
            }
            for (variant_id, variant) in &case.variants {
                lint_id(self, format!("{base}.variants.{variant_id}"), variant_id)?;
                lint_variant(self, &base, variant_id, variant)?;
            }
            for protected in [
                Some(case.default_variant.as_str()),
                case.fallback_variant.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if case.variants[protected]
                    .structured_output
                    .as_ref()
                    .is_some_and(|output| output.negative)
                {
                    return self.fail(
                        format!("{base}.default_variant"),
                        "negative structured variants cannot be defaults or fallbacks",
                    );
                }
            }
            let has_reasoning = case.variants.values().any(|variant| {
                variant.reasoning_summary.is_some()
                    || variant.reasoning_trace.is_some()
                    || variant.reasoning_encrypted.is_some()
                    || variant.reasoning_tokens.is_some()
            });
            if has_reasoning && !self.requirements.reasoning {
                return self.fail(
                    format!("{base}.variants"),
                    "reasoning output requires requirements.reasoning: true",
                );
            }
            let has_structured = case
                .variants
                .values()
                .any(|variant| variant.structured_output.is_some());
            if has_structured && !self.requirements.structured_output {
                return self.fail(
                    format!("{base}.variants"),
                    "structured output requires requirements.structured_output: true",
                );
            }
            if has_structured && case.constraints.response_format.is_none() {
                return self.fail(
                    format!("{base}.constraints.response_format"),
                    "is required for structured output",
                );
            }
            if !case.constraints.efforts.is_empty()
                && case.fallback_variant.is_none()
                && case
                    .constraints
                    .efforts
                    .iter()
                    .any(|effort| !case.variants.contains_key(effort))
            {
                return self.fail(
                    format!("{base}.constraints.efforts"),
                    "every effort needs an exact variant when no fallback is declared",
                );
            }
        }
        Ok(())
    }

    fn fail<T>(
        &self,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<T, SemanticFixtureError> {
        Err(SemanticFixtureError::Lint {
            fixture: self.id.clone(),
            path: path.into(),
            message: message.into(),
        })
    }
}

fn reject_unstable_yaml(name: &str, text: &str) -> Result<(), SemanticFixtureError> {
    let mut block_indent = None;
    for (index, line) in text.lines().enumerate() {
        let indentation = line.len() - line.trim_start().len();
        if let Some(parent_indent) = block_indent {
            if line.trim().is_empty() || indentation > parent_indent {
                continue;
            }
            block_indent = None;
        }

        let structural = structural_yaml(line);
        let trimmed = structural.trim();
        if matches!(
            trimmed.rsplit_once(':').map(|(_, value)| value.trim()),
            Some("|" | "|-" | "|+" | ">" | ">-" | ">+")
        ) {
            block_indent = Some(indentation);
        }
        if contains_anchor_or_alias(&structural) {
            return Err(SemanticFixtureError::Lint {
                fixture: name.to_string(),
                path: format!("line {}", index + 1),
                message: "YAML anchors and aliases are not supported".to_string(),
            });
        }
        if contains_ambiguous_date(&structural) {
            return Err(SemanticFixtureError::Lint {
                fixture: name.to_string(),
                path: format!("line {}", index + 1),
                message: "date-like scalar must be quoted explicitly".to_string(),
            });
        }
    }
    Ok(())
}

fn structural_yaml(line: &str) -> String {
    let mut result = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if escaped {
            result.push(' ');
            escaped = false;
            continue;
        }
        if double_quoted && ch == '\\' {
            result.push(' ');
            escaped = true;
            continue;
        }
        if !double_quoted && ch == '\'' {
            if single_quoted && chars.peek() == Some(&'\'') {
                result.push(' ');
                result.push(' ');
                chars.next();
                continue;
            }
            single_quoted = !single_quoted;
            result.push(' ');
            continue;
        }
        if !single_quoted && ch == '"' {
            double_quoted = !double_quoted;
            result.push(' ');
            continue;
        }
        if !single_quoted && !double_quoted && ch == '#' {
            break;
        }
        result.push(if single_quoted || double_quoted {
            ' '
        } else {
            ch
        });
    }
    result
}

fn contains_anchor_or_alias(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        if !matches!(byte, b'&' | b'*') {
            return false;
        }
        let boundary_before = index == 0
            || bytes[index - 1].is_ascii_whitespace()
            || matches!(bytes[index - 1], b':' | b'-' | b'[' | b'{' | b',');
        let name_after = bytes
            .get(index + 1)
            .is_some_and(|next| next.is_ascii_alphanumeric() || matches!(next, b'_' | b'-'));
        boundary_before && name_after
    })
}

fn contains_ambiguous_date(line: &str) -> bool {
    line.split(|character: char| {
        character.is_ascii_whitespace() || matches!(character, ':' | ',' | '[' | ']' | '{' | '}')
    })
    .any(looks_like_ambiguous_date)
}

fn looks_like_ambiguous_date(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn lint_id(
    fixture: &SemanticFixture,
    path: impl Into<String>,
    id: &str,
) -> Result<(), SemanticFixtureError> {
    let valid = !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--");
    if valid {
        Ok(())
    } else {
        fixture.fail(
            path,
            "must be canonical lower-kebab-case using ASCII letters and digits",
        )
    }
}

fn lint_variant(
    fixture: &SemanticFixture,
    base: &str,
    id: &str,
    variant: &OutcomeVariant,
) -> Result<(), SemanticFixtureError> {
    let path = format!("{base}.variants.{id}");
    let has_output = variant.reasoning_summary.is_some()
        || variant.reasoning_trace.is_some()
        || variant.reasoning_encrypted.is_some()
        || variant.reasoning_tokens.is_some()
        || variant.answer.is_some()
        || variant.refusal.is_some()
        || variant.structured_output.is_some();
    if !has_output {
        return fixture.fail(path, "must declare an output or reasoning value");
    }
    let visible_outputs = [
        variant.answer.is_some(),
        variant.refusal.is_some(),
        variant.structured_output.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if visible_outputs > 1 {
        return fixture.fail(
            path,
            "answer, refusal, and structured_output are mutually exclusive",
        );
    }
    if let Some(structured) = &variant.structured_output {
        let value = serde_json::from_str::<Value>(&structured.json).map_err(|_| {
            SemanticFixtureError::Lint {
                fixture: fixture.id.clone(),
                path: format!("{path}.structured_output.json"),
                message: "must contain valid JSON bytes".to_string(),
            }
        })?;
        if structured.negative != id.starts_with("negative-") {
            return fixture.fail(
                format!("{path}.structured_output.negative"),
                "must be true exactly for variants whose id starts with 'negative-'",
            );
        }
        lint_owned_schema(fixture, &path, &structured.schema)?;
        let validator = jsonschema::validator_for(&structured.schema).map_err(|_| {
            SemanticFixtureError::Lint {
                fixture: fixture.id.clone(),
                path: format!("{path}.structured_output.schema"),
                message: "must compile as a self-contained JSON Schema".to_string(),
            }
        })?;
        let valid = validator.is_valid(&value);
        if valid == structured.negative {
            return fixture.fail(
                format!("{path}.structured_output.json"),
                if structured.negative {
                    "negative structured output must violate its owned schema"
                } else {
                    "must satisfy its owned schema"
                },
            );
        }
    }
    Ok(())
}

fn lint_owned_schema(
    fixture: &SemanticFixture,
    path: &str,
    schema: &Value,
) -> Result<(), SemanticFixtureError> {
    const ALLOWED: &[&str] = &[
        "$defs",
        "$ref",
        "additionalProperties",
        "allOf",
        "anyOf",
        "const",
        "description",
        "enum",
        "format",
        "items",
        "maxItems",
        "maxLength",
        "maximum",
        "minItems",
        "minLength",
        "minimum",
        "not",
        "oneOf",
        "pattern",
        "properties",
        "required",
        "title",
        "type",
    ];
    let Value::Object(object) = schema else {
        return fixture.fail(
            format!("{path}.structured_output.schema"),
            "must be a JSON Schema object",
        );
    };
    for key in object.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return fixture.fail(
                format!("{path}.structured_output.schema.{key}"),
                "uses an unsupported schema keyword",
            );
        }
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str)
        && !reference.starts_with('#')
    {
        return fixture.fail(
            format!("{path}.structured_output.schema.$ref"),
            "remote schema references are not supported",
        );
    }
    for key in ["additionalProperties", "items", "not"] {
        if let Some(child) = object.get(key)
            && child.is_object()
        {
            lint_owned_schema(fixture, path, child)?;
        }
    }
    for key in ["allOf", "anyOf", "oneOf"] {
        if let Some(children) = object.get(key).and_then(Value::as_array) {
            for child in children {
                lint_owned_schema(fixture, path, child)?;
            }
        }
    }
    for key in ["$defs", "properties"] {
        if let Some(children) = object.get(key).and_then(Value::as_object) {
            for child in children.values() {
                lint_owned_schema(fixture, path, child)?;
            }
        }
    }
    Ok(())
}

fn ensure_unique<T>(
    fixture: &SemanticFixture,
    path: impl Into<String>,
    values: &[T],
) -> Result<(), SemanticFixtureError>
where
    T: Ord,
{
    let unique: BTreeSet<&T> = values.iter().collect();
    if unique.len() == values.len() {
        Ok(())
    } else {
        fixture.fail(path, "must not contain duplicate values")
    }
}

pub fn builtin_fixtures() -> Vec<SemanticFixture> {
    BUILTINS
        .iter()
        .map(|(name, body)| {
            let fixture = SemanticFixture::from_yaml(name, body)
                .unwrap_or_else(|error| panic!("built-in semantic fixture {name}: {error}"));
            fixture
                .lint()
                .unwrap_or_else(|error| panic!("built-in semantic fixture {name}: {error}"));
            fixture
        })
        .collect()
}

pub fn load_dir(path: &Path) -> Result<Vec<SemanticFixture>, SemanticFixtureError> {
    let entries = std::fs::read_dir(path)
        .map_err(|error| SemanticFixtureError::Io(path.display().to_string(), error))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| SemanticFixtureError::Io(path.display().to_string(), error))?;
        let file = entry.path();
        if file.extension().and_then(|value| value.to_str()) == Some("yaml") {
            files.push(file);
        }
    }
    files.sort();

    let mut fixtures = Vec::with_capacity(files.len());
    let mut ids = BTreeSet::new();
    for file in files {
        let name = file.display().to_string();
        let text = std::fs::read_to_string(&file)
            .map_err(|error| SemanticFixtureError::Io(name.clone(), error))?;
        let fixture = SemanticFixture::from_yaml(&name, &text)?;
        fixture.lint()?;
        if !ids.insert(fixture.id.clone()) {
            return fixture.fail("id", "duplicates another fixture id in the directory");
        }
        fixtures.push(fixture);
    }
    fixtures.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(fixtures)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
schema_version: 2
id: minimal
description: Minimal valid semantic fixture.
interfaces: [responses]
match:
  turns:
    - {role: user, text: hello}
cases:
  - id: default
    default_variant: default
    variants:
      default: {answer: hello}
"#;

    #[test]
    fn builtins_are_strict_and_valid() {
        let fixtures = builtin_fixtures();
        assert_eq!(fixtures.len(), BUILTINS.len());
        assert!(
            fixtures
                .iter()
                .any(|fixture| fixture.requirements.reasoning)
        );
        assert!(
            fixtures
                .iter()
                .any(|fixture| fixture.requirements.structured_output)
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let yaml = MINIMAL.replace("description:", "descriptino:");
        assert!(SemanticFixture::from_yaml("test", &yaml).is_err());
    }

    #[test]
    fn duplicate_mapping_keys_are_rejected() {
        let yaml = MINIMAL.replace("id: minimal", "id: minimal\nid: duplicate");
        assert!(SemanticFixture::from_yaml("test", &yaml).is_err());
    }

    #[test]
    fn fallback_must_name_an_authored_variant() {
        let yaml = MINIMAL.replace(
            "default_variant: default",
            "default_variant: default\n    fallback_variant: absent",
        );
        let fixture = SemanticFixture::from_yaml("test", &yaml).unwrap();
        let error = fixture.lint().unwrap_err().to_string();
        assert!(error.contains("fallback_variant"), "{error}");
    }

    #[test]
    fn visible_outputs_are_mutually_exclusive() {
        let yaml = MINIMAL.replace(
            "default: {answer: hello}",
            "default: {answer: hello, refusal: no}",
        );
        let fixture = SemanticFixture::from_yaml("test", &yaml).unwrap();
        let error = fixture.lint().unwrap_err().to_string();
        assert!(error.contains("mutually exclusive"), "{error}");
    }

    #[test]
    fn ids_are_already_canonical() {
        let yaml = MINIMAL.replace("id: minimal", "id: Not_Canonical");
        let fixture = SemanticFixture::from_yaml("test", &yaml).unwrap();
        let error = fixture.lint().unwrap_err().to_string();
        assert!(error.contains("lower-kebab-case"), "{error}");
    }

    #[test]
    fn anchors_and_aliases_are_rejected() {
        let yaml = MINIMAL.replace(
            "default: {answer: hello}",
            "default: &shared {answer: hello}\n      copy: *shared",
        );
        let error = SemanticFixture::from_yaml("test", &yaml)
            .unwrap_err()
            .to_string();
        assert!(error.contains("anchors and aliases"), "{error}");
    }

    #[test]
    fn date_like_plain_scalars_must_be_quoted() {
        let yaml = MINIMAL.replace("text: hello", "text: 2026-07-26");
        let error = SemanticFixture::from_yaml("test", &yaml)
            .unwrap_err()
            .to_string();
        assert!(error.contains("must be quoted"), "{error}");

        let quoted = MINIMAL.replace("text: hello", "text: '2026-07-26'");
        SemanticFixture::from_yaml("test", &quoted).unwrap();
    }

    #[test]
    fn block_scalar_content_is_not_treated_as_yaml_structure() {
        let yaml = MINIMAL.replace(
            "default: {answer: hello}",
            "default:\n        answer: |-\n          Release date: 2026-07-26\n          * deterministic",
        );
        let fixture = SemanticFixture::from_yaml("test", &yaml).unwrap();
        fixture.lint().unwrap();
    }

    #[test]
    fn structured_variants_own_and_enforce_supported_schemas() {
        let mut fixture = builtin_fixtures()
            .into_iter()
            .find(|fixture| fixture.id == "structured-output")
            .unwrap();
        let case = &mut fixture.cases[0];
        let valid = case
            .variants
            .get_mut("valid")
            .unwrap()
            .structured_output
            .as_mut()
            .unwrap();
        valid.schema["unevaluatedProperties"] = Value::Bool(false);
        let error = fixture.lint().unwrap_err().to_string();
        assert!(error.contains("unsupported schema keyword"), "{error}");

        let mut fixture = builtin_fixtures()
            .into_iter()
            .find(|fixture| fixture.id == "structured-output")
            .unwrap();
        fixture.cases[0]
            .variants
            .get_mut("valid")
            .unwrap()
            .structured_output
            .as_mut()
            .unwrap()
            .json = r#"{"status":"green"}"#.to_string();
        let error = fixture.lint().unwrap_err().to_string();
        assert!(error.contains("must satisfy its owned schema"), "{error}");
    }

    #[test]
    fn negative_structured_variants_are_named_invalid_and_never_defaults() {
        let mut fixture = builtin_fixtures()
            .into_iter()
            .find(|fixture| fixture.id == "structured-output")
            .unwrap();
        fixture.cases[0].default_variant = "negative-missing-blockers".to_string();
        let error = fixture.lint().unwrap_err().to_string();
        assert!(error.contains("cannot be defaults"), "{error}");

        let mut fixture = builtin_fixtures()
            .into_iter()
            .find(|fixture| fixture.id == "structured-output")
            .unwrap();
        fixture.cases[0]
            .variants
            .get_mut("negative-missing-blockers")
            .unwrap()
            .structured_output
            .as_mut()
            .unwrap()
            .json = r#"{"status":"green","blockers":0}"#.to_string();
        let error = fixture.lint().unwrap_err().to_string();
        assert!(error.contains("must violate its owned schema"), "{error}");
    }
}
