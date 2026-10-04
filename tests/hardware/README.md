# Explicit hardware acceptance

Default pytest discovery never opens Bluetooth. Stop only your own local station probes before
running this standalone runner. Do not run it alongside a daemon using another runtime directory.
The runner starts one real daemon with its own data/log/runtime paths, exercises the installed
CLI and API, records observations/resources, and terminates its own process on completion.

Passive monitoring needs the station powered on with Bluetooth enabled and the configured
Actions adapter exposed by BlueZ. It sends no output frame unless the write opt-in is supplied.

For output tests, the operator must establish that AC/DC loads are safely interruptible and
**do not power the test computer, Proxmox host, network path or critical equipment**. A 0 W
reading or historical test does not establish this. The lamp must be in ordinary common-lamp
mode; unknown individual/SOS behavior cannot be restored by these commands.

```bash
uv run python tests/hardware/run_acceptance.py --env-file .env \
  --output .local/hardware-read-20261004 --seconds 3600

# Run only after the above operator conditions have actually been established:
uv run python tests/hardware/run_acceptance.py --env-file .env \
  --output .local/hardware-control-20261004 --seconds 3600 \
  --allow-output-changes --loads-safely-interruptible --ordinary-common-lamp-mode
```

`MYPOWERS_TEST_ALLOW_OUTPUT_CHANGES=1` can replace only `--allow-output-changes`; it does not
replace the safe-load/lamp declarations. No flag is enabled by the program. There are no prompts
at individual transitions. The runner rotates the eight-state Gray cycle to its actual initial
flags, changes one output per API request, and requires two post-write samples. Confirmations
prove BLE observations, not electrical voltage or causal attribution against physical buttons.

On an unexpected state, lost telemetry or ambiguous command, stop and inspect the station.
Guarded sequential restoration is allowed only from the fresh expected test progression. The
runner never restores blindly or switches everything on. SIGINT/SIGTERM cancel monitoring and
stop the daemon. Raw DEBUG evidence remains JSONL under the selected output directory.
`results.json` and `REPORT.md` classify reads, controls, soak, and physical observation separately.
Hardware validation evidence must be reviewed before any release claim; simulated tests are
not substitute hardware results.
