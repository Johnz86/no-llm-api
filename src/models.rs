//! The model catalogue behind `GET /models`.
//!
//! Only the four spec fields of the `Model` schema (`openapi.yaml:43830`) are
//! serialised to clients. Simulation-only fields stay in `ModelProfile` so no
//! GUI ever sees a key its schema does not know.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The wire shape of one catalogue entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    #[serde(default = "model_object")]
    pub object: String,
    pub created: i64,
    pub owned_by: String,
}

fn model_object() -> String {
    "model".to_string()
}

/// `ListModelsResponse` (`openapi.yaml:42437`).
#[derive(Debug, Clone, Serialize)]
pub struct ModelList {
    pub object: &'static str,
    pub data: Vec<Model>,
}

/// Simulation knobs attached to a model; not part of any client-visible payload.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelProfile {
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub latency: Latency,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
    #[serde(default)]
    pub structured_output: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Latency {
    #[serde(default)]
    pub ttft_ms: Option<u64>,
    #[serde(default)]
    pub tokens_per_second: Option<u32>,
    #[serde(default)]
    pub jitter_ms: Option<u64>,
}

/// One catalogue file entry: spec fields plus simulation profile, flattened.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default = "default_created")]
    pub created: i64,
    #[serde(default = "default_owner")]
    pub owned_by: String,
    #[serde(flatten)]
    pub profile: ModelProfile,
}

fn default_created() -> i64 {
    1_735_689_600
}

fn default_owner() -> String {
    "no-llm-api".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum CatalogueFile {
    Wrapped { data: Vec<ModelEntry> },
    Bare(Vec<ModelEntry>),
}

/// The resolved catalogue, shared by the router.
#[derive(Debug, Clone)]
pub struct ModelCatalogue {
    entries: Arc<[ModelEntry]>,
}

impl ModelCatalogue {
    pub fn from_entries(entries: Vec<ModelEntry>) -> Self {
        Self {
            entries: entries.into(),
        }
    }

    /// Built-in catalogue: the zero-configuration default.
    pub fn builtin() -> Self {
        Self::from_entries(vec![
            builtin_entry("mock-gpt-4o", false),
            builtin_entry("mock-gpt-4o-mini", false),
            builtin_entry("mock-reasoner", true),
        ])
    }

    pub fn from_ids<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::from_entries(
            ids.into_iter()
                .map(|id| ModelEntry {
                    id: id.into(),
                    created: default_created(),
                    owned_by: default_owner(),
                    profile: ModelProfile::default(),
                })
                .collect(),
        )
    }

    /// Reads a catalogue file, accepting either a bare array or `{ "data": [...] }`.
    pub fn from_path(path: &Path) -> Result<Self, CatalogueError> {
        let text = std::fs::read_to_string(path)?;
        let parsed: CatalogueFile = serde_json::from_str(&text)?;
        let entries = match parsed {
            CatalogueFile::Wrapped { data } => data,
            CatalogueFile::Bare(data) => data,
        };
        if entries.is_empty() {
            return Err(CatalogueError::Empty);
        }
        Ok(Self::from_entries(entries))
    }

    /// Resolves the catalogue by the documented precedence:
    /// explicit file, then explicit id list, then the built-in default.
    pub fn resolve(path: Option<&Path>, ids: Option<&[String]>) -> Self {
        if let Some(path) = path {
            match Self::from_path(path) {
                Ok(catalogue) => return catalogue,
                Err(error) => {
                    tracing::warn!(
                        target: "no_llm_api",
                        ?error,
                        path = %path.display(),
                        "model catalogue unreadable; falling back"
                    );
                }
            }
        }
        match ids {
            Some(ids) if !ids.is_empty() => Self::from_ids(ids.iter().cloned()),
            _ => Self::builtin(),
        }
    }

    pub fn list(&self) -> ModelList {
        ModelList {
            object: "list",
            data: self.entries.iter().map(to_model).collect(),
        }
    }

    pub fn get(&self, id: &str) -> Option<Model> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(to_model)
    }

    pub fn profile(&self, id: &str) -> Option<&ModelProfile> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.profile)
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.id.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The catalogue including simulation profiles, for `GET /_mock/models` only.
    pub fn debug_view(&self) -> serde_json::Value {
        serde_json::json!({
            "object": "list",
            "data": self
                .entries
                .iter()
                .map(|entry| serde_json::json!({
                    "id": entry.id,
                    "created": entry.created,
                    "owned_by": entry.owned_by,
                    "profile": entry.profile,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

fn builtin_entry(id: &str, reasoning: bool) -> ModelEntry {
    ModelEntry {
        id: id.to_string(),
        created: default_created(),
        owned_by: default_owner(),
        profile: ModelProfile {
            capabilities: Capabilities {
                reasoning,
                reasoning_efforts: if reasoning {
                    ["low", "medium", "high"]
                        .into_iter()
                        .map(str::to_string)
                        .collect()
                } else {
                    Vec::new()
                },
                structured_output: true,
                ..Default::default()
            },
            ..Default::default()
        },
    }
}

fn to_model(entry: &ModelEntry) -> Model {
    Model {
        id: entry.id.clone(),
        object: model_object(),
        created: entry.created,
        owned_by: entry.owned_by.clone(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid catalogue json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("catalogue is empty")]
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalogue_is_non_empty_and_spec_shaped() {
        let catalogue = ModelCatalogue::builtin();
        let list = catalogue.list();
        assert_eq!(list.object, "list");
        assert!(!list.data.is_empty());
        for model in &list.data {
            assert_eq!(model.object, "model");
            assert_eq!(model.owned_by, "no-llm-api");
        }
        let reasoner = catalogue.profile("mock-reasoner").unwrap();
        assert!(reasoner.capabilities.reasoning);
        assert_eq!(
            reasoner.capabilities.reasoning_efforts,
            ["low", "medium", "high"]
        );
        assert!(reasoner.capabilities.structured_output);
    }

    #[test]
    fn explicit_ids_win_over_the_builtin_list() {
        let ids = vec!["only-this".to_string()];
        let catalogue = ModelCatalogue::resolve(None, Some(&ids));
        assert_eq!(catalogue.ids().collect::<Vec<_>>(), vec!["only-this"]);
    }

    #[test]
    fn catalogue_file_accepts_both_layouts_and_keeps_profiles_private() {
        let dir = tempfile::tempdir().unwrap();
        let wrapped = dir.path().join("wrapped.json");
        std::fs::write(
            &wrapped,
            r#"{"data":[{"id":"a","context_window":128000,"capabilities":{"tools":true}}]}"#,
        )
        .unwrap();
        let catalogue = ModelCatalogue::from_path(&wrapped).unwrap();
        assert_eq!(catalogue.profile("a").unwrap().context_window, Some(128000));
        assert!(catalogue.profile("a").unwrap().capabilities.tools);
        let serialised = serde_json::to_value(catalogue.list()).unwrap();
        assert_eq!(
            serialised["data"][0].as_object().unwrap().keys().len(),
            4,
            "only the four spec fields may reach a client"
        );

        let bare = dir.path().join("bare.json");
        std::fs::write(&bare, r#"[{"id":"b","created":1,"owned_by":"me"}]"#).unwrap();
        let catalogue = ModelCatalogue::from_path(&bare).unwrap();
        let model = catalogue.get("b").unwrap();
        assert_eq!((model.created, model.owned_by.as_str()), (1, "me"));
    }

    #[test]
    fn unreadable_file_falls_back_instead_of_failing() {
        let catalogue = ModelCatalogue::resolve(Some(Path::new("does-not-exist.json")), None);
        assert!(!catalogue.is_empty());
    }
}
