# S300 complete three-switch matrix evidence

Scope: the exact `AP S300 V2.0` at `2A:02:01:48:6B:D0`, selected through
Actions `F4:4E:FC:A1:CB:FF`. All three short sessions on 2026-10-04 passed the
same eight-edge Gray cycle, preserving RX `04/08`. Every session began and ended
at RX `0c` and disconnected cleanly. There were 24 writes and 95 valid statuses;
no invalid profile or unconfirmed command occurred.

Each session directory contains:

- `session.jsonl`: unmodified, flushed Bleak application-level TX/RX and event
  capture, including exact starting frame, write time, complete RX frames,
  camera paths/hashes, restoration and disconnect. This is not an HCI capture.
- `matrix_vectors.json`: eight transitions derived from that capture. Camera
  interpretation and final-state references were added after image review;
  transmitted/received bytes and timestamps were not changed.

`first` is the original experiment. `repeat` is the operator-requested repeat
after they switched off an external light. `manual-camera` is the final repeat
with existing camera auto-exposure disabled and exposure set to `20`. Original
camera settings (`auto_exposure=3`, `exposure_time_absolute=156`) were restored
and read back after the final session. The external light remained as set by the
operator. The initial simultaneous multi-control restore command was rejected
with permission denied; sequential camera-control updates restored both values.
No extra station write was required for camera cleanup.

`summary.json` records counts and measured latency ranges. `manifest.json`
contains SHA256 hashes of archived files, relevant source versions and all 27
camera images. Images remain exclusively in `/tmp/allpowers-camera/`, as the
operator requested; they may disappear when `/tmp` is cleared.

`first-session-harness.py` and `repeat-session-harness.py` are frozen evidence
copies, not the maintained probe.
The first session used that version without the later 1.2-second settling delay
after lamp transitions. The repeat and manual-camera session used the second
snapshot. The maintained `s300_matrix_validation.py` subsequently narrowed
cleanup exception catches to `Exception`/`CancelledError` for lint; this did not
change any successful captured command or response. These versions never accept raw
frames or other controls. Their fixed candidate set is a research experiment,
not an authority to extend another hardware revision's write allowlist.

The operator confirmed phone Bluetooth off, safe interruptible loads, and no
station button changes during the test. No subagents or external research were
used. AC/DC buttons and lamp positions were reviewed in photographs; lamp glare
obscures labels and digits in some images. In particular, automatic-exposure
`011` photographs cannot verify AC/DC, whereas the manual-exposure image shows
the illuminated DC button and dark AC button. No outlet voltage, current,
frequency, USB port behavior or lamp modes were measured.

All eight complete states and these eight directed transitions are captured:
`000 -> 100 -> 110 -> 010 -> 011 -> 111 -> 101 -> 001 -> 000`, with tuple order
`(AC, DC, common lamps)`. Offline tests cover all eight frames and all 24 directed
single-switch encodings. Those additional edges are not separate hardware trials.

Reproduction is a supervised hardware experiment, not a passive test:

```bash
uv run --no-project s300_matrix_validation.py /tmp/NEW-UNUSED-CAPTURE-DIRECTORY
```

It writes only the fixed three-switch candidates, changes one logical field per
transition, obtains a newly received status before every write, checks exact
identity/GATT/profile, refuses unknown or unexpected flags, requires two complete
fresh confirmations, never retries a write and closes the client. The cycle is
rotated only from the four previously qualified initial profiles. On failure it
stops; restoration is conservative and otherwise explicitly requires manual
intervention. It must not be used unattended with critical loads. The maintained
`control_allpowers.py` has the now-qualified eight-state allowlist, accepts any
qualified initial state, toggles one selected field and restores it.

Offline verification:

```bash
uv run --no-project --with bleak==3.0.2 --with dbus-fast==3.1.2 \
  python -m unittest test_control_allpowers.py test_s300_matrix.py test_s300_connection_lab.py
```

Result: 35 tests passed. The tests never connect to hardware. No `mypowers`,
daemon, UI, server, database or retry state machine was implemented.
