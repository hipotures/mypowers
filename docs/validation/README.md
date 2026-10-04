# Application validation evidence

`IMPLEMENTATION_VALIDATION.md` and `results.json` record new application acceptance, not the
archived research's prior findings. Work captures/databases/private configuration stay under
ignored `work/` or `.local/validation/`. Concise sanitized result summaries are committed here.
`docs/research/s300/` is preserved byte-for-byte.

The initial validation report records its named source commit. A later user instruction changes
dotenv selection: server, CLI and TUI now read the current directory's `.env` when no
`--env-file` is supplied. See `docs/configuration.md` for the current behavior.
Follow-up verification on 2026-10-04 passed 285 tests on each of Python 3.12.14 and 3.14.7,
including actual daemon/CLI/TUI launches without `--env-file`, colored daemon logs and local Caddy.
Ruff, strict mypy and the rebuilt wheel passed. An actual CLI status read also reached the running
user daemon through the unchanged current-directory `.env`.

The supplied `docs/MYPOWERS_PRD.md` was untracked user input at task start and remains outside
implementation commits. A faithful copy is committed at `docs/PRD.md`. Its requirement numbering is used in the final checklist. No secret,
local database or working CA material belongs in these committed artifacts.
