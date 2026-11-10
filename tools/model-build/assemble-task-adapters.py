#!/usr/bin/env python3
"""Assemble separately routed Evermore MCP and panel adapter candidates.

This writes two ordinary inferd model-store entries that share one base GGUF.
The outputs live in a candidate directory and are not release/default-store
packages. Each candidate must pass its task-specific development diagnostic.
"""

import argparse
import hashlib
import json
import os
import shutil
from pathlib import Path


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_candidate(task: str, args) -> tuple[dict, dict, Path, Path, Path]:
    dataset_path = getattr(args, f"{task}_dataset_manifest")
    config_path = getattr(args, f"{task}_config")
    report_path = getattr(args, f"{task}_eval_report")
    adapter_path = getattr(args, f"{task}_adapter")
    dataset = json.loads(dataset_path.read_text())
    report = json.loads(report_path.read_text())
    adapter_path = adapter_path.resolve()

    if report.get("evaluation_split") != "dev":
        raise SystemExit(f"{task}: candidate selection must use a dev report")
    if report.get("dataset_sha256") != dataset.get("dataset_sha256"):
        raise SystemExit(f"{task}: evaluation and dataset hashes do not match")
    if report.get("catalogue_fingerprint") != dataset.get("catalogue_fingerprint"):
        raise SystemExit(f"{task}: evaluation and catalogue fingerprints do not match")
    if report.get("split_sha256") != dataset.get("split_sha256", {}).get("dev"):
        raise SystemExit(f"{task}: evaluation did not use the declared dev diagnostic")
    if len(report.get("runs", [])) != 1:
        raise SystemExit(f"{task}: expected exactly one inference run")

    scores = report["runs"][0]
    if task == "mcp":
        gates = {
            "mcp_listed_tool_rate": 0.95,
            "mcp_correct_tool_schema_rate": 0.95,
            "mcp_refusal_accuracy": 0.95,
        }
    else:
        gates = {
            "panel_write_accuracy": 0.95,
            "panel_schema_valid_rate": 0.95,
            "panel_refusal_accuracy": 0.95,
        }
    failures = [
        f"{metric}={scores.get(metric)} < {threshold}"
        for metric, threshold in gates.items()
        if scores.get(metric) is None or scores[metric] < threshold
    ]
    if failures:
        raise SystemExit(f"{task}: dev diagnostic does not pass: " + "; ".join(failures))
    if task == "mcp" and (not scores.get("mcp_grammar_enforced") or scores.get("mcp_unlisted_calls") != 0):
        raise SystemExit("mcp: grammar must be enforced and there must be no unlisted calls")
    if task == "panel" and not scores.get("grammar_enforced"):
        raise SystemExit("panel: proposal grammar must be enforced")

    return dataset, report, adapter_path, config_path, report_path


def link_or_copy(source: Path, target: Path) -> None:
    try:
        os.link(source, target)
    except OSError:
        shutil.copy2(source, target)


def assemble(
    task: str,
    candidate: tuple,
    base: Path,
    base_hash: str,
    training_weights_hash: str,
    output: Path,
) -> None:
    dataset, report, adapter, config, report_path = candidate
    scores = report["runs"][0]
    package_id = f"p8.model.qwen2.5-1.5b.{task}"
    package_version = "1.0.0-dev.1"
    model_dir = output / f"{package_id}-{package_version}"
    model_dir.mkdir(parents=True)
    link_or_copy(base, model_dir / "base.gguf")
    shutil.copy2(adapter, model_dir / "adapter.gguf")

    config_sha = sha256(config)
    adapter_hash = sha256(model_dir / "adapter.gguf")
    training = {
        "base_model_sha256": training_weights_hash,
        "dataset_sha256": dataset["dataset_sha256"],
        "config_sha256": config_sha,
        "eval_report_sha256": sha256(report_path),
        "catalogue_fingerprint": dataset["catalogue_fingerprint"],
        "contributed_traces": bool(dataset.get("traces", {}).get("included")),
    }
    manifest = {
        "manifest_version": 1,
        "package_id": package_id,
        "package_version": package_version,
        "description": f"Qwen2.5-1.5B with Evermore {task.upper()} tool-use adapter (development candidate)",
        "base": {
            "file": "base.gguf",
            "sha256": base_hash,
            "size_bytes": (model_dir / "base.gguf").stat().st_size,
        },
        "adapter": {
            "file": "adapter.gguf",
            "sha256": adapter_hash,
            "size_bytes": (model_dir / "adapter.gguf").stat().st_size,
            "scale": 1.0,
        },
        "context_size": 2048,
        "quant": "Q4_K_M",
        "capabilities": ["chat"],
        "operation_registry_version": dataset["catalogue_fingerprint"],
        "training": training,
    }
    (model_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    task_metrics = (
        ["mcp_listed_tool_rate", "mcp_correct_tool_schema_rate", "mcp_refusal_accuracy"]
        if task == "mcp"
        else ["panel_write_accuracy", "panel_schema_valid_rate", "panel_refusal_accuracy"]
    )
    metadata = {
        "candidate_task": task,
        "selected_adapter": adapter.name,
        "adapter_sha256": adapter_hash,
        "base_sha256": base_hash,
        "base_model_id": "Qwen/Qwen2.5-1.5B-Instruct",
        "base_model_revision": "989aa7980e4cf806f80c7fef2b1adb7bc71aa306",
        "training_base_weights_sha256": training_weights_hash,
        "training_task": task,
        "training_config_sha256": config_sha,
        "dataset_version": dataset.get("dataset_version"),
        "dataset_sha256": dataset["dataset_sha256"],
        "catalogue_fingerprint": dataset["catalogue_fingerprint"],
        "evaluation_split": report["evaluation_split"],
        "evaluation_records": report["records"],
        "evaluation_split_sha256": report["split_sha256"],
        "evaluation_report_sha256": training["eval_report_sha256"],
        "metrics": {key: scores[key] for key in task_metrics},
        "release_eligible": False,
        "note": "Small task-specific dev diagnostic only; full development and locked-test production-path evaluation is still required.",
    }
    (model_dir / "candidate.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"assembled {task} candidate: {model_dir}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True, type=Path)
    parser.add_argument("--training-base-weights", required=True, type=Path)
    parser.add_argument("--mcp-adapter", required=True, type=Path)
    parser.add_argument("--mcp-dataset-manifest", required=True, type=Path)
    parser.add_argument("--mcp-config", required=True, type=Path)
    parser.add_argument("--mcp-eval-report", required=True, type=Path)
    parser.add_argument("--panel-adapter", required=True, type=Path)
    parser.add_argument("--panel-dataset-manifest", required=True, type=Path)
    parser.add_argument("--panel-config", required=True, type=Path)
    parser.add_argument("--panel-eval-report", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()

    base = args.base.resolve()
    if not base.is_file():
        raise SystemExit(f"base GGUF not found: {base}")
    training_weights = args.training_base_weights.resolve()
    if not training_weights.is_file():
        raise SystemExit(f"training base weights not found: {training_weights}")
    output = args.output_dir.resolve()
    if output.exists() and any(output.iterdir()):
        raise SystemExit(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    base_hash = sha256(base)
    training_weights_hash = sha256(training_weights)
    mcp = load_candidate("mcp", args)
    panel = load_candidate("panel", args)
    assemble("mcp", mcp, base, base_hash, training_weights_hash, output)
    assemble("panel", panel, base, base_hash, training_weights_hash, output)


if __name__ == "__main__":
    main()
