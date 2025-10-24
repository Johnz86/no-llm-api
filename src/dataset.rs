use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::cast::{as_primitive_array, as_string_array};
use arrow_array::types::UInt32Type;
use arrow_array::{RecordBatch, StringArray, UInt32Array};
use arrow_schema::{ArrowError, DataType, Field, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriter;
use thiserror::Error;
use tiktoken_rs::CoreBPE;

/// Represents failures that can happen when working with the parquet dataset.
#[derive(Debug, Error)]
pub enum DatasetError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("arrow error: {0}")]
    Arrow(#[from] ArrowError),
    #[error("dataset is empty")]
    Empty,
}

/// Represents the immutable list of scripted conversations loaded from parquet.
#[derive(Clone)]
pub struct ConversationScripts {
    scripts: Arc<[ConversationScript]>,
}

impl ConversationScripts {
    pub fn load(path: &Path, tokenizer: &Arc<CoreBPE>) -> Result<Self, DatasetError> {
        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        let reader = builder.with_batch_size(256).build()?;
        let mut grouped: HashMap<String, Vec<RawTurn>> = HashMap::new();

        for batch in reader {
            let batch = batch?;
            ingest_batch(&mut grouped, &batch)?;
        }

        let mut scripts = Vec::new();
        for (conversation_id, mut turns) in grouped {
            turns.sort_by_key(|turn| turn.turn_index);

            let id = Arc::<str>::from(conversation_id);
            let mut conversation_turns = Vec::with_capacity(turns.len());
            let mut assistants = Vec::new();

            for raw in turns {
                let role = ConversationRole::from(raw.role.as_str());
                let content: Arc<str> = Arc::<str>::from(raw.content);
                let assistant_index = if matches!(role, ConversationRole::Assistant) {
                    let tokens = tokenizer.encode_with_special_tokens(content.as_ref());
                    let message = AssistantMessage {
                        text: content.clone(),
                        tokens: Arc::from(tokens.into_boxed_slice()),
                    };
                    assistants.push(message);
                    Some(assistants.len() - 1)
                } else {
                    None
                };

                conversation_turns.push(ConversationTurn {
                    role,
                    content: content.clone(),
                    assistant_index,
                });
            }

            if assistants.is_empty() {
                continue;
            }

            let turns: Arc<[ConversationTurn]> = conversation_turns.into();
            let assistants: Arc<[AssistantMessage]> = assistants.into();
            scripts.push(ConversationScript {
                id,
                turns,
                assistants,
            });
        }

        if scripts.is_empty() {
            return Err(DatasetError::Empty);
        }

        scripts.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self {
            scripts: scripts.into(),
        })
    }

    pub fn share(&self) -> Arc<[ConversationScript]> {
        self.scripts.clone()
    }
}

#[derive(Clone)]
pub struct ConversationScript {
    pub id: Arc<str>,
    turns: Arc<[ConversationTurn]>,
    assistants: Arc<[AssistantMessage]>,
}

impl ConversationScript {
    pub fn assistant_at(&self, index: usize) -> AssistantMessage {
        let idx = index % self.assistants.len();
        self.assistants[idx].clone()
    }

    pub fn response_for_user(&self, user_text: &str) -> Option<AssistantMessage> {
        let needle = ConversationTurn::normalize(user_text);
        for (idx, turn) in self.turns.iter().enumerate() {
            if !matches!(turn.role, ConversationRole::User) {
                continue;
            }
            if ConversationTurn::normalize(turn.content.as_ref()) == needle {
                return self.turns.iter().skip(idx + 1).find_map(|next| {
                    next.assistant_index()
                        .map(|slot| self.assistants[slot].clone())
                });
            }
        }
        None
    }
}

#[derive(Clone)]
pub struct AssistantMessage {
    pub text: Arc<str>,
    pub tokens: Arc<[u32]>,
}

#[derive(Clone)]
pub struct ConversationTurn {
    pub role: ConversationRole,
    pub content: Arc<str>,
    assistant_index: Option<usize>,
}

impl ConversationTurn {
    fn assistant_index(&self) -> Option<usize> {
        self.assistant_index
    }

    fn normalize(text: &str) -> String {
        text.trim().to_ascii_lowercase()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConversationRole {
    System,
    User,
    Assistant,
    Tool,
    Unknown,
}

impl ConversationRole {
    fn from(value: &str) -> Self {
        match value {
            "system" => Self::System,
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "tool" => Self::Tool,
            _ => Self::Unknown,
        }
    }
}

struct RawTurn {
    turn_index: u32,
    role: String,
    content: String,
}

fn ingest_batch(
    target: &mut HashMap<String, Vec<RawTurn>>,
    batch: &RecordBatch,
) -> Result<(), DatasetError> {
    let conversation_ids = as_string_array(batch.column(0));
    let turn_indices = as_primitive_array::<UInt32Type>(batch.column(1));
    let roles = as_string_array(batch.column(2));
    let contents = as_string_array(batch.column(3));

    for row in 0..batch.num_rows() {
        let conversation_id = conversation_ids.value(row).to_owned();
        let turn_index = turn_indices.value(row);
        let role = roles.value(row).to_owned();
        let content = contents.value(row).to_owned();
        target.entry(conversation_id).or_default().push(RawTurn {
            turn_index,
            role,
            content,
        });
    }

    Ok(())
}

pub fn ensure_sample_dataset(path: &Path) -> Result<PathBuf, DatasetError> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }

    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("conversation_id", DataType::Utf8, false),
        Field::new("turn_index", DataType::UInt32, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
    ]));

    let rows = sample_rows();
    let conversation_ids: Vec<&str> = rows.iter().map(|row| row.conversation_id).collect();
    let turn_indices: Vec<u32> = rows.iter().map(|row| row.turn_index).collect();
    let roles: Vec<&str> = rows.iter().map(|row| row.role).collect();
    let contents: Vec<&str> = rows.iter().map(|row| row.content).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(conversation_ids)),
            Arc::new(UInt32Array::from(turn_indices)),
            Arc::new(StringArray::from(roles)),
            Arc::new(StringArray::from(contents)),
        ],
    )?;

    let file = File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, None)?;
    writer.write(&batch)?;
    writer.close()?;

    Ok(path.to_path_buf())
}

struct SampleRow {
    conversation_id: &'static str,
    turn_index: u32,
    role: &'static str,
    content: &'static str,
}

fn sample_rows() -> Vec<SampleRow> {
    vec![
        SampleRow {
            conversation_id: "conv-orion",
            turn_index: 0,
            role: "system",
            content: "You are an efficient assistant who speaks in short sentences.",
        },
        SampleRow {
            conversation_id: "conv-orion",
            turn_index: 1,
            role: "user",
            content: "Summarize the sprint update.",
        },
        SampleRow {
            conversation_id: "conv-orion",
            turn_index: 2,
            role: "assistant",
            content: "Sprint closed 14 tickets, shipped analytics, and stabilized the API.",
        },
        SampleRow {
            conversation_id: "conv-orion",
            turn_index: 3,
            role: "user",
            content: "Highlight risks?",
        },
        SampleRow {
            conversation_id: "conv-orion",
            turn_index: 4,
            role: "assistant",
            content: "Cloud migration is a week late; auth failover still in QA.",
        },
        SampleRow {
            conversation_id: "conv-lyra",
            turn_index: 0,
            role: "system",
            content: "You are a supportive assistant that uses actionable advice.",
        },
        SampleRow {
            conversation_id: "conv-lyra",
            turn_index: 1,
            role: "user",
            content: "Help me unblock a failing integration test.",
        },
        SampleRow {
            conversation_id: "conv-lyra",
            turn_index: 2,
            role: "assistant",
            content: "Enable verbose logging, capture the failing payload, and replay locally.",
        },
        SampleRow {
            conversation_id: "conv-lyra",
            turn_index: 3,
            role: "user",
            content: "What next if replay still fails?",
        },
        SampleRow {
            conversation_id: "conv-lyra",
            turn_index: 4,
            role: "assistant",
            content: "Diff the dependency tree, pin versions, and bisect recent merges.",
        },
        SampleRow {
            conversation_id: "conv-vega",
            turn_index: 0,
            role: "system",
            content: "You are an assistant who answers with bullet lists only when needed.",
        },
        SampleRow {
            conversation_id: "conv-vega",
            turn_index: 1,
            role: "user",
            content: "Outline a team retrospective agenda.",
        },
        SampleRow {
            conversation_id: "conv-vega",
            turn_index: 2,
            role: "assistant",
            content: "Opening round, metrics review, wins, friction points, experiments, next steps.",
        },
        SampleRow {
            conversation_id: "conv-vega",
            turn_index: 3,
            role: "user",
            content: "Expand on experiments.",
        },
        SampleRow {
            conversation_id: "conv-vega",
            turn_index: 4,
            role: "assistant",
            content: "Pilot async standups, rotate facilitator, and test pairing hours weekly.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tiktoken_rs::cl100k_base;

    #[test]
    fn loads_sample_dataset() -> Result<(), DatasetError> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.parquet");
        ensure_sample_dataset(&path)?;
        let tokenizer = Arc::new(cl100k_base().unwrap());
        let scripts = ConversationScripts::load(&path, &tokenizer)?;
        assert!(!scripts.share().is_empty());
        Ok(())
    }
}
