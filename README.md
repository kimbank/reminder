# reminder

Multi-account GitHub triage desktop app built with eframe/egui. It surfaces review requests, mentions, recent reviews, and notifications for several identities so you can sweep queues quickly without hopping profiles.

<img width="500" height="300" alt="preview" src="https://github.com/user-attachments/assets/820231fc-b025-47c9-91fe-a38928092b95" />


## Features
- Track multiple GitHub accounts with manual and auto-refresh (every ~180s) so long-running network work stays off the UI thread.
- Switch each account between a GitHub-like unified inbox view and the existing bucketed triage view.
- Responsive layout keeps account controls usable in narrow windows and falls back to a compact notification list when tables would get cramped.
- The left tracked-account list acts as the account selector. It shows per-account `new`, `unseen`, and `updated` counts so you can spot where activity changed before opening that inbox.
- Notification buckets: review requests, mentions, and everything else. Section headers display live counts for `unseen` and `updated` items within each bucket.
- Visual cues: unread rows use normal text; seen rows fade to a weaker palette; threads updated after `last_read_at` get a warning color plus an inline `Updated` badge so churn is obvious even if GitHub marked them read.
- Inline search filter per account matches repository, subject, or reason fields.
- Links open the underlying GitHub issue/PR page (subject URLs are normalized from `/pulls/` to `/pull/`).

## Setup
- Requires Rust (edition 2024) and a GitHub Personal Access Token per account with `notifications` and repo read scope.
- Tokens are stored in plaintext at `~/.reminder/accounts.json`; secure storage is a TODO.

## Running
```bash
cargo run --release
```
Use the left side panel to add an account (login + PAT), then click a tracked account to choose which inbox to show in the main panel. Each tracked account row shows whether new notifications arrived since that account was last opened, plus the current `unseen` and `updated` counts. Manual refresh is available per account; auto-refresh runs when data is older than the configured interval and no fetch is active.

## Developing
- Format and lint: `cargo fmt` and `cargo clippy --all-targets --all-features -D warnings`.
- Check builds quickly: `cargo check`.
- UI profiling: `cargo run --release`.

## Known limitations
- "Done" actions are intentionally disabled until GitHub exposes filtering that can hide already-archived notifications.
- Secure token storage is not yet implemented; avoid sharing hosts where plaintext PATs would be risky.
