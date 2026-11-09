# buchcraft PRD

## What it is

Austrian double-entry bookkeeping (Einnahmen-Ausgaben is not the model; this is doppelte Buchhaltung on the EKR chart) for a small business, local-first. Pure Rust, ArtCraft pattern: one engine, CLI + MCP + egui window. The hub of the back-office craft suite (`BACKOFFICE_CRAFT_SUITE.md`, section 4.1).

## Users

- The owner or bookkeeper doing monthly books and the UVA pre-fill.
- Agents over MCP: propose accounts and tax keys for unmatched bank lines, prepare drafts; a human approves before anything is booked.
- The Steuerberater receives the Saldenliste and journal export.

## Jobs

1. Book correctly and immutably, with VAT computed by the engine.
2. Get bank lines in and matched without retyping.
3. Produce U30 figures, Saldenliste and Kontoblatt at month end.
4. Accept bookings proposed by other craft apps (AfA from assetcraft) as drafts.

## Today (v0.2.0)

- Ledger on SQLite WAL with immutability triggers; entries balance to the cent; the engine writes all tax lines; Storno with two-way links; month close.
- EKR seed chart with tax keys (V20/V10/V13/U20/U10/U13/IG) and U30 Kennzahlen.
- Bank CSV import (Erste George layouts), dedup, booking of bank lines.
- Drafts inbox fed by the craft-biz bus (`asset.depreciation_due`), verified end to end with assetcraft.
- 22 MCP tools ([TOOLS.md](TOOLS.md), generated and test-checked), CLI, egui app with 8 views (De/En, light/dark, screenshot mode).

## Boundaries

- No FinanzOnline submission, no RKSV / Registrierkasse, no payroll, no UGB balance sheet layout (Saldenliste only), EUR only.
- Agents never book directly from external input: drafts need human approval.
- Not a replacement for the Steuerberater's review.

## Gaps (honest)

- Bank CSV aliases not verified against a real George export.
- EKR seed account numbers need a Steuerberater check.
- CAMT.053 import not built.
- No golden-snapshot GUI tests, no democraft recipes, no installer yet.
- Synthetic data only.

## Next

1. Verify the bank importer with one real George export.
2. egui_kittest snapshots for the 8 views; right-align amount columns.
3. Steuerberater review of the seed chart and U30 mapping.
