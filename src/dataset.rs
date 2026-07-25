use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::cast::{as_primitive_array, as_string_array};
use arrow_array::types::UInt32Type;
use arrow_array::{Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{ArrowError, DataType, Field, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriter;
use thiserror::Error;
use tiktoken_rs::CoreBPE;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::warn;

use crate::model::{
    ChatCompletionMessageToolCall, ChatCompletionRequestMessage, ChatCompletionResponse,
    ChatCompletionResponseMessageAudio, ChatCompletionUsage, ChatRole, FinishReason, FunctionCall,
    MessageContent,
};

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

/// Represents a single row that can be written to the parquet dataset.
#[derive(Clone)]
pub struct DatasetRow<'a> {
    pub conversation_id: Cow<'a, str>,
    pub turn_index: u32,
    pub role: Cow<'a, str>,
    pub content: Option<Cow<'a, str>>,
    pub content_parts: Option<Cow<'a, str>>,
    pub refusal: Option<Cow<'a, str>>,
    pub tool_calls: Option<Cow<'a, str>>,
    pub function_call: Option<Cow<'a, str>>,
    pub audio: Option<Cow<'a, str>>,
    pub finish_reason: Option<Cow<'a, str>>,
    pub usage: Option<Cow<'a, str>>,
}

impl<'a> DatasetRow<'a> {
    pub fn to_owned(&self) -> DatasetRow<'static> {
        DatasetRow {
            conversation_id: Cow::Owned(self.conversation_id.to_string()),
            turn_index: self.turn_index,
            role: Cow::Owned(self.role.to_string()),
            content: self
                .content
                .as_ref()
                .map(|value| Cow::Owned(value.to_string())),
            content_parts: self
                .content_parts
                .as_ref()
                .map(|value| Cow::Owned(value.to_string())),
            refusal: self
                .refusal
                .as_ref()
                .map(|value| Cow::Owned(value.to_string())),
            tool_calls: self
                .tool_calls
                .as_ref()
                .map(|value| Cow::Owned(value.to_string())),
            function_call: self
                .function_call
                .as_ref()
                .map(|value| Cow::Owned(value.to_string())),
            audio: self
                .audio
                .as_ref()
                .map(|value| Cow::Owned(value.to_string())),
            finish_reason: self
                .finish_reason
                .as_ref()
                .map(|value| Cow::Owned(value.to_string())),
            usage: self
                .usage
                .as_ref()
                .map(|value| Cow::Owned(value.to_string())),
        }
    }
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
                let base_text = raw.content.clone().unwrap_or_default();
                let mut turn_content: Arc<str> = Arc::from(base_text.clone());
                let assistant_index = if matches!(role, ConversationRole::Assistant) {
                    let message_content = message_content_from(&raw);
                    let rendered = message_content
                        .as_ref()
                        .map(|content| content.render().into_owned())
                        .unwrap_or(base_text.clone());
                    let rendered_arc: Arc<str> = Arc::from(rendered.clone());
                    let tokens = tokenizer.encode_with_special_tokens(rendered_arc.as_ref());
                    let message = AssistantMessage {
                        rendered_text: rendered_arc.clone(),
                        tokens: Arc::from(tokens.into_boxed_slice()),
                        content: message_content,
                        refusal: raw.refusal.clone(),
                        tool_calls: parse_json_field(raw.tool_calls.as_deref(), "tool_calls"),
                        function_call: parse_json_field(
                            raw.function_call.as_deref(),
                            "function_call",
                        ),
                        audio: parse_json_field(raw.audio.as_deref(), "audio"),
                        finish_reason: parse_finish_reason(raw.finish_reason.as_deref()),
                        usage: parse_json_field(raw.usage.as_deref(), "usage"),
                    };
                    assistants.push(message);
                    turn_content = rendered_arc;
                    Some(assistants.len() - 1)
                } else {
                    None
                };

                conversation_turns.push(ConversationTurn {
                    role,
                    content: turn_content.clone(),
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
    pub fn turns(&self) -> &[ConversationTurn] {
        &self.turns
    }

    pub fn assistants(&self) -> &[AssistantMessage] {
        &self.assistants
    }

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
    pub rendered_text: Arc<str>,
    pub tokens: Arc<[u32]>,
    pub content: Option<MessageContent>,
    pub refusal: Option<String>,
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCall>>,
    pub function_call: Option<FunctionCall>,
    pub audio: Option<ChatCompletionResponseMessageAudio>,
    pub finish_reason: Option<FinishReason>,
    pub usage: Option<ChatCompletionUsage>,
}

#[derive(Clone)]
pub struct ConversationTurn {
    pub role: ConversationRole,
    pub content: Arc<str>,
    assistant_index: Option<usize>,
}

impl ConversationTurn {
    pub fn assistant_slot(&self) -> Option<usize> {
        self.assistant_index
    }

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
    content: Option<String>,
    content_parts: Option<String>,
    refusal: Option<String>,
    tool_calls: Option<String>,
    function_call: Option<String>,
    audio: Option<String>,
    finish_reason: Option<String>,
    usage: Option<String>,
}

fn read_string(batch: &RecordBatch, index: Option<usize>, row: usize) -> Option<String> {
    index.and_then(|idx| {
        let array = as_string_array(batch.column(idx));
        if array.is_null(row) {
            None
        } else {
            Some(array.value(row).to_owned())
        }
    })
}

fn message_content_from(raw: &RawTurn) -> Option<MessageContent> {
    if let Some(parts) = raw.content_parts.as_deref() {
        parse_json_field::<Vec<Value>>(Some(parts), "content_parts").map(MessageContent::Parts)
    } else {
        raw.content
            .as_ref()
            .map(|text| MessageContent::Text(text.clone()))
    }
}

fn parse_finish_reason(value: Option<&str>) -> Option<FinishReason> {
    value.and_then(|raw| match raw {
        "stop" => Some(FinishReason::Stop),
        "length" => Some(FinishReason::Length),
        "tool_calls" => Some(FinishReason::ToolCalls),
        "content_filter" => Some(FinishReason::ContentFilter),
        "function_call" => Some(FinishReason::FunctionCall),
        other => {
            warn!(
                target: "no_llm_api",
                field = "finish_reason",
                %other,
                "unrecognised finish reason in dataset"
            );
            None
        }
    })
}

fn parse_json_field<T>(value: Option<&str>, field: &str) -> Option<T>
where
    T: DeserializeOwned,
{
    value.and_then(|raw| match serde_json::from_str::<T>(raw) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            warn!(
                target: "no_llm_api",
                field = field,
                ?error,
                "failed to parse dataset json field"
            );
            None
        }
    })
}

fn ingest_batch(
    target: &mut HashMap<String, Vec<RawTurn>>,
    batch: &RecordBatch,
) -> Result<(), DatasetError> {
    let schema = batch.schema();
    let conversation_idx = schema.index_of("conversation_id")?;
    let turn_idx = schema.index_of("turn_index")?;
    let role_idx = schema.index_of("role")?;
    let content_idx = schema.column_with_name("content").map(|(idx, _)| idx);
    let content_parts_idx = schema.column_with_name("content_parts").map(|(idx, _)| idx);
    let refusal_idx = schema.column_with_name("refusal").map(|(idx, _)| idx);
    let tool_calls_idx = schema.column_with_name("tool_calls").map(|(idx, _)| idx);
    let function_call_idx = schema.column_with_name("function_call").map(|(idx, _)| idx);
    let audio_idx = schema.column_with_name("audio").map(|(idx, _)| idx);
    let finish_reason_idx = schema.column_with_name("finish_reason").map(|(idx, _)| idx);
    let usage_idx = schema.column_with_name("usage").map(|(idx, _)| idx);

    let conversation_ids = as_string_array(batch.column(conversation_idx));
    let turn_indices = as_primitive_array::<UInt32Type>(batch.column(turn_idx));
    let roles = as_string_array(batch.column(role_idx));

    for row in 0..batch.num_rows() {
        let conversation_id = conversation_ids.value(row).to_owned();
        let turn_index = turn_indices.value(row);
        let role = roles.value(row).to_owned();
        let content = read_string(batch, content_idx, row);
        let content_parts = read_string(batch, content_parts_idx, row);
        let refusal = read_string(batch, refusal_idx, row);
        let tool_calls = read_string(batch, tool_calls_idx, row);
        let function_call = read_string(batch, function_call_idx, row);
        let audio = read_string(batch, audio_idx, row);
        let finish_reason = read_string(batch, finish_reason_idx, row);
        let usage = read_string(batch, usage_idx, row);

        target.entry(conversation_id).or_default().push(RawTurn {
            turn_index,
            role,
            content,
            content_parts,
            refusal,
            tool_calls,
            function_call,
            audio,
            finish_reason,
            usage,
        });
    }

    Ok(())
}

pub fn write_dataset(path: &Path, rows: &[DatasetRow<'_>]) -> Result<PathBuf, DatasetError> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }

    let schema = Arc::new(Schema::new(vec![
        Field::new("conversation_id", DataType::Utf8, false),
        Field::new("turn_index", DataType::UInt32, false),
        Field::new("role", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, true),
        Field::new("content_parts", DataType::Utf8, true),
        Field::new("refusal", DataType::Utf8, true),
        Field::new("tool_calls", DataType::Utf8, true),
        Field::new("function_call", DataType::Utf8, true),
        Field::new("audio", DataType::Utf8, true),
        Field::new("finish_reason", DataType::Utf8, true),
        Field::new("usage", DataType::Utf8, true),
    ]));

    let conversation_ids: Vec<&str> = rows
        .iter()
        .map(|row| row.conversation_id.as_ref())
        .collect();
    let turn_indices: Vec<u32> = rows.iter().map(|row| row.turn_index).collect();
    let roles: Vec<&str> = rows.iter().map(|row| row.role.as_ref()).collect();
    let contents: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.content.as_ref().map(|value| value.as_ref()))
        .collect();
    let content_parts: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.content_parts.as_ref().map(|value| value.as_ref()))
        .collect();
    let refusals: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.refusal.as_ref().map(|value| value.as_ref()))
        .collect();
    let tool_calls: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.tool_calls.as_ref().map(|value| value.as_ref()))
        .collect();
    let function_calls: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.function_call.as_ref().map(|value| value.as_ref()))
        .collect();
    let audios: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.audio.as_ref().map(|value| value.as_ref()))
        .collect();
    let finish_reasons: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.finish_reason.as_ref().map(|value| value.as_ref()))
        .collect();
    let usages: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.usage.as_ref().map(|value| value.as_ref()))
        .collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(conversation_ids)),
            Arc::new(UInt32Array::from(turn_indices)),
            Arc::new(StringArray::from(roles)),
            Arc::new(StringArray::from(contents)),
            Arc::new(StringArray::from(content_parts)),
            Arc::new(StringArray::from(refusals)),
            Arc::new(StringArray::from(tool_calls)),
            Arc::new(StringArray::from(function_calls)),
            Arc::new(StringArray::from(audios)),
            Arc::new(StringArray::from(finish_reasons)),
            Arc::new(StringArray::from(usages)),
        ],
    )?;

    let file = File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, None)?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(path.to_path_buf())
}

pub fn append_dataset_rows(
    path: &Path,
    new_rows: &[DatasetRow<'_>],
) -> Result<PathBuf, DatasetError> {
    let mut existing = read_dataset_rows(path)?;
    for row in new_rows {
        existing.push(row.to_owned());
    }
    write_dataset(path, &existing)
}

pub fn read_dataset_rows(path: &Path) -> Result<Vec<DatasetRow<'static>>, DatasetError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let reader = builder.with_batch_size(256).build()?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        rows.extend(rows_from_batch(&batch)?);
    }
    Ok(rows)
}

fn rows_from_batch(batch: &RecordBatch) -> Result<Vec<DatasetRow<'static>>, DatasetError> {
    let schema = batch.schema();
    let conversation_idx = schema.index_of("conversation_id")?;
    let turn_idx = schema.index_of("turn_index")?;
    let role_idx = schema.index_of("role")?;
    let content_idx = schema.column_with_name("content").map(|(idx, _)| idx);
    let content_parts_idx = schema.column_with_name("content_parts").map(|(idx, _)| idx);
    let refusal_idx = schema.column_with_name("refusal").map(|(idx, _)| idx);
    let tool_calls_idx = schema.column_with_name("tool_calls").map(|(idx, _)| idx);
    let function_call_idx = schema.column_with_name("function_call").map(|(idx, _)| idx);
    let audio_idx = schema.column_with_name("audio").map(|(idx, _)| idx);
    let finish_reason_idx = schema.column_with_name("finish_reason").map(|(idx, _)| idx);
    let usage_idx = schema.column_with_name("usage").map(|(idx, _)| idx);

    let conversation_ids = as_string_array(batch.column(conversation_idx));
    let turn_indices = as_primitive_array::<UInt32Type>(batch.column(turn_idx));
    let roles = as_string_array(batch.column(role_idx));

    let mut rows = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let conversation_id = conversation_ids.value(row).to_owned();
        let turn_index = turn_indices.value(row);
        let role = roles.value(row).to_owned();
        rows.push(DatasetRow {
            conversation_id: Cow::Owned(conversation_id),
            turn_index,
            role: Cow::Owned(role),
            content: read_string(batch, content_idx, row).map(Cow::Owned),
            content_parts: read_string(batch, content_parts_idx, row).map(Cow::Owned),
            refusal: read_string(batch, refusal_idx, row).map(Cow::Owned),
            tool_calls: read_string(batch, tool_calls_idx, row).map(Cow::Owned),
            function_call: read_string(batch, function_call_idx, row).map(Cow::Owned),
            audio: read_string(batch, audio_idx, row).map(Cow::Owned),
            finish_reason: read_string(batch, finish_reason_idx, row).map(Cow::Owned),
            usage: read_string(batch, usage_idx, row).map(Cow::Owned),
        });
    }
    Ok(rows)
}

pub fn ensure_sample_dataset(path: &Path) -> Result<PathBuf, DatasetError> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    write_sample_dataset(path)
}

/// Writes the bundled sample dataset, replacing whatever is there.
///
/// `ensure_sample_dataset` returns early when the file exists, which is why
/// regeneration needs its own entry point instead of silently doing nothing.
pub fn write_sample_dataset(path: &Path) -> Result<PathBuf, DatasetError> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)?;
    }

    write_dataset(path, &sample_rows())
}

/// The bundled fixture rows, exposed so a test can detect drift between the
/// tracked parquet file and the source of truth in this module.
pub fn bundled_rows() -> Vec<DatasetRow<'static>> {
    sample_rows()
}

fn role_as_str(role: &ChatRole) -> &'static str {
    match role {
        ChatRole::System => "system",
        ChatRole::Developer => "developer",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
        ChatRole::Function => "function",
    }
}

fn finish_reason_as_str(reason: &FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::FunctionCall => "function_call",
    }
}

fn encode_message_content(
    content: Option<&MessageContent>,
) -> (Option<Cow<'static, str>>, Option<Cow<'static, str>>) {
    match content {
        Some(MessageContent::Text(text)) => {
            if text.is_empty() {
                (None, None)
            } else {
                (Some(Cow::Owned(text.clone())), None)
            }
        }
        Some(MessageContent::Parts(parts)) => {
            let rendered = MessageContent::Parts(parts.clone()).render().into_owned();
            let json = serde_json::to_string(parts).ok();
            (
                if rendered.is_empty() {
                    None
                } else {
                    Some(Cow::Owned(rendered))
                },
                json.map(Cow::Owned),
            )
        }
        None => (None, None),
    }
}

fn optional_json<T>(value: Option<&T>) -> Option<Cow<'static, str>>
where
    T: Serialize,
{
    value
        .and_then(|inner| serde_json::to_string(inner).ok())
        .map(Cow::Owned)
}

pub fn rows_from_interaction(
    conversation_id: &str,
    request: &[ChatCompletionRequestMessage],
    response: &ChatCompletionResponse,
) -> Vec<DatasetRow<'static>> {
    let mut rows = Vec::new();
    for (index, message) in request.iter().enumerate() {
        let (content, content_parts) = encode_message_content(message.content.as_ref());
        rows.push(DatasetRow {
            conversation_id: Cow::Owned(conversation_id.to_string()),
            turn_index: index as u32,
            role: Cow::Owned(role_as_str(&message.role).to_string()),
            content,
            content_parts,
            refusal: message
                .refusal
                .as_ref()
                .map(|value| Cow::Owned(value.clone())),
            tool_calls: optional_json(message.tool_calls.as_ref()),
            function_call: optional_json(message.function_call.as_ref()),
            audio: optional_json(message.audio.as_ref()),
            finish_reason: None,
            usage: None,
        });
    }

    if let Some(choice) = response.choices.first() {
        let message = &choice.message;
        let (content, content_parts) = encode_message_content(message.content.as_ref());
        rows.push(DatasetRow {
            conversation_id: Cow::Owned(conversation_id.to_string()),
            turn_index: rows.len() as u32,
            role: Cow::Owned("assistant".to_string()),
            content,
            content_parts,
            refusal: message
                .refusal
                .as_ref()
                .map(|value| Cow::Owned(value.clone())),
            tool_calls: optional_json(message.tool_calls.as_ref()),
            function_call: optional_json(message.function_call.as_ref()),
            audio: optional_json(message.audio.as_ref()),
            finish_reason: choice
                .finish_reason
                .as_ref()
                .map(|reason| Cow::Owned(finish_reason_as_str(reason).to_string())),
            usage: serde_json::to_string(&response.usage).ok().map(Cow::Owned),
        });
    }

    rows
}

/// The bundled fixture rows, built from the YAML fixture sets in `fixtures/`.
///
/// These used to be hundreds of lines of hand-written row literals, which is
/// precisely why nobody extended them.
fn sample_rows() -> Vec<DatasetRow<'static>> {
    crate::fixtures::builtin_rows()
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
        let shared = scripts.share();
        assert!(!shared.is_empty());

        let lyra = shared
            .iter()
            .find(|script| script.id.as_ref() == "conv-lyra")
            .expect("conv-lyra present");
        let lyra_first = lyra.assistant_at(0);
        assert_eq!(lyra_first.finish_reason, Some(FinishReason::ToolCalls));
        assert!(
            lyra_first
                .tool_calls
                .as_ref()
                .map(|calls| !calls.is_empty())
                .unwrap_or(false)
        );

        let lyra_second = lyra.assistant_at(1);
        assert!(lyra_second.function_call.is_some());

        let vega = shared
            .iter()
            .find(|script| script.id.as_ref() == "conv-vega")
            .expect("conv-vega present");
        let vega_refusal = vega.assistant_at(1);
        assert_eq!(
            vega_refusal.refusal.as_deref(),
            Some("I’m sorry, but I can’t help with that.")
        );
        assert_eq!(
            vega_refusal.finish_reason,
            Some(FinishReason::ContentFilter)
        );

        let audio = shared
            .iter()
            .find(|script| script.id.as_ref() == "conv-audio")
            .expect("conv-audio present");
        let audio_response = audio.assistant_at(0);
        match audio_response.content {
            Some(MessageContent::Parts(_)) => {}
            other => panic!("expected multi-part content, got {:?}", other),
        }
        let audio_meta = audio_response.audio.expect("audio metadata present");
        assert_eq!(audio_meta.id.as_deref(), Some("aud_123"));
        assert!(
            audio_response
                .usage
                .as_ref()
                .and_then(|usage| usage.prompt_tokens_details.as_ref())
                .is_some()
        );

        let builder = shared
            .iter()
            .find(|script| script.id.as_ref() == "conv-builder")
            .expect("conv-builder present");
        let builder_call = builder.assistant_at(0);
        assert_eq!(builder_call.finish_reason, Some(FinishReason::FunctionCall));
        assert!(builder_call.function_call.is_some());
        Ok(())
    }
}
