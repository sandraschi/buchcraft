# AGENTS.md - buchcraft

Austrian double-entry bookkeeping craft app. Read first:
`mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md`,
`mcp-central-docs/projects/craft-fleet-expansion/BACKOFFICE_CRAFT_SUITE.md` (section 4.1).

## Layout

- `crates/buchcraft-store` (L1): ledger on SQLite WAL. All accounting invariants live here.
- `crates/buchcraft-core` (L1): `Command` bus, `dispatch`, MCP catalog, bank CSV parser, bus inbox, demo seed.
- `crates/buchcraft-mcp` (L2): `buchcraft` binary. CLI verbs map onto the same tool calls as MCP.
- `crates/buchcraft-app` (L3, egui 0.37): `buchcraft-app` window. Mutations go through `dispatch` (same path as CLI/MCP); reads use the typed ledger API. `--smoke` and `--screenshot PATH --view NAME` for headless checks.
- Shared kernel: `../craft-biz` (money, AT validators, EKR, bus, MCP loop).

## Accounting invariants (never weaken)

- Entries balance to exactly zero cents, including generated tax lines.
- Callers book NET lines with a tax key; only the engine writes tax lines.
- Booked rows are immutable (SQLite triggers). Corrections are Storno entries.
- Closed months refuse bookings and Storno dates.
- Money is `craft_biz::Money` (i64 cents). No floats for amounts, ever.

## Docs (keep current in the same commit)

- `README.md`: what it does, quick start, MCP config, build.
- `HELP.md`: task-oriented how-to; update when a verb or workflow changes.
- `docs/PRD.md`: purpose, boundaries, honest gaps, next steps; update "Today" and "Gaps" with every release.
- `docs/TOOLS.md`: generated. After any tool change run `just docs`; the `tools_doc_is_current` test fails otherwise.
- `CHANGELOG.md`: one entry per change set.
- CLI help: `about`, argument doc comments and `AFTER_HELP` in the mcp crate's `main.rs`.

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
6. `CHANGELOG.md` entry; README / HELP / PRD updated if behaviour changed


## Documentation preservation and hooks

On 2026-11-20 a README "standardization" sweep across the fleet deleted project descriptions,
status sections and user warnings in nine repos. Full rules:
`mcp-central-docs/standards/CONTENT_PRESERVATION_STANDARD.md`.

- Applying any standard or template restructures; it never deletes. Move content (within the file
  or to `docs/` with a link), do not drop it. Never remove what the project is, what it is based on,
  its status (what works, what does not, what is not built) or warnings to users.
- Remove content only when Sandra explicitly asks for that removal, in this repo.
- Never add claims you have not verified (release downloads, versions, test counts, features).
- Only standards with `status: active` may be applied across repos. A change across several repos is
  done on one repo first and approved by Sandra; its commits name the tool and carry a `Sweep-Id:` line.
- `.githooks/commit-msg` blocks a commit that deletes a Markdown file, removes 20% of its lines
  (15 or more) or removes 3 or more headings. A removal Sandra asked for passes with
  `Docs-Removal-Approved: <who and why>` in the message. Enable once per clone:
  `git config core.hooksPath .githooks`. Never use `--no-verify`; if a hook blocks you, stop and ask.
