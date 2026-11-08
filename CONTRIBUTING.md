# Contributing to Evermore

Thanks for your interest in Evermore. Contributions of all sizes are welcome,
including questions, feedback, design ideas, documentation, bug reports, and
code.

## AI-assisted contributions are welcome

**AI-coded development is welcomed and encouraged.** You can use AI tools to
explore the project, plan a change, write code or documentation, review a
diff, and help validate it. Fully AI-written contributions are welcome too.
There is no requirement to avoid AI or to use a particular tool.

The person submitting a change is responsible for understanding and reviewing
it. Please check generated work for correctness, security, licensing, and fit
with the project; run relevant checks; and describe honestly what was and was
not verified. A short note about AI assistance is helpful when it explains
how the work was produced or checked, but AI use is not something to hide or
apologize for.

AI agents working on Evermore should read the relevant parts of
[`PROJECT_IDEA.md`](PROJECT_IDEA.md) and this guide, inspect the current diff
before editing, and preserve unrelated or concurrent work. Do not give an
agent access to private keys, credentials, personal calendar data, or other
private material.

## Before starting

Evermore is an early prototype, not a finished desktop or a supported Linux
distribution. Check the [project roadmap](PROJECT_IDEA.md) to see what is
implemented, what is still being explored, and how a proposed change fits.
For larger changes, open an issue or discussion first so the approach can be
considered before significant work begins. Small fixes and documentation
improvements can go straight to a pull request.

The project favors using established Linux and Nostr tools and standards
where they fit. Please explain the user need a new service or custom component
solves, and consider whether an existing project can provide it instead.

## Development setup

The repository is a Rust workspace with a separate Go relay. For the cleanest
development and desktop checks, use the disposable Arch Linux VM described in
[`vm/README.md`](vm/README.md). It keeps experiments separate from your main
computer and includes scripts for exercising the Sway desktop.

The VM workflow is:

```bash
vm/create-vm.sh
vm/sync-repo.sh "cargo check --workspace"
```

You can also build directly on a compatible Linux system with the Rust stable
toolchain and the system libraries required by the desktop components. The VM
guide lists the setup details and available checks. The relay has its own Go
module under `relay/`.

## Making a change

- Keep changes focused and explain the user problem they address.
- Follow existing patterns and project documentation; update the roadmap or
  relevant documentation when behavior or project direction changes.
- Prefer standard Nostr events and established Linux interfaces over new
  project-specific protocols when they meet the need.
- Treat identity keys and user data carefully. Never commit secrets, private
  keys, generated VM state, personal data, or real event contents.
- For AI features, keep permissions narrow, make consequential actions
  reviewable by the owner, and account for untrusted content in notes, files,
  and events.
- For user-interface changes, include a screenshot when practical and check
  keyboard use, focus visibility, and the existing Evermore visual direction.

## Checks and pull requests

Run checks that cover the part you changed. For Rust changes, useful starting
points are:

```bash
cargo fmt --all -- --check
cargo check --workspace
```

Run relevant tests or VM scripts where practical. If a check needs hardware,
network access, a model, or another setup you do not have, say so rather than
implying it passed. Include any important manual steps and known limitations.

When opening a pull request, please describe:

1. What problem the change solves and what you changed.
2. How you checked it, including any checks you could not run.
3. Any user-visible changes, screenshots, risks, or follow-up work.

Draft pull requests are welcome for early feedback. You do not need to have
every detail settled before asking for review.

## Be kind and constructive

Evermore welcomes contributors with different levels of experience and
different workflows, including people collaborating with AI. Please focus
reviews on the work and its evidence, explain requested changes clearly, and
make room for questions and iteration.
