#!/usr/bin/env python3
"""Fine-tune a LoRA adapter on the catalogue-derived dataset.

Training is intentionally outside `ai-inferd` and the ISO build. Install the
extras in `requirements.txt` on a machine with a CUDA/ROCm GPU, then:

    tools/adapter-training/train_lora.py \
      --config tools/adapter-training/config.example.json \
      --data-dir tools/adapter-training/data \
      --output-dir out/adapter-peft

The resulting PEFT directory is converted with
`tools/model-build/convert-adapter.sh`.
"""

import argparse
import json
import os
import sys


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    parser.add_argument("--data-dir", required=True)
    parser.add_argument("--output-dir", required=True)
    return parser.parse_args()


def main():
    args = parse_args()
    with open(args.config, encoding="utf-8") as handle:
        config = json.load(handle)

    try:
        import torch
        from datasets import Dataset
        from peft import LoraConfig
        from transformers import AutoModelForCausalLM, AutoTokenizer
        from trl import SFTConfig, SFTTrainer
    except ImportError as error:
        print(
            f"training dependencies are missing ({error}).\n"
            "Install them on a GPU host with:\n"
            "  python3 -m venv .venv && . .venv/bin/activate\n"
            "  pip install -r tools/adapter-training/requirements.txt",
            file=sys.stderr,
        )
        return 2

    tokenizer = AutoTokenizer.from_pretrained(config["base_model"])
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    def load(split):
        rows = []
        path = os.path.join(args.data_dir, f"{split}.jsonl")
        with open(path, encoding="utf-8") as handle:
            for line in handle:
                if not line.strip():
                    continue
                record = json.loads(line)
                text = tokenizer.apply_chat_template(
                    record["messages"], tokenize=False, add_generation_prompt=False
                )
                rows.append({"text": text})
        return Dataset.from_list(rows)

    train = load("train")
    evaluation = load("eval")

    model = AutoModelForCausalLM.from_pretrained(
        config["base_model"],
        torch_dtype=torch.bfloat16 if config.get("bf16", True) else torch.float16,
        device_map="auto",
    )
    model.config.use_cache = False

    peft = LoraConfig(
        r=config.get("rank", 16),
        lora_alpha=config.get("alpha", 32),
        lora_dropout=config.get("dropout", 0.05),
        bias="none",
        task_type="CAUSAL_LM",
        target_modules=config.get("target_modules"),
    )

    training = SFTConfig(
        output_dir=args.output_dir,
        num_train_epochs=config.get("epochs", 3),
        per_device_train_batch_size=config.get("batch_size", 1),
        gradient_accumulation_steps=config.get("gradient_accumulation", 8),
        learning_rate=config.get("learning_rate", 2e-4),
        max_length=config.get("max_seq_length", 1024),
        logging_steps=10,
        save_strategy="epoch",
        eval_strategy="epoch",
        seed=config.get("seed", 7),
        bf16=config.get("bf16", True),
        report_to=[],
    )
    trainer = SFTTrainer(
        model=model,
        args=training,
        train_dataset=train,
        eval_dataset=evaluation,
        peft_config=peft,
        processing_class=tokenizer,
    )
    trainer.train()
    trainer.save_model(args.output_dir)
    tokenizer.save_pretrained(args.output_dir)
    with open(os.path.join(args.output_dir, "training-config.json"), "w", encoding="utf-8") as handle:
        json.dump(config, handle, indent=2)
    print(f"adapter written to {args.output_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
