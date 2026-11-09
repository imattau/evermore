#!/usr/bin/env python3
"""Assemble the offline BGE + Qwen/adapter model store after evaluation."""

import argparse
import hashlib
import json
import shutil
from pathlib import Path


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def artifact(path, filename):
    return {
        "file": filename,
        "sha256": sha256(path),
        "size_bytes": path.stat().st_size,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True, type=Path, help="Qwen Q4_K_M base GGUF")
    parser.add_argument("--adapter", required=True, type=Path, help="evaluated F16 LoRA GGUF")
    parser.add_argument("--bge-dir", required=True, type=Path, help="directory containing BGE base.gguf and manifest.json")
    parser.add_argument("--dataset-manifest", required=True, type=Path)
    parser.add_argument("--config", required=True, type=Path)
    parser.add_argument("--eval-report", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()

    dataset = json.loads(args.dataset_manifest.read_text())
    report = json.loads(args.eval_report.read_text())
    if dataset.get("scenario_quality", {}).get("reviewed") is not True:
        raise SystemExit("default model packaging requires independently authored and reviewed test scenarios")
    runs = report.get("runs", [])
    if len(runs) != 1:
        raise SystemExit("expected one adapter evaluation run in the report")
    if report.get("evaluation_split") != "test":
        raise SystemExit("default model packaging requires a locked test split evaluation")
    if report.get("dataset_sha256") != dataset.get("dataset_sha256"):
        raise SystemExit("evaluation report and dataset manifest hashes do not match")
    if report.get("catalogue_fingerprint") != dataset.get("catalogue_fingerprint"):
        raise SystemExit("evaluation report and dataset catalogue fingerprints do not match")
    if report.get("split_sha256") != dataset.get("split_sha256", {}).get("test"):
        raise SystemExit("evaluation did not use the locked test split from this dataset")
    if report.get("records") != dataset.get("counts", {}).get("test"):
        raise SystemExit("release evaluation must cover every record in the test split")
    scores = runs[0]
    thresholds = {
        "mcp_tool_accuracy": 0.95,
        "mcp_listed_tool_rate": 1.0,
        "mcp_correct_tool_schema_rate": 0.95,
        "panel_write_accuracy": 0.95,
        "panel_schema_valid_rate": 0.95,
        "panel_refusal_accuracy": 0.95,
        "mcp_refusal_accuracy": 0.95,
    }
    failures = [
        f"{metric}={scores.get(metric)} < required {threshold}"
        for metric, threshold in thresholds.items()
        if scores.get(metric) is None or scores[metric] < threshold
    ]
    if failures:
        raise SystemExit("adapter evaluation does not meet the default-store gate: " + "; ".join(failures))
    if not scores.get("mcp_grammar_enforced") or not scores.get("grammar_enforced"):
        raise SystemExit("both panel and MCP output grammars must be enforced during evaluation")
    if scores.get("mcp_unlisted_calls", 1) != 0:
        raise SystemExit("MCP evaluation contains unlisted tool calls")
    coverage = report.get("positive_case_coverage", {})
    for task, operations in (
        ("mcp", dataset.get("mcp_tools", [])),
        ("panel_write", dataset.get("panel_write_operations", [])),
    ):
        counts = coverage.get(task, {})
        missing = [name for name in operations if counts.get(name, 0) < 5]
        if missing:
            raise SystemExit(
                f"locked test needs at least five independently authored {task} positives per operation; "
                f"insufficient coverage: {', '.join(missing[:8])}"
            )

    output = args.output_dir
    output.mkdir(parents=True, exist_ok=True)
    for old_model in [*output.glob("p8.test.qwen2.5-1.5b-*"), *output.glob("p7.model.tinyllama-*")]:
        if old_model.is_dir():
            shutil.rmtree(old_model)
    qwen = output / "p8.model.qwen2.5-1.5b"
    bge = output / "p7.model.bge-1.0.0"
    qwen.mkdir(parents=True, exist_ok=True)
    bge.mkdir(parents=True, exist_ok=True)
    base_dest = qwen / "base.gguf"
    adapter_dest = qwen / "adapter.gguf"
    shutil.copy2(args.base, base_dest)
    shutil.copy2(args.adapter, adapter_dest)
    for name in ("base.gguf", "manifest.json"):
        shutil.copy2(args.bge_dir / name, bge / name)

    (qwen / "manifest.json").write_text(
        json.dumps(
            {
                "manifest_version": 1,
                "package_id": "p8.model.qwen2.5-1.5b",
                "package_version": "1.0.0",
                "description": "Qwen2.5-1.5B-Instruct with Evermore MCP and desktop operation tool-use LoRA",
                "base": artifact(base_dest, "base.gguf"),
                "adapter": {**artifact(adapter_dest, "adapter.gguf"), "scale": 1.0},
                "context_size": 2048,
                "quant": "Q4_K_M",
                "capabilities": ["chat"],
                "operation_registry_version": dataset["catalogue_fingerprint"],
                "training": {
                    "base_model_sha256": sha256(base_dest),
                    "dataset_sha256": dataset["dataset_sha256"],
                    "locked_test_sha256": report["split_sha256"],
                    "config_sha256": sha256(args.config),
                    "eval_report_sha256": sha256(args.eval_report),
                    "catalogue_fingerprint": dataset["catalogue_fingerprint"],
                    "contributed_traces": dataset["traces"]["included"],
                },
                "evaluation": {
                    key: scores[key]
                    for key in (
                        "records",
                        "parse_rate",
                        "operation_accuracy",
                        "panel_write_accuracy",
                        "panel_schema_valid_rate",
                        "mcp_tool_accuracy",
                        "mcp_listed_tool_rate",
                        "mcp_correct_tool_schema_rate",
                        "schema_valid_rate",
                        "refusal_accuracy",
                        "panel_refusal_accuracy",
                        "mcp_refusal_accuracy",
                        "mcp_unlisted_calls",
                        "grammar_enforced",
                        "mcp_grammar_enforced",
                    )
                },
                "test_coverage": report["positive_case_coverage"],
                "mcp_tools": dataset["mcp_tools"],
                "panel_write_operations": dataset["panel_write_operations"],
                "evaluation_split": report["evaluation_split"],
                "publisher": "Evermore default desktop image",
            },
            indent=2,
        )
        + "\n"
    )
    print(f"assembled offline model store: {output}")


if __name__ == "__main__":
    main()
