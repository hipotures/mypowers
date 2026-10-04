# S300 connection-validation evidence

Read [the connection research document](../../../ALLPOWERS_S300_CONNECTION_VALIDATION.md)
for interpretation and recommendations. These files do not implement mypowersd.

- `session.jsonl`: complete supervised hardware session, 626 events and 319
  valid status frames; no station output-control frames. Main capture includes
  operator observations, raw RX, timed scan windows, service snapshots, exception
  types/messages, and cleanup. It ended with a clean disconnect.
- `trials.json`: selected event references and measured durations, with hardware,
  operator-report, negative-control, isolated-transport and excluded trials
  distinguished explicitly.
- `phone-first-address-connect.json`: Bleak's implicit scan while the phone
  owned the connection; no device discovered by its 25-second deadline.
- `station-bt-off-demo.json`: the unchanged original passive reader's failure
  after its 20-second scan, with station BT physically off.
- `actions-powered-off.jsonl`, `actions-rfkill-blocked.jsonl`,
  `host-mutations.json`: local controller conditions, raw Bleak exceptions,
  rfkill/PowerState observations and restoration commands.
- `actions-absent-environment.json`, `final-environment.json`: guest/controller
  absence evidence and restored environment. The operator removed/re-added VM
  USB passthrough, rather than reporting a physical host unplug.
- `bluez-unavailable-*.jsonl`, `private-bus.xml`: actual isolated D-Bus without
  BlueZ or activation; the real host service remained active.
- `dbus-transport-unavailable.json`: a test process addressed a nonexistent bus
  socket; the real system bus was unchanged.
- `host-bluez-stop-permission.json`: real service stop was denied by systemd;
  do not claim the isolated tests were a host crash/restart trial.
- `manifest.json`: SHA256 provenance, capture counts, final raw status and source
  version caveats. Top-level prior control evidence remains unchanged.

`lab-session-version.py` is the source originally loaded for the long hardware
session. `lab-rawscan-version.py` is the later extension used by short private-bus
and local-controller captures. Source snippets printed in some main-session
tracebacks can reflect a later edit to the working file; compiled line numbers
refer to the initial version. Types, messages, attributes, frames and monotonic
timings were recorded directly and are not inferred from those snippets.
The current top-level lab utility includes subsequent reviewed cleanup fixes;
it is the version to use for new work.

The other archived Python scripts reproduce the historical setup only. In
particular **do not rerun `host-failures.py` unchanged**: it used then-current
rfkill ID 2. After reattachment Actions has ID 3. Historical scripts and fixed
output filenames are evidence, not reusable environment-mutating utilities.
Resolve live identifiers and preserve current state before a new experiment.

One overlapping station-off connect/host-power trial (main events 292–297) is
excluded from cause conclusions. One rejected duplicate connect (277–281) is a
lab guard/cleanup event, not a Bleak or device rejection. Two Actions scanner
callbacks at disconnection (272–273) carried RSSI −127/cached property data and
are not advertising evidence. `ff03` silence is a deliberate wrong-channel
negative control; a naturally silent initial correct `fff1` was not reproduced.

Phone reports are operator observations, not official-app/HCI packet captures.
No exact physical button timestamps or privileged radio traces were obtained.
