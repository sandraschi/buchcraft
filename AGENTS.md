# AGENTS.md - buchcraft

Austrian double-entry bookkeeping craft app. Read first:
`mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md`,
`mcp-central-docs/projects/craft-fleet-expansion/BACKOFFICE_CRAFT_SUITE.md` (section 4.1).

## Layout

- `crates/buchcraft-store` (L1): ledger on SQLite WAL. All accounting invariants live here.
- `crates/buchcraft-core` (L1): `Command` bus, `dispatch`, MCP catalog, bank CSV parser, bus inbox, demo seed.
- `crates/buchcraft-mcp` (L2): `buchcraft` binary. CLI verbs map onto the same tool calls as MCP.
- `crates/buchcraft-app` (L3, egui): not built yet.
- Shared kernel: `../craft-biz` (money, AT validators, EKR, bus, MCP loop).

## Accounting invariants (never weaken)

- Entries balance to exactly zero cents, including generated tax lines.
- Callers book NET lines with a tax key; only the engine writes tax lines.
- Booked rows are immutable (SQLite triggers). Corrections are Storno entries.
- Closed months refuse bookings and Storno dates.
- Money is `craft_biz::Money` (i64 cents). No floats for amounts, ever.

## Rules

- Zero-panic: unwrap/expect/panic/todo/unimplemented denied outside tests.
- MCP mode: stdout is JSON-RPC only; diagnostics to stderr.
- File tools must go through `Confinement::check`.
- Demo data only via `demo-seed`, only on an empty ledger, always flagged `demo: true`.
- Every bug fix ships with a regression test.

## Repo hygiene

- LF line endings only (`.gitattributes`). CRLF breaks commit hooks.
- Single trunk `main`. No agent branches or leftover WIP branches.
- No em dash characters in code, docs or commit messages.
- No `*.bak`, `*.db`, `target/`, binaries in git.

## Gates

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test --workspace`
3. `cargo fmt --all --check`
4. MCP probe: `initialize` -> `tools/list` -> `tools/call`, stdout pure JSON
5. `Cargo.lock` committed, clean `git status`
6. `CHANGELOG.md` line
