#!/usr/bin/env python3
"""Evaluate a base or adapter model against the catalogue-derived eval split.

Starts llama-server locally, asks for one operation proposal per record, and
reports selection accuracy, schema validity, refusal correctness, and parse
rate. Runs an adapter-scale sweep when --adapter and multiple --scale values
are given.

    tools/adapter-training/evaluate.py \
      --eval tools/adapter-training/data/eval.jsonl \
      --base out/base.gguf \
      --adapter out/adapter.gguf \
      --scale 0,0.5,1.0 \
      --grammar tools/adapter-training/proposal.gbnf \
      --out out/eval-report.json
"""

import argparse
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--eval", required=True, help="eval.jsonl from evermore-adapter-data")
    parser.add_argument("--base", required=True, help="base GGUF")
    parser.add_argument("--adapter", help="optional LoRA GGUF")
    parser.add_argument("--scale", default="1.0", help="comma-separated adapter scales")
    parser.add_argument("--grammar", help="optional GBNF grammar (proposal.gbnf)")
    parser.add_argument("--llama-server", default="llama-server")
    parser.add_argument("--ctx-size", type=int, default=2048)
    parser.add_argument("--max-tokens", type=int, default=192)
    parser.add_argument("--limit", type=int, default=0, help="evaluate only the first N records")
    parser.add_argument("--out", required=True, help="report JSON path")
    return parser.parse_args()


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_ready(port, process, timeout=180):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"llama-server exited with {process.returncode}")
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=1) as response:
                if response.status == 200:
                    return
        except OSError:
            pass
        time.sleep(0.2)
    raise RuntimeError("llama-server did not become ready")


def chat(port, messages, grammar, max_tokens):
    body = {
        "model": "eval",
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": 0,
        "stream": False,
    }
    if grammar:
        body["grammar"] = grammar
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=300) as response:
        payload = json.load(response)
    return payload["choices"][0]["message"]["content"]


def extract_json(text):
    text = text.strip()
    if text.startswith("```"):
        lines = [line for line in text.splitlines() if not line.startswith("```")]
        text = "\n".join(lines).strip()
    start = text.find("{")
    end = text.rfind("}")
    if start < 0 or end < start:
        raise ValueError("no JSON object in output")
    return json.loads(text[start : end + 1])


TYPE_MAP = {
    "string": str,
    "integer": int,
    "number": (int, float),
    "boolean": bool,
    "array": list,
    "object": dict,
}


def validate(schema, value, path="arguments"):
    if "const" in schema and value != schema["const"]:
        raise ValueError(f"{path} must equal {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        raise ValueError(f"{path} is not one of {schema['enum']!r}")
    kind = schema.get("type")
    if isinstance(kind, list):
        kind = kind[0] if kind else None
    if kind == "integer" and isinstance(value, bool):
        raise ValueError(f"{path} must be an integer")
    if kind in TYPE_MAP and not isinstance(value, TYPE_MAP[kind]):
        raise ValueError(f"{path} must be {kind}")
    if isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            raise ValueError(f"{path} is too short")
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            raise ValueError(f"{path} is too long")
        if schema.get("format") == "sha256":
            if len(value) != 64 or any(character not in "0123456789abcdefABCDEF" for character in value):
                raise ValueError(f"{path} must be a SHA-256")
        if schema.get("format") == "path_component":
            allowed = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._+-")
            if not value or any(character not in allowed for character in value):
                raise ValueError(f"{path} must be a safe path component")
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            raise ValueError(f"{path} is below the minimum")
        if "maximum" in schema and value > schema["maximum"]:
            raise ValueError(f"{path} is above the maximum")
    if isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            raise ValueError(f"{path} has too few items")
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            raise ValueError(f"{path} has too many items")
        if "items" in schema:
            for index, item in enumerate(value):
                validate(schema["items"], item, f"{path}[{index}]")
    if isinstance(value, dict):
        for key in schema.get("required", []):
            if key not in value:
                raise ValueError(f"{path}.{key} is required")
        properties = schema.get("properties", {})
        if schema.get("additionalProperties") is False:
            for key in value:
                if key not in properties:
                    raise ValueError(f"{path}.{key} is not allowed")
        for key, item in value.items():
            if key in properties:
                validate(properties[key], item, f"{path}.{key}")


def run_split(args, records, scale, grammar, port, process):
    if args.adapter:
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/lora-adapters",
            data=json.dumps([{"id": 0, "scale": scale}]).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=30) as response:
            response.read()

    totals = {
        "records": len(records),
        "parsed": 0,
        "kind_correct": 0,
        "operation_correct": 0,
        "schema_valid": 0,
        "refusals_expected": 0,
        "refusals_correct": 0,
        "unexpected_operations": 0,
    }
    known = {record["expected_operation"] for record in records if record.get("expected_operation")}
    failures = []

    for record in records:
        messages = record["messages"][:-1]
        try:
            content = chat(port, messages, grammar, args.max_tokens)
            proposal = extract_json(content)
            totals["parsed"] += 1
        except (OSError, ValueError, json.JSONDecodeError, KeyError) as error:
            failures.append({"id": record["id"], "error": str(error)})
            continue

        if "refusal" in proposal:
            got_kind = "refusal"
        elif "operation" in proposal:
            got_kind = "proposal"
        else:
            got_kind = "unknown"
        expected_kind = record["expected_kind"]
        if got_kind == expected_kind:
            totals["kind_correct"] += 1

        if expected_kind == "refusal":
            totals["refusals_expected"] += 1
            if got_kind == "refusal":
                totals["refusals_correct"] += 1
            continue

        expected_operation = record["expected_operation"]
        operation = proposal.get("operation")
        if operation == expected_operation:
            totals["operation_correct"] += 1
        elif operation not in known:
            totals["unexpected_operations"] += 1
        arguments = proposal.get("arguments")
        if isinstance(arguments, dict):
            try:
                validate(record["input_schema"], arguments)
                totals["schema_valid"] += 1
            except ValueError as error:
                failures.append({"id": record["id"], "error": str(error)})
        else:
            failures.append({"id": record["id"], "error": "arguments is not an object"})

    proposals = max(1, totals["records"] - totals["refusals_expected"])
    report = dict(totals)
    report["parse_rate"] = totals["parsed"] / max(1, totals["records"])
    report["operation_accuracy"] = totals["operation_correct"] / proposals
    report["schema_valid_rate"] = totals["schema_valid"] / proposals
    report["refusal_accuracy"] = (
        totals["refusals_correct"] / totals["refusals_expected"]
        if totals["refusals_expected"]
        else None
    )
    report["grammar_enforced"] = bool(grammar)
    report["scale"] = scale if args.adapter else None
    report["failures"] = failures[:20]
    return report


def main():
    args = parse_args()
    records = []
    with open(args.eval, encoding="utf-8") as handle:
        for line in handle:
            if line.strip():
                records.append(json.loads(line))
    if args.limit:
        records = records[: args.limit]
    grammar = None
    if args.grammar:
        grammar = open(args.grammar, encoding="utf-8").read()

    scales = [float(value) for value in args.scale.split(",")] if args.adapter else [None]
    port = free_port()
    log = tempfile.NamedTemporaryFile(prefix="evermore-eval-llama-", delete=False)
    command = [
        args.llama_server,
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--model",
        args.base,
        "--ctx-size",
        str(args.ctx_size),
        "--no-webui",
        "--alias",
        "eval",
    ]
    if args.adapter:
        command.extend(["--lora", args.adapter])
    process = subprocess.Popen(command, stdout=log, stderr=log)

    def stop(*_):
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

    try:
        wait_ready(port, process)
        runs = [run_split(args, records, scale, grammar, port, process) for scale in scales]
    finally:
        stop()
        log.close()
        if process.returncode not in (0, -signal.SIGTERM, 143):
            with open(log.name, encoding="utf-8", errors="replace") as handle:
                print(handle.read()[-4000:], file=sys.stderr)
        os.unlink(log.name)

    report = {
        "eval_file": args.eval,
        "base": args.base,
        "adapter": args.adapter,
        "records": len(records),
        "runs": runs,
    }
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2)
    for run in runs:
        print(
            "scale={scale} parse={parse:.3f} operation={operation:.3f} "
            "schema={schema:.3f} refusal={refusal}".format(
                scale=run["scale"],
                parse=run["parse_rate"],
                operation=run["operation_accuracy"],
                schema=run["schema_valid_rate"],
                refusal=run["refusal_accuracy"],
            )
        )


if __name__ == "__main__":
    main()
