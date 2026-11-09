# buchcraft help

Task-oriented guide. Every command also exists as an MCP tool ([docs/TOOLS.md](docs/TOOLS.md)); add `--json` for machine-readable output; `buchcraft <command> --help` lists all options. The desktop window (`buchcraft-app`, or `start.bat`) shows the same data in 8 views.

## First setup

```powershell
buchcraft accounts seed          # EKR chart with tax keys (or: buchcraft demo-seed to look around)
buchcraft accounts list
buchcraft accounts add 7610 "Fachliteratur" --kind expense --tax-key V10
```

## Booking

Book **net** lines with a tax key; buchcraft adds the VAT line itself. Amounts are DE format (`1.200,00`), never floats.

```powershell
# One-liner for the common case: gross amount, account gets net, VAT added, contra gets -gross
buchcraft gross --date 05.11.2026 --kreis ER --account 7600 --contra 2800 --gross "120,00" --tax-key V20 "Büromaterial"

# Any entry as JSON (file with @)
buchcraft book @entry.json
```

Belegkreise: AR (Ausgangsrechnungen), ER (Eingangsrechnungen), BK (Bank), KA (Kassa), SO (Sonstige). Belegnummern are assigned per year and Kreis (`ER-2026-0007`).

**Mistakes are never edited.** Reverse with a Storno entry; both point at each other:

```powershell
buchcraft reverse 17 --date 06.11.2026 --reason "falsches Konto"
```

## Bank statements

```powershell
buchcraft bank import D:\Bank\george-2026-11.csv   # Erste George CSV; re-import skips known lines
buchcraft bank unmatched
buchcraft bank book 42 --account 7380 --tax-key V20 --description "A1 Internet"
```

## Drafts from other apps

assetcraft sends depreciation (AfA) as drafts. Nothing is booked until you approve:

```powershell
buchcraft inbox-drain
buchcraft drafts list
buchcraft drafts approve 3       # or: drafts discard 3
```

## Month end

```powershell
buchcraft journal --from 01.11.2026 --to 30.11.2026
buchcraft balance --from 01.01.2026 --to 30.11.2026      # Saldenliste
buchcraft ledger 2800 --from 01.11.2026 --to 30.11.2026  # Kontoblatt
buchcraft u30 2026-11                                    # UVA pre-fill (KZ 000, 022, 029, 060, 065, 070, 095)
buchcraft export --from 01.01.2026 --to 31.12.2026 D:\Temp\journal-2026.csv
buchcraft close-month 2026-11                            # no more bookings or Storno dates in November
```

The U30 output is a pre-fill to type into FinanzOnline yourself; buchcraft does not submit anything.

## Troubleshooting

- **"entry does not balance"**: the lines must sum to exactly 0,00 including the generated VAT line; book net amounts with a tax key, do not add VAT lines yourself.
- **"period is closed"**: book into the next open month or Storno there; closed months stay closed.
- **Bank import found 0 lines**: the CSV header is not one of the known George layouts yet; send a sample (header only) so the alias table can be extended.
- **MCP cannot read the CSV**: start the server with `--allow-path <folder>`.
