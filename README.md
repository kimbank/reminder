# reminder

Multi-account GitHub triage desktop app built with eframe/egui. It surfaces review requests, mentions, recent reviews, and notifications for several identities so you can sweep queues quickly without hopping profiles.

<img width="500" height="300" alt="preview" src="https://github.com/user-attachments/assets/820231fc-b025-47c9-91fe-a38928092b95" />

## Features

- Track multiple GitHub accounts with manual and auto-refresh (every ~180s) so long-running network work stays off the UI thread.
- Switch each account between a GitHub-like unified inbox view and the existing bucketed triage view.
- Responsive layout keeps account controls usable in narrow windows and falls back to a compact notification list when tables would get cramped.

## Setup

- Requires Rust (edition 2024) and a GitHub Personal Access Token per account with `notifications` and repo read scope.
- Tokens and local repo path mappings are stored in plaintext at `~/.reminder/accounts.json`; secure storage is a TODO.

## Running

```bash
cargo run --release
```

## Developing

- Format and lint: `cargo fmt` and `cargo clippy --all-targets --all-features -D warnings`.
- Check builds quickly: `cargo check`.
- UI profiling: `cargo run --release`.

## Known limitations

- "Done" actions are intentionally disabled until GitHub exposes filtering that can hide already-archived notifications.
- Secure token storage is not yet implemented; avoid sharing hosts where plaintext PATs would be risky.
