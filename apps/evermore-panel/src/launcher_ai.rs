//! One-shot, warm-model launcher suggestions. This path never starts a cold
//! model and never creates or saves an AI chat session.

use evermore_common::ai::{Message, Role};
use std::io::Write;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const FIRST_TOKEN_BUDGET: Duration = Duration::from_millis(300);
const TOTAL_BUDGET: Duration = Duration::from_secs(3);

pub struct SuggestionTask {
    pub preview: String,
    answer: Receiver<Option<String>>,
    deadline: Instant,
    cancellation: crate::ai_infer_client::ChatCancellation,
}

impl SuggestionTask {
    pub fn finish(&mut self) -> Option<String> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            self.cancellation.cancel();
            return None;
        }
        match self.answer.recv_timeout(remaining) {
            Ok(answer) => answer,
            Err(_) => {
                self.cancellation.cancel();
                None
            }
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}

impl Drop for SuggestionTask {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

pub fn start_if_warm(query: &str) -> Option<SuggestionTask> {
    if !is_open_ended_query(query)
        || std::process::Command::new("wl-copy")
            .arg("--version")
            .output()
            .is_err()
    {
        return None;
    }

    let started = Instant::now();
    let first_token_deadline = started + FIRST_TOKEN_BUDGET;
    let total_deadline = started + TOTAL_BUDGET;
    let preferences = crate::ai_infer_client::load_preferences();
    let models =
        crate::ai_infer_client::list_models_with_timeout(Some(Duration::from_millis(100))).ok()?;
    // Include service discovery in the launcher budget. If the local service
    // is slow to answer, skip the suggestion instead of showing a late card.
    if Instant::now() >= first_token_deadline {
        return None;
    }
    let model = preferences
        .model
        .as_deref()
        .and_then(|name| {
            models.iter().find(|model| {
                model.name == name
                    && model.loaded
                    && model.capabilities.iter().any(|cap| cap == "chat")
            })
        })
        .or_else(|| {
            models
                .iter()
                .find(|model| model.loaded && model.capabilities.iter().any(|cap| cap == "chat"))
        })?;

    let messages = vec![
        Message {
            role: Role::System,
            content: "Give one concise, useful suggestion for the user's open-ended query. Do not run commands, change system state, or claim an action was taken. Return plain text only.".into(),
        },
        Message {
            role: Role::User,
            content: evermore_common::ai::redact_text(query),
        },
    ];
    let task = crate::ai_infer_client::start_chat_if_warm(
        model.name.clone(),
        preferences.backend.unwrap_or_default(),
        messages,
    );
    let remaining = first_token_deadline.saturating_duration_since(Instant::now());
    let first_token = match task.receiver.recv_timeout(remaining) {
        Ok(crate::ai_infer_client::ChatUpdate::Token(token))
            if Instant::now() <= first_token_deadline =>
        {
            token
        }
        _ => {
            task.cancellation.cancel();
            return None;
        }
    };
    let preview = first_token.trim().to_string();
    if preview.is_empty() {
        task.cancellation.cancel();
        return None;
    }

    let (answer_sender, answer) = mpsc::channel();
    let cancellation = task.cancellation.clone();
    let worker_cancellation = task.cancellation;
    let receiver = task.receiver;
    std::thread::spawn(move || {
        let mut answer_text = first_token;
        loop {
            let remaining = total_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                worker_cancellation.cancel();
                let _ = answer_sender.send(None);
                return;
            }
            match receiver.recv_timeout(remaining) {
                Ok(crate::ai_infer_client::ChatUpdate::Token(token)) => {
                    answer_text.push_str(&token)
                }
                Ok(crate::ai_infer_client::ChatUpdate::Complete) => {
                    let answer = answer_text.trim().to_string();
                    let _ = answer_sender.send((!answer.is_empty()).then_some(answer));
                    return;
                }
                Ok(crate::ai_infer_client::ChatUpdate::Cancelled)
                | Ok(crate::ai_infer_client::ChatUpdate::Error(_))
                | Err(RecvTimeoutError::Disconnected) => {
                    let _ = answer_sender.send(None);
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {
                    worker_cancellation.cancel();
                    let _ = answer_sender.send(None);
                    return;
                }
            }
        }
    });
    Some(SuggestionTask {
        preview,
        answer,
        deadline: total_deadline,
        cancellation,
    })
}

pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut child = std::process::Command::new("wl-copy")
        .args(["--type", "text/plain"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("starting wl-copy: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "wl-copy did not open stdin".to_string())?
        .write_all(text.as_bytes())
        .map_err(|error| format!("writing local suggestion to clipboard: {error}"))?;
    let status = child
        .wait()
        .map_err(|error| format!("waiting for wl-copy: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("wl-copy could not copy the local suggestion".into())
    }
}

pub fn is_open_ended_query(query: &str) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return false;
    }
    query.ends_with('?')
        || [
            "how ",
            "why ",
            "what ",
            "where ",
            "when ",
            "who ",
            "explain ",
            "suggest ",
            "recommend ",
            "compare ",
            "summarize ",
            "draft ",
            "help me ",
            "give me ",
        ]
        .iter()
        .any(|prefix| query.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::is_open_ended_query;

    #[test]
    fn only_question_or_open_ended_language_is_eligible() {
        assert!(is_open_ended_query("Where is the telescope stored?"));
        assert!(is_open_ended_query("Explain the difference between these"));
        assert!(!is_open_ended_query("firefox"));
        assert!(!is_open_ended_query(""));
    }
}
