# ALLPOWERS S300 connection and failure validation

This record supplements [the verified protocol](ALLPOWERS_S300_PROTOCOL.md).
It concerns the same physical AP S300 V2.0 at `2A:02:01:48:6B:D0`, not every
S300 firmware revision. Tests were performed on 2026-10-04, using Actions
USB `10d7:b012`, MAC `F4:4E:FC:A1:CB:FF`, Linux BlueZ 5.87,
kernel `7.2.8-1-cachyos`, Python 3.12.14, Bleak 3.0.2 and dbus-fast 3.1.2.
The operator used the official ALLPOWERS phone application **v1.5.1.155**.
Phone operating system and model were not recorded. Hardware/firmware numbers
remain UNKNOWN; the name V2.0 is not a firmware-version measurement.

## Method and evidence limits

Existing protocol notes, captures, the passive `read_allpowers.py` demo, and
`control_allpowers.py` were reviewed first. `s300_connection_lab.py` reuses the
known adapter resolver, Bleak connection path and strict telemetry decoder.
It records UTC and monotonic elapsed times, exact exception classes/messages,
D-Bus error attributes, raw notifications and BlueZ snapshots. It contains no
station control-write path and no daemon retry implementation.

Operator reports establish physical station BT state and the phone's app state;
they are not packet captures from the phone. The operator confirmed the phone's
live connection by switching the lamps on and off through the official app and
restored the lamps off. No Linux output-control frames were transmitted.
Initial flags were `0c` (AC/DC/lamps off), rather than the prior task's `0e`.
The Linux tests preserved the observed output state.

Logs are application-level observations, not HCI traces. There is no privileged
radio sniffer. Scan callbacks on the *connected* Actions adapter can be caused
by BlueZ property changes, so they do not establish continuing advertising.
Intel `64:BC:58:16:C6:59` / `hci1` was used only as an independent scanner,
never as a connection fallback. It detected other devices during negative
windows and S300 again after Linux disconnected, despite weak RSSI around
−102/−103 dBm. Negative windows establish no observed S300 advertising in those
windows, not a guarantee that no RF packet could ever be emitted.

Recorded times measure API operations and capture events. The operator's button
press timestamps are unknown; reply/note timestamps are later upper bounds.
Do not report scan-start latency as latency from the actual phone/button action.

## Findings from ordered connection trials

1. With phone BT off, Actions discovered S300, connected, resolved its services,
   subscribed to `fff1`, and received valid automatic telemetry. Initial GATT
   connect plus subscription took 1.926 s; the first status followed subscription
   by 1.019 s.
2. With Linux still connected, the phone could not connect and displayed
   **`Device offline`**. Linux telemetry continued and its connection remained
   resolved. Intel scans of 10, 20 and 10 seconds had zero target callbacks while
   detecting other devices.
3. Linux disconnect completed in 2.180 s. Intel saw S300 0.733 s after the
   subsequent scan started. Without a station BT toggle, the operator selected
   the app's **Reconnect** and reported successful connection after about 5 s:
   88%, 0/0 W and 131 h 39 m. Automatic phone recovery was **not demonstrated**;
   explicit app reconnect was demonstrated.
4. With the phone connected first, Actions had zero target callbacks during a
   20.012 s scan, with six other addresses seen. An address-based Bleak client,
   which performs an implicit scan, raised `BleakDeviceNotFoundError` after
   25.044 s. The phone remained connected; its lamp control still worked.
5. A separately tried old `BLEDevice` failed in 0.0033 s with
   `BleakError("device 'dev_2A_02_01_48_6B_D0' not found")`: its BlueZ object
   had disappeared. This failed *before a radio connection attempt* and diagnoses
   a stale local object, not an occupied peripheral.
6. After the operator disabled phone BT, without toggling station BT, Actions
   saw S300 after 0.131 s of a new scan. GATT plus subscription took 0.838 s;
   the first valid status arrived 0.016 s after subscription. Linux recovery by
   rediscovery/reconnection is verified; an autonomous production retry loop
   was neither implemented nor tested.

**Observed connection policy:** the tested unit was usable by one central at a
time in both orderings. No simultaneous working phone/Linux session was achieved.
The active client was not evicted by the unsuccessful competitor. This supports
exclusive-use recommendations for this exact unit; it does not measure the
controller's theoretical connection capacity or prove every firmware enforces a
one-link limit through an explicit rejection code.

**No competing-client-specific error was found.** The occupied-phone scan result
is observationally indistinguishable from a nonadvertising, unreachable or
powered-off station. Do not display “Connected to another client” as a diagnosis.
Suggest disconnecting another app as one possible remedy.

## Station Bluetooth disabled

The operator disabled the station's BT while Linux was receiving correct `fff1`
telemetry and left the station itself powered on. The last status was
15:38:50.001357 UTC. BlueZ still reported `Connected=true` and
`ServicesResolved=true` at 15:39:29.155672, during telemetry silence. A subsequent
five-second observation received zero notifications and raised the lab's own
`TimeoutError("no validated telemetry during 5-second observation")`.
Bleak itself raised no missing-telemetry exception.

The disconnected callback arrived at 15:39:35.980095, **45.979 s after the last
status**. This is a measured last-frame-to-callback gap, not a measured
button-to-disconnect latency or a universal supervision timeout. Connected state
alone cannot be used as a liveness guarantee.

An Actions scan overlapping the disconnection received two callbacks with
RSSI −127 and cached/property-only data. These were **not** accepted as evidence
of advertising. A separate Intel scan had no target callbacks. After local host
tests were restored, a clean Actions 20-second scan saw zero target callbacks and
five other addresses. The unchanged original demo also failed after 20.033 s:
`RuntimeError("Station not found; check station Bluetooth and phone connection")`.
Its underlying scanner timeout returns `None`, not an exception from Bleak.

One cached-device connect attempt at 15:40:32 overlapped adapter power/rfkill
changes and ended in a 25-second `TimeoutError`. This combined-condition trial
is retained in the raw log but **excluded** from station-off/cause conclusions.

The operator then re-enabled station BT. The next Actions scan detected S300
after 0.345 s. Correct `fff1` telemetry subsequently recovered with flags `0c`;
this must not be interpreted as remote BT control.

## GATT success without telemetry

Two different observations must remain separate:

- The real station-BT-off trial above produced silence on the **already correct
  and working `fff1` subscription**, while Connected and ServicesResolved still
  reported true. This verifies a stale established connection.
- A deliberate negative control connected successfully and subscribed only to
  the previously silent `ff03` characteristic. Connection plus subscription
  took 3.569 s; a 5.003 s observation received zero callbacks while still
  connected. The lab generated the same local TimeoutError as above. This
  verifies that GATT/subscription success does not imply telemetry, but it is
  **not** a defect in `fff1` or evidence that the station spontaneously suppresses
  telemetry after a correct new subscription.

After the negative control, disconnecting, rediscovering, and subscribing to
`fff1` took 3.819 s for connection/subscription; the first valid status followed
by 0.480 s, with four frames in five seconds. A naturally silent **initial**
correct `fff1` subscription was not reproduced; that failure cause remains
UNVERIFIED. No guessed request/control frame was used to recover.

## Local infrastructure failures

### Actions controller absent from the VM

The operator removed VM passthrough entry `usb13`, host port `5-2` (the original
host list identifies this as Actions / general adapter). The guest's `lsusb`
no longer showed `10d7:b012`; BlueZ and rfkill showed only CSR and Intel.
The operator supplied kernel event `usb 9-14: USB disconnect, device number 51`.
The existing lab resolver raised
`RuntimeError("expected one powered Actions adapter, found 0")` in 0.0039 s.
An explicit hci2 scanner raised `BleakError("adapter 'hci2' not found")` in
0.0015 s. Other adapters remaining available did not authorize a fallback.
This verifies actual controller absence in the guest following passthrough
removal, not a physical host unplug. In ordinary operation, Actions missing from
BlueZ alone cannot distinguish unplugging, passthrough, driver or kernel issues.

After passthrough was reattached, the same Actions MAC resolved to hci2 again.
The first target scan callback followed scan start by 0.420 s; connection plus
`fff1` subscription took 4.218 s and the first valid status followed subscription
by 0.726 s. No station BT toggle was needed for this adapter recovery. USB device
number changed 51 to 53, and **rfkill ID changed 2 to 3**, even though hci2 stayed
the same. Resolve live identifiers by adapter MAC and current sysfs/rfkill data;
never store rfkill ID 2 as a permanent Actions identifier.

### Powered off and rfkill blocked

Actions was powered off with BlueZ, inspected, and restored. It remained present,
`Powered=false`, `PowerState="off"`, and kernel rfkill remained clear. The adapter
resolver raised `RuntimeError("Actions adapter is not powered")` in 0.0019 s.
A direct explicit-adapter Bleak scan raised
`BleakDBusError("org.bluez.Error.NotReady", "Resource Not Ready")` in 0.0064 s.

Actions was then soft-blocked through kernel rfkill ID 2, inspected, and restored
to unblocked/powered-on. `Powered=false`, `PowerState="off-blocked"`, rfkill soft
blocked and hard unblocked were observed. The resolver gave the same message in
0.0022 s; raw Bleak gave the same NotReady error in 0.0069 s. Powering on while
still blocked failed. **NotReady alone does not distinguish these states.**
Use both adapter properties and kernel rfkill; do not guess from exception text.
Hard-blocked hardware was not induced.

### BlueZ and system-bus unavailability: isolated transport experiments

An isolated private D-Bus daemon with no `org.bluez` owner and no activation
directory was used. The real host BlueZ and its adapters were left running.
The resolver raised
`RuntimeError("BlueZ GetManagedObjects failed: org.freedesktop.DBus.Error.ServiceUnknown")`
in 0.0074 s. Raw Bleak scanner start raised `BleakDBusError` in 0.0069 s:

```text
[org.freedesktop.DBus.Error.ServiceUnknown] The name org.bluez was not provided by any .service files
```

Pointing only the test process at a nonexistent system-bus socket produced
`FileNotFoundError(2, 'No such file or directory')` / message
`[Errno 2] No such file or directory`: resolver 0.0003 s, raw Bleak 0.0034 s.
This is distinct from a reachable bus without BlueZ. These are genuine D-Bus/
Bleak transport observations under **isolated failure injection**, not a live
host `bluetooth.service` crash test. Host service loss mid-connection, service
activation/restart timing, and D-Bus permission denial remain UNVERIFIED.
An attempted noninteractive stop of the real host service was denied by systemd:
`Access denied as the requested operation requires interactive authentication`.
The host service stayed active. The private-bus measurements therefore do not
stand in for a claim that a live host crash/restart was tested.

## Evidence-based recommendations for mypowersd

| Requested question | Validation outcome |
|---|---|
| More than one simultaneous central | No simultaneous working phone/Linux session achieved in either ordering; practical exclusive use verified, theoretical maximum UNKNOWN |
| Linux first, phone second | VERIFIED hardware session plus operator app result: Device offline, Linux continues |
| Phone first, Linux second | VERIFIED: no discovery; implicit-scan BleakDeviceNotFoundError; phone control still works |
| Advertising while connected | No target advertisements observed on independent scans while Linux owns link, or Actions while phone owns link; absolute RF silence not proven |
| Recovery after competing client releases | VERIFIED rediscovery/reconnection without station BT toggle; phone required Reconnect, automatic app recovery UNVERIFIED |
| Station BT disabled | VERIFIED: telemetry stale before delayed disconnect, then no discovery |
| USB controller absent | VERIFIED guest absence through actual passthrough removal; physical host unplug not tested |
| Controller blocked/unavailable | VERIFIED local Powered-off and soft-rfkill conditions; hard block and arbitrary driver failures UNVERIFIED |
| BlueZ unavailable | VERIFIED isolated no-BlueZ bus; live host crash/restart UNVERIFIED because service-stop authorization was denied |
| GATT succeeds, telemetry absent | VERIFIED established correct-fff1 silence during BT loss and deliberate ff03 negative control; naturally silent initial correct fff1 UNVERIFIED |
| Time to distinguish failures | Captured local API timings and configured radio deadlines; radio cause cannot be distinguished by waiting |
| Competing-client-specific BLE code | None observed; device-not-found/absence is generic, and cached-object failure is local |

Recommendations below are design inputs, not an implemented state machine.
The error states describe observations; radio causes remain conditional.

| State | What software observes | Safe conclusion | Must not claim | Suggested user message | Retry recommendation |
|---|---|---|---|---|---|
| Live telemetry | Selected Actions present/powered; resolved GATT; valid `fff1` with monotonic arrival time | Latest measured fields are available at the recorded age | Future connectivity, exact firmware, or other-client absence | “Connected. Last update: …” | No reconnect while healthy; monitor freshness |
| Target not discovered | Scan successfully starts/ends, no qualified target advertisements; `find_device_by_address` returns None | S300 was not discovered in this window | Bluetooth disabled, powered off, out of range, or occupied as a specific diagnosis | “Station not found. Check power, Bluetooth and range; disconnect other apps.” | Yes; bounded scans, pause/backoff between attempts, promptly retry on explicit user request |
| Known device object expired | Cached BLEDevice path missing in live ObjectManager; generic device-not-found BleakError | Local discovery object is no longer usable | Another central owns the device | “Connection information expired; searching again.” | Discard cached object, resolve adapter and rediscover; no pairing/cache purge required |
| Connection attempt fails/times out | Live device object exists; Connect/GATT await fails; record stage, exception and elapsed time | This attempt did not establish usable GATT by its deadline | Phone contention, authentication failure, or device fault unless separately observed | “Could not establish the Bluetooth connection. Retrying.” | Yes, bounded rediscovery after cleanup/backoff; never overlap connection attempts |
| Connected but no initial telemetry | GATT resolved, expected subscription completed, no valid `fff1` by local deadline | No usable status received; output state UNKNOWN | All outputs off, empty battery, competing client, or protocol failure from silence alone | “Bluetooth connected; waiting for station data.” then “No station data received.” | Allow a bounded initial wait; then clean disconnect/rediscover/resubscribe; no guessed polling frame |
| Telemetry stale | Previously valid `fff1`, age exceeds deadline; link may still report connected | Last values are stale; current state UNKNOWN | Station BT disabled or another client as diagnosed cause | “Station data stopped updating. Reconnecting.” | Yes, one bounded reconnect; invalidate any write authorization immediately |
| Local adapter absent | Reachable BlueZ; Actions MAC absent from Adapter1 objects | Required controller is unavailable to BlueZ | Physical unplugging as the unique cause; kernel/driver/passthrough can also hide it | “Actions Bluetooth controller is unavailable. Check USB or VM passthrough.” | Wait for adapter addition; slow infrastructure checks as backup; no fallback to CSR/Intel |
| Local adapter powered off | Required adapter present, Powered false; PowerState off, rfkill clear | Selected controller is off | Station fault or contention | “Actions Bluetooth is turned off. Enable the controller.” | Await property change; do not repeatedly scan/connect or automatically power it on |
| Local adapter rfkill blocked | PowerState off-blocked plus rfkill soft/hard state | Local radio block observed | Which app/user applied it, or station BT state | “Actions Bluetooth is blocked. Unblock Bluetooth to reconnect.” | Await unblock/property change; avoid scan loop or overriding user block |
| BlueZ unavailable | System bus reachable; org.bluez has no owner; explicit calls yield ServiceUnknown | BlueZ unavailable at observation time | No USB controller, station unavailable, or permanent failure | “Bluetooth service is unavailable. Check BlueZ.” | Await NameOwnerChanged/recheck with bounded backoff; rebuild client/scanner on recovery |
| System bus unavailable | Socket/auth/transport exception before BlueZ request | Bluetooth infrastructure cannot be accessed through this bus | BlueZ daemon specifically crashed or S300 faulted | “Cannot access the system Bluetooth service.” | Infrastructure backoff; no RF retries until transport restored |
| Unexpected/invalid payload | Raw callback arrives but strict status decode rejects it | Received data is not a validated status | Healthy live telemetry or battery/output defaults | “Received unsupported station data.” | Preserve raw diagnostic frame; keep state invalid; bounded reconnect if sustained; never invent format or commands |

### Timing and retry policy inputs

Use local timeouts to bound observations, not to identify radio causes. The
validated reader's scan window is 20 s and its client timeout is 25 s. An
address-based client has an implicit scan; avoid accidentally performing two
full scans. Target absence can be reported at the end of the chosen scan window,
but **no finite wait identifies the reason for absence**.

Initial valid statuses arrived well within five seconds in successful tested
connections. A proposed initial-data deadline of 5 s and stale-data threshold
of 3 s are reasonable starting settings for this unit's approximately 1.3 s
stream, not guaranteed limits. A stale transition does not need to wait for the
roughly 46-second disconnection seen in the disabled-BT trial. Longer continuous
validation is needed before claiming false-positive rates or an SLA.

Proposed radio recovery: serialize attempts, fully clean up a failed client,
rediscover with a new BLEDevice, then connect and resubscribe. Add a short delay
and capped exponential backoff (for example 2, 5, 10, 20, 30 s with jitter)
between failed attempts. These values are **recommendations**, not measured
optimal settings. Wake immediately for user-requested retry or a local
adapter/service recovery event. Do not cycle station BT, toggle local power,
pair, or select another controller automatically. Local unavailable/blocked
states should wait on infrastructure changes rather than run a radio scan loop.

Connected and telemetry-current must remain separate observations. Keep the
last sample with its age for diagnostic history, never silently turn it into
current values or enable control writes from it. No current state means no
read-modify-write operation, regardless of Connected/ServicesResolved/Notifying.

## Captures and standalone research utility

Evidence is in [allpowers_s300_evidence/connection](allpowers_s300_evidence/connection/README.md):
the complete 626-event session contains 319 valid `fff1` frames, zero Linux
station control frames, and a clean final disconnect. Separate files preserve
private-bus/local-controller failures, original-demo exceptions, trial references,
source snapshots and SHA256 provenance. The README explains excluded trials and
traceback source-version caveats; do not infer additional evidence from cached
scan property callbacks or a wrong-characteristic subscription.

The final frame was `a565b1000108010c58000000001ed7e4`: battery 88%, input/output
0/0 W, flags `0c` (AC/DC/lamps off), remaining estimate 7895 minutes. Outputs
remain in the initial state; station BT is on, phone BT off, Actions powered/
unblocked and BlueZ active. All research clients were disconnected at completion.

The standalone lab can be run with a new capture filename:

```bash
uv run --no-project s300_connection_lab.py /tmp/allpowers-contention/new-session.jsonl
```

Supervised commands include `resolve`, `snapshot`, `scan hci2 5`, `connect`,
`observe 5`, `disconnect`, `note ...`, and `quit`. Resolve the live Actions hci
before selecting an explicit scanner. `rawscan` bypasses the application resolver
only for local API failure diagnosis; `ff03` and `none` are deliberate negative
controls, not alternate telemetry sources. No adapter/service mutation or station
control command exists in this lab. Captures refuse overwrite. Cleanup failures
retain the client and block another connection; rejected commands leave an
existing healthy connection intact. Four offline cleanup guards passed after
independent review. The fixes did not require new output or radio mutations.

## API/source corroboration

The exact installed Bleak 3.0.2 source was inspected independently. With explicit
adapter selection, missing/off adapters produce generic BleakError or
BleakDBusError; the richer `BleakBluetoothNotAvailableError.reason` values are
mainly produced by the default-adapter path. Do not change adapter selection to
obtain a nicer exception and inadvertently choose another controller.

BlueZ `Connected` and `ServicesResolved` are local-stack observations;
`GattCharacteristic1.Notifying=true` means a subscription exists, not that valid
frames are arriving. Bleak has no standard exception for notification silence.
The lab's TimeoutError is explicitly generated by its observation deadline.

Primary API references:

- [Bleak 3.0.2 Linux backend source](https://github.com/hbldh/bleak/tree/v3.0.2/bleak/backends/bluezdbus)
- [BlueZ Adapter1 specification](https://github.com/bluez/bluez/blob/5.87/doc/org.bluez.Adapter.rst)
- [BlueZ Device1 specification](https://github.com/bluez/bluez/blob/5.87/doc/org.bluez.Device.rst)
- [BlueZ GATT characteristic specification](https://github.com/bluez/bluez/blob/5.87/doc/org.bluez.GattCharacteristic.rst)
- [D-Bus specification](https://dbus.freedesktop.org/doc/dbus-specification.html)

These corroborate API interpretation, not undocumented S300 radio behavior.
