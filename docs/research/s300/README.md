# ALLPOWERS S300 pre-project research

This bundle preserves pre-project hardware/protocol research for the exact tested
ALLPOWERS **AP-SS-005 / `AP S300 V2.0`**, address `2A:02:01:48:6B:D0`.
It does not establish compatibility with every S300 hardware or firmware revision.

- [ALLPOWERS_S300_PROTOCOL.md](ALLPOWERS_S300_PROTOCOL.md) is authoritative for
  verified BLE read/write behavior and the qualified eight-state AC/DC/common-lamp
  control matrix, including its fresh-state and preservation requirements.
- [ALLPOWERS_S300_CONNECTION_VALIDATION.md](ALLPOWERS_S300_CONNECTION_VALIDATION.md)
  is authoritative for connection lifecycle, contention, failure detection and
  observed recovery behavior and limitations.
- [ALLPOWERS.md](ALLPOWERS.md) and [BLUETOOTH.md](BLUETOOTH.md) retain the
  investigation history and environment background.
- **[ALLPOWERS_TUI_PRD.md](ALLPOWERS_TUI_PRD.md) is historical and superseded**
  by the later protocol/connection research and the future project-specific
  `mypowers` application PRD. Its original readiness statements and architecture
  are preserved as history, not current implementation requirements.

`read_allpowers.py`, `control_allpowers.py`, `s300_connection_lab.py`,
`s300_matrix_validation.py` and their three offline test files are research/reference
implementations, **not production `mypowers` architecture**. Archived helper
scripts describe historical experiments; some contain obsolete environment IDs
or paths. Preserve their meaning rather than treating them as production tools.

The complete `allpowers_s300_evidence/` tree retains regression vectors, manifests,
JSON/JSONL captures, README files and frozen experiment scripts as regression and
provenance material. Earlier evidence README files describe earlier qualification
stages; the authoritative documents above take precedence.

Future production code must live **outside this research directory**. Treat these
files as historical/evidence inputs and do not casually modify them during
application implementation. Preserve existing captures when adding new evidence.
Capabilities marked **UNKNOWN** or **UNVERIFIED** must not become supported
application features without new hardware evidence.

Imported on 2026-10-04 from `/home/user/DEV/tmp`, source commit
`2736cdd284d3f246504d3cd2c353dcce21390e70`: 12 top-level files and 47 evidence
files copied unchanged, with their directory structure preserved. SHA256, source
modification times and file modes were checked. The source files were retained.
No application code was added or modified. All 35 existing offline tests passed
from this copied directory.

Repository-local capture/vector references and relative Markdown links were
checked. One pre-existing link in
`allpowers_s300_evidence/connection/README.md` uses
`../../../ALLPOWERS_S300_CONNECTION_VALIDATION.md`; it points one directory too
high and remains unchanged to preserve the source. Use the authoritative link
above. Absolute paths and Git object IDs retain their original research-workspace
provenance; they are not portable runtime dependencies. Historical manifest source
hashes describe their capture dates and may precede later research updates.
External camera images, APK/XAPK binaries, third-party checkouts and analysis
scratch files were intentionally not imported; their provenance references remain.
