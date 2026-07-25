//! Deterministic fixture selection.
//!
//! Selection is a pure function of the normalised request: no counters, no
//! rotation, no interior mutability. Two identical requests therefore choose the
//! same fixture on any machine and under any amount of concurrency, which is the
//! precondition for every snapshot assertion downstream.

use std::collections::HashMap;
use std::sync::Arc;

use crate::dataset::{AssistantMessage, ConversationRole, ConversationScript};
use crate::model::{ChatCompletionRequest, ChatCompletionRequestMessage, ChatRole};
use crate::sim::digest::{Digest, pick};

/// How a reply was chosen, surfaced as `X-Simulate-Match` for test triage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// The request's user turns are a prefix of a scripted conversation.
    ConversationPrefix,
    /// The request's trailing user turns appear contiguously in a script.
    Suffix,
    /// The last user message matches a scripted user turn exactly.
    LastUser,
    /// Nothing matched; a fixture was derived from the request digest.
    Fallback,
}

impl MatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchKind::ConversationPrefix => "conversation_prefix",
            MatchKind::Suffix => "suffix",
            MatchKind::LastUser => "last_user",
            MatchKind::Fallback => "fallback",
        }
    }
}

/// Collapses insignificant differences so equivalent prompts match.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        for lowered in ch.to_lowercase() {
            out.push(lowered);
        }
    }
    out
}

/// The normalised request, and the only thing selection is allowed to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionKey {
    pub model: String,
    /// Normalised `(role, content)` pairs, in request order.
    pub turns: Vec<(String, String)>,
    pub user_turns: Vec<String>,
}

impl SelectionKey {
    pub fn from_request(request: &ChatCompletionRequest) -> Self {
        let turns: Vec<(String, String)> = request
            .messages
            .iter()
            .map(|message| (role_key(message).to_string(), normalized_content(message)))
            .collect();
        let user_turns = turns
            .iter()
            .filter(|(role, _)| role == "user")
            .map(|(_, content)| content.clone())
            .collect();
        Self {
            model: request.model.clone(),
            turns,
            user_turns,
        }
    }

    /// A stable digest over everything that can influence the reply.
    pub fn digest(&self) -> u64 {
        let mut digest = Digest::new();
        digest.field(self.model.as_bytes());
        for (role, content) in &self.turns {
            digest.field(role.as_bytes());
            digest.field(content.as_bytes());
        }
        digest.finish()
    }

    pub fn last_user(&self) -> Option<&str> {
        self.user_turns.last().map(String::as_str)
    }
}

fn role_key(message: &ChatCompletionRequestMessage) -> &'static str {
    match message.role {
        ChatRole::System => "system",
        ChatRole::Developer => "developer",
        ChatRole::User => "user",
        ChatRole::Assistant => "assistant",
        ChatRole::Tool => "tool",
        ChatRole::Function => "function",
    }
}

fn normalized_content(message: &ChatCompletionRequestMessage) -> String {
    message
        .content
        .as_ref()
        .map(|content| normalize(content.render().as_ref()))
        .unwrap_or_default()
}

/// An immutable, pre-indexed view of the fixture set.
pub struct ScriptIndex {
    scripts: Arc<[ConversationScript]>,
    /// Normalised user text of every scripted user turn -> the reply that follows.
    by_last_user: HashMap<String, (usize, usize)>,
    /// Normalised user turns per script, in order.
    user_turns: Vec<Vec<String>>,
    /// The reply that answers each scripted user turn, aligned with `user_turns`.
    replies: Vec<Vec<usize>>,
}

impl ScriptIndex {
    pub fn build(scripts: Arc<[ConversationScript]>) -> Self {
        let mut by_last_user = HashMap::new();
        let mut user_turns = Vec::with_capacity(scripts.len());
        let mut replies = Vec::with_capacity(scripts.len());

        for (script_index, script) in scripts.iter().enumerate() {
            let mut script_users = Vec::new();
            let mut script_replies = Vec::new();
            let turns = script.turns();
            for (turn_index, turn) in turns.iter().enumerate() {
                if !matches!(turn.role, ConversationRole::User) {
                    continue;
                }
                let Some(reply) = turns
                    .iter()
                    .skip(turn_index + 1)
                    .find_map(|next| next.assistant_slot())
                else {
                    continue;
                };
                let normalized = normalize(turn.content.as_ref());
                by_last_user
                    .entry(normalized.clone())
                    .or_insert((script_index, reply));
                script_users.push(normalized);
                script_replies.push(reply);
            }
            user_turns.push(script_users);
            replies.push(script_replies);
        }

        Self {
            scripts,
            by_last_user,
            user_turns,
            replies,
        }
    }

    pub fn len(&self) -> usize {
        self.scripts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    /// Chooses a reply for a request. Never fails while any fixture exists.
    pub fn select(&self, key: &SelectionKey) -> Selection {
        if let Some(selection) = self.match_conversation_prefix(key) {
            return selection;
        }
        if let Some(selection) = self.match_suffix(key) {
            return selection;
        }
        if let Some(selection) = self.match_last_user(key) {
            return selection;
        }
        self.fallback(key)
    }

    /// Level 1: the request's user turns are a prefix of one script's user turns.
    fn match_conversation_prefix(&self, key: &SelectionKey) -> Option<Selection> {
        if key.user_turns.is_empty() {
            return None;
        }
        let mut best: Option<(usize, usize, usize)> = None;
        for (script_index, users) in self.user_turns.iter().enumerate() {
            if users.len() < key.user_turns.len() {
                continue;
            }
            if users[..key.user_turns.len()] != key.user_turns[..] {
                continue;
            }
            let matched = key.user_turns.len();
            let reply = self.replies[script_index][matched - 1];
            let better = best.is_none_or(|(_, best_len, _)| matched > best_len);
            if better {
                best = Some((script_index, matched, reply));
            }
        }
        best.map(|(script_index, _, reply)| {
            self.selection(script_index, reply, MatchKind::ConversationPrefix)
        })
    }

    /// Level 2: the request's trailing user turns appear contiguously in a script.
    fn match_suffix(&self, key: &SelectionKey) -> Option<Selection> {
        let total = key.user_turns.len();
        if total == 0 {
            return None;
        }
        for window in (1..=total).rev() {
            let needle = &key.user_turns[total - window..];
            for (script_index, users) in self.user_turns.iter().enumerate() {
                if users.len() < window {
                    continue;
                }
                for start in 0..=users.len() - window {
                    if users[start..start + window] == needle[..] {
                        let reply = self.replies[script_index][start + window - 1];
                        let kind = if window == total && start == 0 {
                            MatchKind::ConversationPrefix
                        } else {
                            MatchKind::Suffix
                        };
                        return Some(self.selection(script_index, reply, kind));
                    }
                }
            }
        }
        None
    }

    /// Level 3: exact match on the last user message alone.
    fn match_last_user(&self, key: &SelectionKey) -> Option<Selection> {
        let last = key.last_user()?;
        self.by_last_user
            .get(last)
            .map(|(script, reply)| self.selection(*script, *reply, MatchKind::LastUser))
    }

    /// Level 4: derive a fixture from the request digest, so off-script prompts
    /// still answer the same way every time.
    fn fallback(&self, key: &SelectionKey) -> Selection {
        let digest = key.digest();
        let script_index = pick(digest, self.scripts.len().max(1));
        let script = &self.scripts[script_index];
        let reply = pick(digest >> 32, script.assistants().len().max(1));
        self.selection(script_index, reply, MatchKind::Fallback)
    }

    fn selection(&self, script_index: usize, reply: usize, kind: MatchKind) -> Selection {
        let script = &self.scripts[script_index];
        Selection {
            script_id: script.id.clone(),
            message: script.assistants()[reply.min(script.assistants().len() - 1)].clone(),
            kind,
        }
    }
}

/// The chosen reply and how it was found.
pub struct Selection {
    pub script_id: Arc<str>,
    pub message: AssistantMessage,
    pub kind: MatchKind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{ConversationScripts, DatasetRow, write_dataset};
    use crate::model::MessageContent;
    use std::borrow::Cow;
    use tiktoken_rs::cl100k_base;

    fn row(conversation: &str, turn: u32, role: &str, content: &str) -> DatasetRow<'static> {
        DatasetRow {
            conversation_id: Cow::Owned(conversation.to_string()),
            turn_index: turn,
            role: Cow::Owned(role.to_string()),
            content: Some(Cow::Owned(content.to_string())),
            content_parts: None,
            refusal: None,
            tool_calls: None,
            function_call: None,
            audio: None,
            finish_reason: Some(Cow::Borrowed("stop")),
            usage: None,
        }
    }

    fn index() -> ScriptIndex {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.parquet");
        write_dataset(
            &path,
            &[
                row("a", 0, "user", "first question"),
                row("a", 1, "assistant", "first answer"),
                row("a", 2, "user", "second question"),
                row("a", 3, "assistant", "second answer"),
                row("b", 0, "user", "other question"),
                row("b", 1, "assistant", "other answer"),
            ],
        )
        .unwrap();
        let tokenizer = Arc::new(cl100k_base().unwrap());
        let scripts = ConversationScripts::load(&path, &tokenizer).unwrap();
        ScriptIndex::build(scripts.share())
    }

    fn request(prompts: &[(&str, &str)]) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "test-model".to_string(),
            messages: prompts
                .iter()
                .map(|(role, text)| ChatCompletionRequestMessage {
                    role: match *role {
                        "assistant" => ChatRole::Assistant,
                        "system" => ChatRole::System,
                        _ => ChatRole::User,
                    },
                    content: Some(MessageContent::Text((*text).to_string())),
                    name: None,
                    tool_calls: None,
                    function_call: None,
                    audio: None,
                    refusal: None,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn reply_for(index: &ScriptIndex, prompts: &[(&str, &str)]) -> (String, MatchKind) {
        let key = SelectionKey::from_request(&request(prompts));
        let selection = index.select(&key);
        (
            selection.message.rendered_text.as_ref().to_string(),
            selection.kind,
        )
    }

    #[test]
    fn normalization_collapses_case_and_whitespace() {
        assert_eq!(normalize("  Hello   World \n"), "hello world");
        assert_eq!(normalize("HELLO WORLD"), normalize("hello  world"));
    }

    #[test]
    fn conversation_prefix_wins_over_a_bare_last_user_match() {
        let index = index();
        let (reply, kind) = reply_for(
            &index,
            &[
                ("user", "first question"),
                ("assistant", "first answer"),
                ("user", "second question"),
            ],
        );
        assert_eq!(reply, "second answer");
        assert_eq!(kind, MatchKind::ConversationPrefix);
    }

    #[test]
    fn single_turn_matches_the_scripted_answer() {
        let index = index();
        let (reply, kind) = reply_for(&index, &[("user", "First Question")]);
        assert_eq!(reply, "first answer");
        assert_eq!(kind, MatchKind::ConversationPrefix);
    }

    #[test]
    fn a_later_turn_alone_still_matches_by_suffix() {
        let index = index();
        let (reply, kind) = reply_for(&index, &[("user", "second question")]);
        assert_eq!(reply, "second answer");
        assert!(matches!(
            kind,
            MatchKind::Suffix | MatchKind::LastUser | MatchKind::ConversationPrefix
        ));
    }

    #[test]
    fn unknown_prompts_fall_back_deterministically() {
        let index = index();
        let first = reply_for(&index, &[("user", "nothing like the fixtures")]);
        let second = reply_for(&index, &[("user", "nothing like the fixtures")]);
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, MatchKind::Fallback);

        let other = reply_for(&index, &[("user", "a completely different prompt")]);
        assert_eq!(other.1, MatchKind::Fallback);
    }

    #[test]
    fn selection_never_depends_on_call_order() {
        let index = index();
        let baseline = reply_for(&index, &[("user", "unmatched prompt")]);
        for _ in 0..10 {
            let _ = reply_for(&index, &[("user", "first question")]);
            let _ = reply_for(&index, &[("user", "other question")]);
        }
        assert_eq!(reply_for(&index, &[("user", "unmatched prompt")]), baseline);
    }

    #[test]
    fn the_model_name_participates_in_the_digest() {
        let mut a = request(&[("user", "x")]);
        let mut b = a.clone();
        a.model = "model-a".to_string();
        b.model = "model-b".to_string();
        assert_ne!(
            SelectionKey::from_request(&a).digest(),
            SelectionKey::from_request(&b).digest()
        );
    }
}
