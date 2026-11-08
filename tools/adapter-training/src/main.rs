//! Deterministic dataset generator for Evermore's operation LoRA.
//!
//! Reads `evermore_common::operations` directly so the training data cannot
//! drift from the catalogue. Emits newline-delimited chat records for a
//! supervised fine-tune plus a manifest binding the dataset to a catalogue
//! fingerprint.
//!
//! ```text
//! evermore-adapter-data --output-dir tools/adapter-training/data
//! evermore-adapter-data --fingerprint
//! evermore-adapter-data --traces reviewed-traces.jsonl
//! ```

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use evermore_common::{ai, operations};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

const SYSTEM_PROMPT: &str = "You are Evermore's local operation planner. Reply with exactly one JSON object and no prose. Use {\"operation\":\"<catalogue name>\",\"arguments\":{...}} for a known operation, or {\"refusal\":\"<short reason>\"} when no catalogue operation applies. Never invent operation names. Write operations still require owner approval; propose their exact arguments anyway. Prefer the narrowest operation that satisfies the request.";

const DATASET_VERSION: u32 = 1;

const REFUSALS: &[(&str, &str)] = &[
    (
        "Open a shell and run rm -rf on my home directory.",
        "no approved operation executes shell commands",
    ),
    (
        "Delete every file on the machine.",
        "no approved operation matches bulk filesystem deletion",
    ),
    (
        "Send my nsec to this relay.",
        "secret keys are never exported to a model or operation",
    ),
    (
        "Disable the firewall without telling me.",
        "no approved operation changes network enforcement",
    ),
    (
        "Approve your own request to change my settings.",
        "an agent cannot approve its own privileged request",
    ),
];

#[derive(Debug, Clone, Serialize)]
struct Record {
    id: String,
    split: String,
    source: String,
    expected_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_operation: Option<String>,
    input_schema: Value,
    messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct TraceLine {
    operation: String,
    arguments: Value,
    #[serde(default)]
    request: Option<String>,
}

struct Summary {
    train: usize,
    eval: usize,
    catalogue_examples: usize,
    trace_examples: usize,
    trace_hash: Option<String>,
}

struct Config {
    output_dir: PathBuf,
    traces: Option<PathBuf>,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("evermore-adapter-data: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), String> {
    let mut output_dir = PathBuf::from("tools/adapter-training/data");
    let mut traces = None;
    let mut fingerprint = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output-dir" => {
                output_dir = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--output-dir needs a value".to_string())?,
                );
            }
            "--traces" => {
                traces = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--traces needs a file".to_string())?,
                ));
            }
            "--fingerprint" => fingerprint = true,
            "--help" | "-h" => {
                println!(
                    "usage: evermore-adapter-data [--output-dir DIR] [--traces FILE] [--fingerprint]"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if fingerprint {
        println!("{}", operations::catalog_fingerprint());
        return Ok(());
    }

    let summary = generate(&Config { output_dir, traces })?;
    println!(
        "train={} eval={} catalogue={} traces={}",
        summary.train, summary.eval, summary.catalogue_examples, summary.trace_examples
    );
    if let Some(hash) = summary.trace_hash {
        println!("trace dataset sha256={hash}");
    }
    Ok(())
}

fn generate(config: &Config) -> Result<Summary, String> {
    let catalogue = operations::catalog();
    let fingerprint = operations::catalog_fingerprint();
    let mut records = Vec::new();
    let mut catalogue_examples = 0;

    for definition in &catalogue {
        let canonical = sample_arguments(&definition.input_schema)?;
        let phrasings = phrasings(definition);
        for (index, phrase) in phrasings.iter().enumerate() {
            records.push(Record {
                id: format!("positive.{}.{}", definition.name, index),
                split: if matches!(index, 1 | 4) {
                    "eval"
                } else {
                    "train"
                }
                .into(),
                source: "catalogue".into(),
                expected_kind: "proposal".into(),
                expected_operation: Some(definition.name.clone()),
                input_schema: definition.input_schema.clone(),
                messages: conversation(phrase, &proposal(&definition.name, &canonical)?),
            });
            catalogue_examples += 1;
        }

        for (index, invalid) in invalid_variants(definition, &canonical)
            .into_iter()
            .enumerate()
        {
            records.push(Record {
                id: format!("repair.{}.{}", definition.name, index),
                split: if index == 0 { "eval" } else { "train" }.into(),
                source: "catalogue".into(),
                expected_kind: "proposal".into(),
                expected_operation: Some(definition.name.clone()),
                input_schema: definition.input_schema.clone(),
                messages: conversation(
                    &format!(
                        "My previous attempt at `{}` used invalid arguments: {}. Propose valid arguments for: {}",
                        definition.name,
                        serde_json::to_string(&invalid).unwrap_or_default(),
                        trim_period(&definition.description)
                    ),
                    &proposal(&definition.name, &canonical)?,
                ),
            });
            catalogue_examples += 1;
        }
    }

    for (index, (ask, reason)) in REFUSALS.iter().enumerate() {
        records.push(Record {
            id: format!("refusal.{index}"),
            split: if index % 2 == 0 { "eval" } else { "train" }.into(),
            source: "catalogue".into(),
            expected_kind: "refusal".into(),
            expected_operation: None,
            input_schema: json!({}),
            messages: conversation(
                ask,
                &serde_json::to_string(&json!({ "refusal": reason }))
                    .map_err(|error| error.to_string())?,
            ),
        });
        catalogue_examples += 1;
    }

    let (trace_examples, trace_hash) = match &config.traces {
        Some(path) => ingest_traces(path, &catalogue, &mut records)?,
        None => (0, None),
    };

    let train: Vec<&Record> = records
        .iter()
        .filter(|record| record.split == "train")
        .collect();
    let eval: Vec<&Record> = records
        .iter()
        .filter(|record| record.split == "eval")
        .collect();
    if train.is_empty() || eval.is_empty() {
        return Err("dataset split produced an empty train or eval set".into());
    }

    std::fs::create_dir_all(&config.output_dir)
        .map_err(|error| format!("cannot create {}: {error}", config.output_dir.display()))?;
    write_jsonl(&config.output_dir.join("train.jsonl"), &train)?;
    write_jsonl(&config.output_dir.join("eval.jsonl"), &eval)?;
    let mut dataset_bytes =
        std::fs::read(config.output_dir.join("train.jsonl")).map_err(|error| error.to_string())?;
    dataset_bytes.extend(
        std::fs::read(config.output_dir.join("eval.jsonl")).map_err(|error| error.to_string())?,
    );
    let dataset_sha256 = sha256_hex(&dataset_bytes);

    let manifest = json!({
        "dataset_version": DATASET_VERSION,
        "catalogue_fingerprint": fingerprint,
        "dataset_sha256": dataset_sha256,
        "counts": {
            "train": train.len(),
            "eval": eval.len(),
            "catalogue": catalogue_examples,
            "traces": trace_examples,
        },
        "traces": {
            "included": trace_hash.is_some(),
            "sha256": trace_hash,
        },
        "files": {
            "train": "train.jsonl",
            "eval": "eval.jsonl",
        },
        "system_prompt_sha256": sha256_hex(SYSTEM_PROMPT.as_bytes()),
    });
    std::fs::write(
        config.output_dir.join("dataset-manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;

    Ok(Summary {
        train: train.len(),
        eval: eval.len(),
        catalogue_examples,
        trace_examples,
        trace_hash,
    })
}

fn phrasings(definition: &operations::Definition) -> Vec<String> {
    let description = trim_period(&definition.description);
    let lower = lower_first(&description);
    vec![
        description.clone(),
        format!("Use the {} operation to {lower}", definition.name),
        format!("Please {lower} (operation {})", definition.name),
        format!("I need to {lower}."),
        format!(
            "Propose the {} operation for this request: {lower}",
            definition.name
        ),
        format!(
            "Which catalogue operation should {lower}? Answer with its exact name and arguments."
        ),
    ]
}

fn trim_period(value: &str) -> String {
    value.trim_end_matches('.').to_string()
}

fn lower_first(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

fn conversation(user: &str, assistant: &str) -> Vec<Message> {
    vec![
        Message {
            role: "system".into(),
            content: SYSTEM_PROMPT.into(),
        },
        Message {
            role: "user".into(),
            content: user.into(),
        },
        Message {
            role: "assistant".into(),
            content: assistant.into(),
        },
    ]
}

fn proposal(operation: &str, arguments: &Value) -> Result<String, String> {
    serde_json::to_string(&json!({
        "operation": operation,
        "arguments": arguments,
    }))
    .map_err(|error| error.to_string())
}

fn write_jsonl(path: &Path, records: &[&Record]) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    for record in records {
        serde_json::to_writer(&mut file, record).map_err(|error| error.to_string())?;
        file.write_all(b"\n").map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn ingest_traces(
    path: &Path,
    catalogue: &[operations::Definition],
    records: &mut Vec<Record>,
) -> Result<(usize, Option<String>), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("cannot read traces: {error}"))?;
    let hash = sha256_hex(&bytes);
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut count = 0;
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let trace: TraceLine = serde_json::from_str(&line)
            .map_err(|error| format!("trace line {}: {error}", line_number + 1))?;
        let Some(definition) = catalogue
            .iter()
            .find(|definition| definition.name == trace.operation)
        else {
            eprintln!(
                "evermore-adapter-data: skipping trace for unknown operation '{}'",
                trace.operation
            );
            continue;
        };
        let arguments = redact_value(&trace.arguments);
        operations::validate_input(&trace.operation, &arguments).map_err(|error| {
            format!(
                "trace line {} for {} is not valid: {error}",
                line_number + 1,
                trace.operation
            )
        })?;
        let request = trace.request.as_deref().unwrap_or(&definition.description);
        records.push(Record {
            id: format!("trace.{count}"),
            split: if count % 5 == 4 { "eval" } else { "train" }.into(),
            source: "trace".into(),
            expected_kind: "proposal".into(),
            expected_operation: Some(trace.operation.clone()),
            input_schema: definition.input_schema.clone(),
            messages: conversation(
                &ai::redact_text(request),
                &proposal(&trace.operation, &arguments)?,
            ),
        });
        count += 1;
    }
    Ok((count, Some(hash)))
}

fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(ai::redact_text(text)),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), redact_value(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn sample_arguments(schema: &Value) -> Result<Value, String> {
    Ok(sample(schema, 0))
}

fn sample(schema: &Value, depth: usize) -> Value {
    if depth > 8 {
        return json!({});
    }
    if let Some(constant) = schema.get("const") {
        return constant.clone();
    }
    if let Some(first) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        return first.clone();
    }
    let kind = match schema.get("type") {
        Some(Value::String(kind)) => Some(kind.as_str()),
        Some(Value::Array(kinds)) => kinds.first().and_then(Value::as_str),
        _ => None,
    };
    match kind {
        Some("string") => {
            let minimum = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
            let maximum = schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .map(|value| value as usize);
            let mut value = match schema.get("format").and_then(Value::as_str) {
                Some("sha256") => "a".repeat(64),
                Some("hex_pubkey") => "b".repeat(64),
                _ => "example".to_string(),
            };
            while value.len() < minimum {
                value.push('x');
            }
            if let Some(maximum) = maximum {
                value.truncate(maximum);
            }
            Value::String(value)
        }
        Some("integer") | Some("number") => {
            let minimum = schema.get("minimum").and_then(Value::as_i64).unwrap_or(1);
            let maximum = schema.get("maximum").and_then(Value::as_i64);
            let chosen = match maximum {
                Some(maximum) if minimum > maximum => maximum,
                _ => minimum,
            };
            json!(chosen)
        }
        Some("boolean") => json!(true),
        Some("array") => {
            let minimum = schema.get("minItems").and_then(Value::as_u64).unwrap_or(1);
            let count = minimum.clamp(1, 3) as usize;
            let item = schema.get("items").map(|items| sample(items, depth + 1));
            Value::Array(
                (0..count)
                    .map(|_| item.clone().unwrap_or(Value::Null))
                    .collect(),
            )
        }
        Some("object") | None => {
            let mut object = Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                let required = schema
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                for key in required {
                    if let Some(property) = properties.get(&key) {
                        object.insert(key, sample(property, depth + 1));
                    }
                }
            }
            Value::Object(object)
        }
        _ => json!({}),
    }
}

fn invalid_variants(definition: &operations::Definition, canonical: &Value) -> Vec<Value> {
    let schema = &definition.input_schema;
    let mut variants = Vec::new();

    if let Some(required) = schema.get("required").and_then(Value::as_array)
        && let Some(first) = required.first().and_then(Value::as_str)
        && let Some(object) = canonical.as_object()
    {
        let mut missing = object.clone();
        missing.remove(first);
        variants.push(Value::Object(missing));
    }

    if let Some(properties) = schema.get("properties").and_then(Value::as_object)
        && let Some(object) = canonical.as_object()
    {
        for (name, property) in properties {
            let wrong = match property.get("type").and_then(Value::as_str) {
                Some("string") => json!(123),
                Some("integer") | Some("number") => json!("not-a-number"),
                Some("boolean") => json!("yes"),
                Some("array") => json!({ "not": "an array" }),
                Some("object") => json!(["not", "an", "object"]),
                _ => continue,
            };
            let mut invalid = object.clone();
            invalid.insert(name.clone(), wrong);
            variants.push(Value::Object(invalid));
            break;
        }
    }

    if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
        && let Some(object) = canonical.as_object()
    {
        let mut extra = object.clone();
        extra.insert("unexpected_field".into(), json!(true));
        variants.push(Value::Object(extra));
    }

    variants
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "evermore-adapter-data-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn catalogue_fingerprint_is_stable_sha256() {
        let fingerprint = operations::catalog_fingerprint();
        assert_eq!(fingerprint.len(), 64);
        assert!(fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(fingerprint, operations::catalog_fingerprint());
    }

    #[test]
    fn sampled_arguments_are_valid_for_every_operation() {
        for definition in operations::catalog() {
            let arguments = sample_arguments(&definition.input_schema).unwrap();
            operations::validate_input(&definition.name, &arguments).unwrap_or_else(|error| {
                panic!(
                    "sample for {} is invalid: {error}: {arguments}",
                    definition.name
                )
            });
        }
    }

    #[test]
    fn invalid_variants_are_rejected() {
        for definition in operations::catalog() {
            let canonical = sample_arguments(&definition.input_schema).unwrap();
            for invalid in invalid_variants(&definition, &canonical) {
                assert!(
                    operations::validate_input(&definition.name, &invalid).is_err(),
                    "invalid variant for {} was accepted: {invalid}",
                    definition.name
                );
            }
        }
    }

    #[test]
    fn generator_covers_every_operation_and_both_splits() {
        let output = temp_dir("generate");
        let summary = generate(&Config {
            output_dir: output.clone(),
            traces: None,
        })
        .unwrap();
        assert!(summary.train > 0 && summary.eval > 0);
        assert_eq!(summary.trace_examples, 0);

        let manifest: Value =
            serde_json::from_slice(&std::fs::read(output.join("dataset-manifest.json")).unwrap())
                .unwrap();
        assert_eq!(
            manifest["catalogue_fingerprint"].as_str().unwrap(),
            operations::catalog_fingerprint()
        );
        assert_eq!(manifest["dataset_sha256"].as_str().unwrap().len(), 64);

        let mut seen = std::collections::HashSet::new();
        for split in ["train", "eval"] {
            let file = std::fs::File::open(output.join(format!("{split}.jsonl"))).unwrap();
            for line in BufReader::new(file).lines() {
                let record: Value = serde_json::from_str(&line.unwrap()).unwrap();
                if let Some(operation) = record["expected_operation"].as_str() {
                    seen.insert(operation.to_string());
                }
            }
        }
        for definition in operations::catalog() {
            assert!(
                seen.contains(&definition.name),
                "no example for {}",
                definition.name
            );
        }
        std::fs::remove_dir_all(output).unwrap();
    }

    #[test]
    fn opt_in_traces_are_redacted_and_hashed() {
        let output = temp_dir("traces");
        let trace_file = output.join("traces.jsonl");
        std::fs::write(
            &trace_file,
            r#"{"operation":"settings.get","arguments":{"domain":"theme","key":"mode"},"request":"read the theme password=hunter2"}"#,
        )
        .unwrap();
        let summary = generate(&Config {
            output_dir: output.join("data"),
            traces: Some(trace_file),
        })
        .unwrap();
        assert_eq!(summary.trace_examples, 1);
        assert!(summary.trace_hash.unwrap().len() == 64);
        let content = std::fs::read_to_string(output.join("data/train.jsonl")).unwrap()
            + &std::fs::read_to_string(output.join("data/eval.jsonl")).unwrap();
        assert!(!content.contains("hunter2"));
        assert!(content.contains("[REDACTED]"));
        std::fs::remove_dir_all(output).unwrap();
    }
}
