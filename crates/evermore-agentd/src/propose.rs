use evermore_common::agent::{self, PROPOSAL_CONTEXT_TOKENS, Proposal};
use evermore_common::ai::{Message, Role};
use evermore_common::operations::{self};

use crate::infer;

const MAX_ATTEMPTS: u32 = 2;
const MAX_COMPLETION_TOKENS: u32 = 512;
const MIN_COMPLETION_TOKENS: u32 = 64;

/// GBNF grammar matching the panel's proposal marker. Kept in sync with
/// `tools/adapter-training/proposal.gbnf`; the model may only emit one
/// validated proposal or a short refusal.
const PROPOSAL_GRAMMAR: &str = r#"
root ::= proposal | refusal
proposal ::= "EVERMORE_PROPOSAL: {\"operation\":\"" name "\",\"arguments\":" value "}"
refusal ::= "{\"refusal\":\"" text "\"}"
name ::= [a-z] [a-z0-9_.]*
value ::= object | array | string | number | "true" | "false" | "null"
object ::= "{" (string ":" value ("," string ":" value)*)? "}"
array ::= "[" (value ("," value)*)? "]"
string ::= "\"" char* "\""
char ::= [^"\\] | "\\" ["\\/bfnrt]
number ::= "-"? [0-9]+ ("." [0-9]+)?
text ::= [^"\\]*
"#;

/// Generate one approval-gated write proposal from a natural language request.
/// This validates against the catalogue and never submits an operation.
pub fn propose(prompt: &str) -> Result<Proposal, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("describe the change you want first".into());
    }
    if prompt.chars().count() > 4000 {
        return Err("request is too long; keep it under 4000 characters".into());
    }
    let catalog = operations::catalog();
    let instructions = agent::write_constrained_proposal_instructions(prompt, &catalog)
        .ok_or_else(|| "no approval-gated write operation matches that request".to_string())?;
    let model = infer::select_chat_model()?;

    let budget = PROPOSAL_CONTEXT_TOKENS;
    let mut messages = vec![
        Message {
            role: Role::System,
            content: instructions,
        },
        Message {
            role: Role::User,
            content: prompt.to_string(),
        },
    ];
    let mut last_error = String::new();

    for attempt in 1..=MAX_ATTEMPTS {
        let estimated = estimate_messages(&messages);
        if estimated >= budget {
            return Err(format!(
                "request needs about {estimated} context tokens, over the {budget}-token proposal budget"
            ));
        }
        let max_tokens = MAX_COMPLETION_TOKENS
            .min(budget.saturating_sub(estimated))
            .max(MIN_COMPLETION_TOKENS);
        let result = infer::chat(
            &model,
            messages.clone(),
            Some(PROPOSAL_GRAMMAR.to_string()),
            max_tokens,
        )?;
        if result
            .completion_tokens
            .is_some_and(|tokens| tokens > u64::from(max_tokens))
        {
            return Err("local model exceeded the proposal token budget".into());
        }

        let validated = agent::parse_proposal_marker(&result.text)
            .and_then(|proposal| {
                proposal.ok_or_else(|| "model did not produce a proposal".to_string())
            })
            .and_then(|draft| {
                agent::validate_write_proposal(&draft).map(|(_, arguments)| (draft, arguments))
            });
        match validated {
            Ok((draft, arguments)) => {
                return Ok(Proposal {
                    operation: draft.operation,
                    arguments,
                    model,
                    attempts: attempt,
                    prompt_tokens: result.prompt_tokens,
                    completion_tokens: result.completion_tokens,
                    context_budget: budget,
                    triggering_prompt: prompt.to_string(),
                });
            }
            Err(error) if error.starts_with("Model declined:") => return Err(error),
            Err(error) => {
                last_error = error;
                if attempt == MAX_ATTEMPTS {
                    break;
                }
                messages.push(Message {
                    role: Role::Assistant,
                    content: result.text,
                });
                messages.push(Message {
                    role: Role::User,
                    content: format!(
                        "That proposal was rejected: {last_error}. Return one corrected EVERMORE_PROPOSAL line using only the catalogue above, or a refusal object."
                    ),
                });
            }
        }
    }

    Err(format!(
        "could not produce a valid proposal after {MAX_ATTEMPTS} attempts: {last_error}"
    ))
}

fn estimate_messages(messages: &[Message]) -> u32 {
    messages
        .iter()
        .map(|message| estimate_tokens(&message.content).saturating_add(8))
        .sum()
}

fn estimate_tokens(text: &str) -> u32 {
    (text.chars().count() as u32 / 4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_is_embedded_and_pins_the_marker() {
        assert!(PROPOSAL_GRAMMAR.contains("EVERMORE_PROPOSAL:"));
        assert!(PROPOSAL_GRAMMAR.contains("refusal"));
    }

    #[test]
    fn estimates_leave_room_under_the_budget() {
        let messages = vec![Message {
            role: Role::System,
            content: "x".repeat(400),
        }];
        assert_eq!(estimate_messages(&messages), 108);
    }
}
