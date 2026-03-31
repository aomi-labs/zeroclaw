use crate::providers::traits::ChatMessage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

fn default_max_tokens() -> usize {
    8192
}

fn default_keep_recent() -> usize {
    4
}

fn default_collapse() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HistoryPrunerConfig {
    /// Enable history pruning. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum estimated tokens for message history. Default: 8192.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    /// Keep the N most recent messages untouched. Default: 4.
    #[serde(default = "default_keep_recent")]
    pub keep_recent: usize,
    /// Collapse old tool call/result pairs into short summaries. Default: true.
    #[serde(default = "default_collapse")]
    pub collapse_tool_results: bool,
}

impl Default for HistoryPrunerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_tokens: 8192,
            keep_recent: 4,
            collapse_tool_results: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneStats {
    pub messages_before: usize,
    pub messages_after: usize,
    pub collapsed_pairs: usize,
    pub dropped_messages: usize,
}

// ---------------------------------------------------------------------------
// Token estimation
// ---------------------------------------------------------------------------

fn estimate_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(|m| m.content.len() / 4).sum()
}

// ---------------------------------------------------------------------------
// Protected-index helpers
// ---------------------------------------------------------------------------

fn protected_indices(messages: &[ChatMessage], keep_recent: usize) -> Vec<bool> {
    let len = messages.len();
    let mut protected = vec![false; len];
    for (i, msg) in messages.iter().enumerate() {
        if msg.role == "system" {
            protected[i] = true;
        }
    }
    let recent_start = len.saturating_sub(keep_recent);
    for p in protected.iter_mut().skip(recent_start) {
        *p = true;
    }
    protected
}

// ---------------------------------------------------------------------------
// Tool-pair-aware helpers
// ---------------------------------------------------------------------------

/// Count consecutive `tool` messages starting at `start_idx`.
fn count_tool_results(messages: &[ChatMessage], start_idx: usize) -> usize {
    messages[start_idx..]
        .iter()
        .take_while(|m| m.role == "tool")
        .count()
}

/// Check if ALL messages in range [start..start+count) are unprotected.
fn all_unprotected(protected: &[bool], start: usize, count: usize) -> bool {
    protected[start..start + count].iter().all(|&p| !p)
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn prune_history(messages: &mut Vec<ChatMessage>, config: &HistoryPrunerConfig) -> PruneStats {
    let messages_before = messages.len();
    if !config.enabled || messages.is_empty() {
        return PruneStats {
            messages_before,
            messages_after: messages_before,
            collapsed_pairs: 0,
            dropped_messages: 0,
        };
    }

    let mut collapsed_pairs: usize = 0;

    // Phase 1 – collapse assistant + ALL consecutive tool results as a group.
    // An assistant message with N tool_use calls is followed by N tool results.
    // We must collapse the entire group (assistant + N tools) into a single
    // summary assistant message, never leaving orphaned tool results.
    if config.collapse_tool_results {
        let mut i = 0;
        while i < messages.len() {
            let protected = protected_indices(messages, config.keep_recent);
            if messages[i].role == "assistant" && !protected[i] {
                // Count consecutive tool results after this assistant
                let tool_count = count_tool_results(messages, i + 1);
                if tool_count > 0
                    && all_unprotected(&protected, i, 1 + tool_count)
                {
                    // Build summary from all tool results
                    let mut summaries = Vec::with_capacity(tool_count);
                    for j in 0..tool_count {
                        let tool_content = &messages[i + 1 + j].content;
                        let truncated: String = tool_content.chars().take(80).collect();
                        summaries.push(truncated);
                    }
                    let summary = if summaries.len() == 1 {
                        format!("[Tool result: {}...]", summaries[0])
                    } else {
                        format!(
                            "[{} tool results: {}...]",
                            summaries.len(),
                            summaries.join(" | ")
                        )
                    };
                    // Replace assistant with summary, remove ALL tool results
                    messages[i] = ChatMessage {
                        role: "assistant".to_string(),
                        content: summary,
                    };
                    for _ in 0..tool_count {
                        messages.remove(i + 1);
                    }
                    collapsed_pairs += 1;
                    // Don't advance i — re-check this position
                    continue;
                }
            }
            i += 1;
        }
    }

    // Phase 2 – budget enforcement (tool-pair-aware).
    // Never drop an assistant message without also dropping its tool results,
    // and never drop a tool result without also dropping its assistant.
    let mut dropped_messages: usize = 0;
    while estimate_tokens(messages) > config.max_tokens {
        let protected = protected_indices(messages, config.keep_recent);

        // Find the first unprotected droppable group.
        let mut found = false;
        let mut i = 0;
        while i < messages.len() {
            if protected[i] {
                i += 1;
                continue;
            }
            // If this is an assistant followed by tool results, drop the whole group
            if messages[i].role == "assistant" {
                let tool_count = count_tool_results(messages, i + 1);
                if tool_count > 0 && all_unprotected(&protected, i, 1 + tool_count) {
                    for _ in 0..=tool_count {
                        messages.remove(i);
                        dropped_messages += 1;
                    }
                    found = true;
                    break;
                }
            }
            // If this is a standalone tool result (shouldn't happen, but be safe),
            // skip it — don't orphan it. Look for non-tool messages to drop instead.
            if messages[i].role == "tool" {
                i += 1;
                continue;
            }
            // Drop standalone non-tool, non-assistant-with-tools messages
            messages.remove(i);
            dropped_messages += 1;
            found = true;
            break;
        }
        if !found {
            break;
        }
    }

    PruneStats {
        messages_before,
        messages_after: messages.len(),
        collapsed_pairs,
        dropped_messages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn prune_disabled_is_noop() {
        let mut messages = vec![
            msg("system", "You are helpful."),
            msg("user", "Hello"),
            msg("assistant", "Hi there!"),
        ];
        let config = HistoryPrunerConfig {
            enabled: false,
            ..Default::default()
        };
        let stats = prune_history(&mut messages, &config);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content, "You are helpful.");
        assert_eq!(stats.messages_before, 3);
        assert_eq!(stats.messages_after, 3);
        assert_eq!(stats.collapsed_pairs, 0);
    }

    #[test]
    fn prune_under_budget_no_change() {
        let mut messages = vec![
            msg("system", "You are helpful."),
            msg("user", "Hello"),
            msg("assistant", "Hi!"),
        ];
        let config = HistoryPrunerConfig {
            enabled: true,
            max_tokens: 8192,
            keep_recent: 2,
            collapse_tool_results: false,
        };
        let stats = prune_history(&mut messages, &config);
        assert_eq!(messages.len(), 3);
        assert_eq!(stats.collapsed_pairs, 0);
        assert_eq!(stats.dropped_messages, 0);
    }

    #[test]
    fn prune_collapses_tool_pairs() {
        let tool_result = "a".repeat(160);
        let mut messages = vec![
            msg("system", "sys"),
            msg("assistant", "calling tool X"),
            msg("tool", &tool_result),
            msg("user", "thanks"),
            msg("assistant", "done"),
        ];
        let config = HistoryPrunerConfig {
            enabled: true,
            max_tokens: 100_000,
            keep_recent: 2,
            collapse_tool_results: true,
        };
        let stats = prune_history(&mut messages, &config);
        assert_eq!(stats.collapsed_pairs, 1);
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1].role, "assistant");
        assert!(messages[1].content.starts_with("[Tool result: "));
    }

    #[test]
    fn prune_collapses_multi_tool_pairs() {
        // assistant makes 2 tool calls -> 2 tool results
        let mut messages = vec![
            msg("system", "sys"),
            msg("assistant", "calling tools A and B"),
            msg("tool", "result A with lots of content here"),
            msg("tool", "result B with lots of content here"),
            msg("user", "thanks"),
            msg("assistant", "done"),
        ];
        let config = HistoryPrunerConfig {
            enabled: true,
            max_tokens: 100_000,
            keep_recent: 2,
            collapse_tool_results: true,
        };
        let stats = prune_history(&mut messages, &config);
        assert_eq!(stats.collapsed_pairs, 1);
        assert_eq!(messages.len(), 4); // system, collapsed_assistant, user, assistant
        assert!(messages[1].content.contains("2 tool results"));
        // Verify no orphaned tool messages remain
        assert!(!messages.iter().any(|m| m.role == "tool"));
    }

    #[test]
    fn prune_preserves_system_and_recent() {
        let big = "x".repeat(40_000);
        let mut messages = vec![
            msg("system", "system prompt"),
            msg("user", &big),
            msg("assistant", "old reply"),
            msg("user", "recent1"),
            msg("assistant", "recent2"),
        ];
        let config = HistoryPrunerConfig {
            enabled: true,
            max_tokens: 100,
            keep_recent: 2,
            collapse_tool_results: false,
        };
        let stats = prune_history(&mut messages, &config);
        assert!(messages.iter().any(|m| m.role == "system"));
        assert!(messages.iter().any(|m| m.content == "recent1"));
        assert!(messages.iter().any(|m| m.content == "recent2"));
        assert!(stats.dropped_messages > 0);
    }

    #[test]
    fn prune_drops_oldest_when_over_budget() {
        let filler = "y".repeat(400);
        let mut messages = vec![
            msg("system", "sys"),
            msg("user", &filler),
            msg("assistant", &filler),
            msg("user", "recent-user"),
            msg("assistant", "recent-assistant"),
        ];
        let config = HistoryPrunerConfig {
            enabled: true,
            max_tokens: 150,
            keep_recent: 2,
            collapse_tool_results: false,
        };
        let stats = prune_history(&mut messages, &config);
        assert!(stats.dropped_messages >= 1);
        assert_eq!(messages[0].role, "system");
        assert!(messages.iter().any(|m| m.content == "recent-user"));
        assert!(messages.iter().any(|m| m.content == "recent-assistant"));
    }

    #[test]
    fn prune_drops_tool_group_atomically() {
        // When budget-dropping, assistant + its tool results must go together
        let big_result = "z".repeat(2000);
        let mut messages = vec![
            msg("system", "sys"),
            msg("assistant", "calling tool"),
            msg("tool", &big_result),
            msg("tool", &big_result),
            msg("user", "recent"),
            msg("assistant", "recent reply"),
        ];
        let config = HistoryPrunerConfig {
            enabled: true,
            max_tokens: 100,
            keep_recent: 2,
            collapse_tool_results: false, // skip phase 1
        };
        let stats = prune_history(&mut messages, &config);
        // The assistant + 2 tool results should be dropped as a group
        assert!(!messages.iter().any(|m| m.role == "tool"),
            "No orphaned tool messages should remain");
        assert!(stats.dropped_messages >= 3);
    }

    #[test]
    fn prune_empty_messages() {
        let mut messages: Vec<ChatMessage> = vec![];
        let config = HistoryPrunerConfig {
            enabled: true,
            ..Default::default()
        };
        let stats = prune_history(&mut messages, &config);
        assert_eq!(stats.messages_before, 0);
        assert_eq!(stats.messages_after, 0);
    }
}
