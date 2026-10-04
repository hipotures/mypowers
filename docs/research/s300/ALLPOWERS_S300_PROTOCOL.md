# ALLPOWERS S300: experimentally scoped BLE protocol

This is the authoritative protocol research record for the single station tested
on 2026-10-04. It supersedes the protocol assumptions in the earlier telemetry
history and TUI PRD. It is not a specification for every S300 revision.

AC, DC and the common lamp switch were exercised on real hardware, observed with
BLE notifications and a camera, and restored. The complete control surface found
in the S300 V2 app path consists of these three switches. Generic app code also
contains fields for other models; those fields are not S300 capabilities.

## 1. Identity and transport evidence

| Evidence | Observed value |
|---|---|
| User-supplied product identity | ALLPOWERS AP-SS-005, 288 Wh, 300 W continuous / 500 W peak |
| Advertisement and readable `2a00` name | `AP S300 V2.0` (GATT value has trailing NUL padding) |
| Station address | `2A:02:01:48:6B:D0`, BlueZ address type `public` |
| Manufacturer identifier / payload | `1494` (`0x05d6`) / `08004a4c414953444b` |
| Working adapter | Actions USB `10d7:b012`, address `F4:4E:FC:A1:CB:FF` |
| Tested adapter path | `/org/bluez/hci2`; resolve from adapter address each run |
| Host environment | Linux/BlueZ in a QEMU guest; Python/Bleak 3.0.2 |
| Pairing, bonding, trust | Not required; all reported false |
| Hardware revision number | **UNKNOWN**: no version notification or Device Information service |
| Firmware version | **UNKNOWN**: `V2.0` is a product/protocol name, not a measured firmware version |

The enclosure photographed during testing is marked 230 V / 300 W. No electrical
frequency, voltage or current instrument was attached. Scope any later write
support to this observed unit/profile until another revision is independently
qualified. Address and name checks in the probe select this unit; they are not
cryptographic authentication or a firmware compatibility test.

All UUIDs below use suffix `-0000-1000-8000-00805f9b34fb`.

| Service | Characteristic | Handle | Properties | Finding |
|---|---|---:|---|---|
| `0000fff0` | `0000fff1` | 5 (CCCD 7) | notify | Verified status stream |
| `0000fff0` | `0000fff2` | 8 | write, write-without-response | Verified combined output command |
| `0000fff0` | `0000ff03` | 10 (CCCD 12) | notify | Subscribed; no messages observed |
| `00001800` | `00002a00` | 2 | read, write | Name read; name writes not attempted |
| `0000ae00` | `0000ae01` | 129 | write-without-response | Purpose UNKNOWN; never written |
| `0000ae00` | `0000ae02` | 131 (CCCD 133) | notify | Subscribed; no messages observed |

Use UUIDs rather than hard-coded handles. The full control UUID is
`0000fff2-0000-1000-8000-00805f9b34fb`.

The proven connection path is: resolve the powered Actions adapter by its MAC
using BlueZ `GetManagedObjects`; scan the station on that adapter; pass the
discovered `BLEDevice` to `BleakClient`; validate service/characteristics/name;
subscribe to `fff1`. Use `bluez={"adapter": adapter}` with Bleak 3.0.2. The phone
was disconnected during experiments. There is no automatic adapter fallback.

Subsequent [connection/failure validation](ALLPOWERS_S300_CONNECTION_VALIDATION.md)
tested the official app against Linux in both connection orderings, adapter
absence/power/rfkill states, station BT loss and isolated D-Bus failures. Use its
evidence-based state/error matrix for later daemon connection behavior. It does
not extend the station's verified command surface.

## 2. Verified read protocol

The station sends status automatically after subscription. Passive periods
preceded all control writes, including more than four minutes before the first
request experiment. The usual interval was approximately 1.3 seconds. A state
change often produced an earlier notification, followed by the regular stream.
Polling writes are unnecessary for this unit.

Each observed status notification contained one complete 16-byte frame. No
fragmentation or concatenation was observed; the standalone probe deliberately
does not invent reassembly behavior.

| Offset | Size | Observed bytes / meaning |
|---:|---:|---|
| 0 | 2 | Header `a5 65` |
| 2 | 3 | Status envelope `b1 00 01`; individual meanings UNKNOWN |
| 5 | 1 | Payload length `08`; total length `8 + N` |
| 6 | 1 | Status command `01` |
| 7 | 1 | Status flags, see direction-specific matrix below |
| 8 | 1 | Battery percentage, 0–100 |
| 9 | 2 | Aggregate input power, unsigned big-endian watts |
| 11 | 2 | Aggregate output power, unsigned big-endian watts |
| 13 | 2 | Station's remaining-time estimate, unsigned big-endian minutes |
| 15 | 1 | XOR of bytes 0–14 |

Validate header, exact length, length byte, command and checksum before decoding.
XOR of every byte including the checksum must be zero. Validate battery range.
The time estimate is not a calculated guarantee of runtime or proof of a charging
mode. At idle it can change markedly; no special sentinel semantics were verified.

Example from the earlier telemetry investigation:

```text
a5 65 b1 00 01 08 01 0e 63 00 37 00 19 01 90 ab
```

This reports flags `0e`, battery 99%, input 55 W, output 25 W and 400 minutes.
The camera later showed 19 W while the same value was delivered in BLE telemetry
with the vacuum cleaner charger powered from AC. Disconnecting the solar panel
changed input from 1 W to 0 W without changing the flags. A connected USB-C cable
alone did not produce an observed flag change; individual port detection is not
established.

## 3. Verified write envelope and confirmation

The verified command family is a **complete combined state**, not a toggle:

```text
a5 65 00 b1 01 01 00 FLAGS XOR
```

| Offset | Size | Meaning |
|---:|---:|---|
| 0 | 2 | Header `a5 65` |
| 2 | 3 | Write envelope `00 b1 01`; individual meanings UNKNOWN |
| 5 | 1 | Payload length `01` |
| 6 | 1 | Output-control command `00` (not notification command `01`) |
| 7 | 1 | Complete control flags |
| 8 | 1 | XOR of bytes 0–7; equivalently `71 XOR FLAGS` for this prefix |

All safe-profile regression frames were sent with `response=False`
(write-without-response). Earlier DC on/off experiments also succeeded with
`response=True`; therefore write-without-response is supported, not uniquely
required. An ATT write response confirms transport receipt, not the physical
effect. The deliverable probe uses the proven `response=False` path explicitly.

No dedicated application acknowledgement, transaction identifier, error packet,
or settings response was observed. Confirmation consists of **two consecutive
post-write `fff1` status frames whose complete flag byte matches the expected
state**. Power/time/battery fields may differ between these frames. Camera
observations confirmed AC/DC indicators and lamp effects. They did not measure
DC voltage or USB current. First changed-status notifications in the canonical
round trips arrived about 0.11–0.37 seconds after transmission; this is an
observation, not a response-time guarantee.

### Status and control flag bytes are different

| Function / app field | RX status mask | TX control mask | Exact-unit evidence |
|---|---:|---:|---|
| DC group | `01` | `01` | Physically observed DC indicator; paired on/off captures |
| AC group | `02` | `02` | Physically observed AC indicator and charger power; paired captures |
| Frequency selector (`is60Hz` in app) | `04` | `08` | Flag cleared by a three-field command, restored by TX `08`; actual Hertz UNVERIFIED |
| `beepOpen` in app | `08` | `10` | RX `08` remained set; clearing TX `10` did not confirm a change; preserve baseline |
| Common light switch | `10` | `20` | Both lamps illuminated in the first lamp trial; on/off status and photographs |
| `screenOpen` in app | `20` | `40` | Generic source only; candidate did not change RX flag |
| `voiceOpen` in app | `40` | `80` | Generic source only; candidate did not change RX flag |
| `closeBle` in older app | None identified | `04` | Legacy source hypothesis; no confirmed disconnect |
| RX bit 7 | `80` | UNKNOWN | Not observed; reject writes |

The mapping above separates hardware findings from app field names. In particular,
RX `08` is not experimentally proven to mean that the S300 buzzer is enabled.
The user heard a beep during the first AC change and suspects changes generally
beep. No audio capture or exhaustive acoustic verification was performed.

An early public three-field encoder sent `a56500b10101000071` to switch AC off
from RX `0e`. The result was RX **`08`**, clearing both AC and the `04` selector.
This is captured evidence of an unintended cross-field change. The original full
flag byte was subsequently restored to `0e` with `a56500b10101001a6b`.

Do not reuse the three-field Python/package encoder as a safe S300 writer.
Do not copy an R600-only command or set TX flags equal to RX flags.

### Complete three-switch state matrix

All eight combinations of **AC, DC group and common lamps are VERIFIED WRITE**
on this exact unit with RX selector `04` and persistent flag `08` set, and higher
fields clear. This replaces the original four-frame research allowlist. Each
frame below was independently checksummed and transmitted using `response=False`.

| AC | DC | Lamps | Full expected RX flags | TX flags | Full TX frame | Result |
|---|---|---|---:|---:|---|---|
| off | off | off | `0c` | `18` | `a56500b10101001869` | PASS |
| on | off | off | `0e` | `1a` | `a56500b10101001a6b` | PASS |
| off | on | off | `0d` | `19` | `a56500b10101001968` | PASS |
| on | on | off | `0f` | `1b` | `a56500b10101001b6a` | PASS |
| off | off | on | `1c` | `38` | `a56500b10101003849` | PASS |
| on | off | on | `1e` | `3a` | `a56500b10101003a4b` | PASS |
| off | on | on | `1d` | `39` | `a56500b10101003948` | PASS |
| on | on | on | `1f` | `3b` | `a56500b10101003b4a` | PASS |

The supervised 2026-10-04 experiment used the proven Actions MAC selection,
exact discovered station, readable name and GATT verification, then subscribed
to `fff1` before writing. Initial flags were `0c`. Traversal was
`000 -> 100 -> 110 -> 010 -> 011 -> 111 -> 101 -> 001 -> 000`, tuple order
**(AC, DC, lamps)**. Each transition changed one field and preserved the other
two plus RX `04/08`. Each command had a newly received pre-write status under
3 seconds old and two consecutive fresh post-write frames with the **entire**
expected RX byte. Writes were sequential and never retried. No unexpected
profile, telemetry loss, disconnect during testing or missing confirmation occurred.

At the operator's request, the same short traversal was repeated after an
external light was switched off. A final repeat used the existing camera with
manual exposure `20` to resolve the illuminated AC/DC buttons next to the bright
lamps; camera exposure `156` and auto-exposure mode `3` were restored afterward.
There were three separate connections, eight writes each, with restoration and
clean disconnect after every cycle. These repeats qualify observation quality,
not long-term stability. Raw captures and annotated vectors are in
[`allpowers_s300_evidence/matrix/`](allpowers_s300_evidence/matrix/README.md).
Across 24 writes, first matching responses arrived in 0.088–0.175 seconds and
the second confirmation in 1.475–1.750 seconds, measured from the application
write timestamp. These are captured timings, not guaranteed latency bounds.

Photos with lamps off clearly show AC/DC indicators. Lamp-on photos show both
lamp positions and, in the manual-exposure repeat, the corresponding illuminated
AC/DC buttons; glare obscures parts of labels and LCD digits. The first two
`011` images cannot independently resolve AC/DC. These limitations are recorded
per vector; no electrical voltage/current measurements were made.

**The future `mypowersd` may expose AC, DC group and common lamps as three
independent switches in arbitrary combinations within this qualified profile.**
Every operation still requires fresh-state read-modify-write, direction-specific
mask translation, preserved selector/persistent flags, serialized writes and
full-state confirmation. Eight complete states and the eight directed edges of
this Gray cycle were exercised, each three times. Other directed edges have
offline encoding coverage, not separate hardware captures. This does not extend
support to other revisions, selector/persistent profiles, individual ports or
lamp modes. The probe has no raw-frame option or unverified-field control.

Final evidence: RX `a565b1000108010c58000000001a8bbc` at
`2026-10-04T16:19:04.580270+00:00`: AC/DC/lamps off, preserved flags `0c`,
battery 88%, input/output 0/0 W. Client disconnected at `16:19:06.792241` UTC.

## 4. Official app surface, candidates and unsupported claims

Static Android artifacts for package `com.allpowers.aipower` identify
`AP S300 V2.0` / `AP-SS-005` as BLE protocol version 2 with
`dcSwitchSeparate=0`, `hasMainSwitch=0`, `hasBuzzerSwitch=0`, and
`hasLedSwitch=1`. The S300 template exposes **AC, merged DC 12 V + USB, and LED**.
An independent S300 pairing video shows these same controls. There is no
independently addressed USB flag in the examined local V2 encoder: `usbOpen` is
ignored there. A DC on photograph shows DC/USB indicators, but no USB load was
attached to measure electrical behavior.

During the later ordinary USB-C trials, the user reported functioning connected
devices while RX remained `0e` (DC flag clear). Aggregate output varied with the
loads. Do **not** interpret DC off as proof that USB-C is unpowered. The merged
DC/USB label describes the examined app UI; USB-C electrical switching is not
independently qualified by the DC indicator tests.

The physical manual describes individual lamp buttons and emergency flashing
using a long press. No S300 BLE brightness, individual-lamp or SOS/strobe command
was found. A one-bit common command is not evidence of remote mode selection.
The status light boolean cannot describe each lamp or a flashing mode. Initially
lit lamps therefore block the probe, avoiding an assumed restoration of a mode.

Generic protocol fields do not establish S300 support. The app does not expose
S300 frequency, buzzer, LCD, voice or Bluetooth-disable controls in the examined
S300 template. Candidate experiments preserve the observed AC/DC/lamp baseline:

| Candidate | Full transmitted frame | Result | Classification |
|---|---|---|---|
| Buzzer off | `a56500b10101000a7b` | RX stayed `0e`; no requested-flag confirmation within 10 s; baseline frame resent | UNVERIFIED |
| LCD field on | `a56500b10101005a2b` | RX stayed `0e`; LCD was already awake; baseline resent | UNVERIFIED |
| Voice field on | `a56500b10101009aeb` | RX stayed `0e`; no requested-flag confirmation; baseline resent | UNVERIFIED |
| Legacy `04` bit alone | `a56500b10101000475` | RX stayed `08`, link remained active; initially labeled a USB hypothesis, later rejected by app source | UNVERIFIED |
| Legacy Bluetooth-off bit with preserved baseline | `a56500b10101001e6f` | RX stayed `0e`, BLE notifications continued and no disconnect occurred; baseline resent | UNVERIFIED |

The labels in the temporary harness are experimental hypotheses, not protocol
definitions. In particular its historical `set usb on` entry does not verify USB.
The older JavaScript encoder names TX `04` `closeBle`; the newer v2.0.1 local
Dart encoder leaves that bit unassigned and its `closeBle` references concern
cloud logic. This source disagreement prevents asserting a Bluetooth-off command.

The R600 settings command (`02`), settings notification (`03`), ECO/charging
modes, car charger setting and version layout are **UNVERIFIED for this S300**.
They were not sent. No settings snapshot was received. Scheduling, time sync,
reset, calibration, firmware update and battery-management operations were not
exercised or exposed by the probe.

### Status/settings request

The R600 reference's exact special request was tested once on `fff2`:

```text
a5 65 b1 00 01 06 01 00 00 00 00 00
```

It is 12 bytes, has XOR `77`, and does not satisfy the normal `8 + N` envelope.
Do not silently repair it into a guessed S300 frame. The next statuses arrived
on the existing approximately 1.3-second schedule, with no separate response or
burst. The experiment establishes **neither acceptance nor rejection**. A S300
status/settings request remains **UNVERIFIED**. The safe probe sends none.

## 5. Read-modify-write safety invariants

### Additional USB-C alarm observation

The user described occasional beeping when connecting a USB-C device, sometimes
cleared by unplugging/replugging or changing the cable. At the end of the output
tests, they tried different devices/cables while a passive recorder subscribed
to `fff1`, `ff03`, and `ae02`, with periodic camera photographs. They reported
that every attempt worked normally and the alarm could not be reproduced.

Capture `20261004T150133Z-usb-c-observation.jsonl` covers
15:01:33–15:10:01 UTC: 369 valid status notifications, all on `fff1`, command `01`,
flags `0e`, input 0 W, output 19–88 W. No new command, unknown flag, malformed
notification, or message on the other subscribed channels was observed. No
control frame was sent during this capture; output state was left unchanged.
Camera event timestamps are archived separately; no audio was recorded.

**USB-C fault indication remains UNVERIFIED.** This normal-operation record
does not identify the cause of the intermittent beep, prove short-circuit
detection, or establish whether a real alarm is reported over BLE. If it occurs
later, capture raw notifications and operator timing before assigning a field.

### Temperature protection versus temperature telemetry

The [manufacturer-linked S300 V2 manual](https://cdn.shopifycdn.net/s/files/1/0443/1223/2089/files/S300_V2.0_AP-SS-005-NEW_220613-compressed.pdf?v=1676864252)
lists thermal charging protection at 55–65 °C, discharging protection at
65–75 °C, and cold protection at −10–0 °C. It documents thermal protection, but
does not specify sensor count/location, which component each threshold measures,
or a BLE temperature field. These are published model ranges, not temperatures
or thresholds experimentally measured on this unit. Internal thermal sensing is
a reasonable inference; an accessible numerical temperature is not established.

The 16-byte status payload is fully occupied by flags, battery, input/output
watts and minutes. Examined S300 V2 app decoding and public implementations have
no temperature field. Neither the normal control trials nor passive USB-C trials
produced another message with a temperature reading. Temperature, sensor count,
and a BLE overheat indication remain **UNKNOWN / UNVERIFIED**. No overheating
was deliberately induced. AC status alone can reveal that output turned off,
but cannot distinguish thermal shutdown from another cause.

The [manufacturer's S-series error chart](https://cdn.shopify.com/s/files/1/0747/3135/6467/files/S_series_product_error_code_query.pdf?v=1725705984)
has an explicit S300 section on page 1: Type-C group 5 includes subcodes 1
(excessive charging current), 2 (input voltage too high), 3 (input voltage too
low), and 4 (excessive discharge current). Its inverter group 7, subcode 7 is
generic; the S700 section's thermal label must not be copied to S300. This chart
contains no BLE frame layout or proof that these codes are exported locally.
It is further evidence of fault detection, not verification of the user's
intermittent beep or remote fault telemetry.

The user additionally recalls seeing a numerical fault code on the station's
LCD during an earlier event. No exact number or simultaneous BLE capture was
available. Record this as an operator recollection, not a verified error frame.
The next useful observation is the exact LCD code and its timing alongside raw
BLE notifications; a display code need not appear in the ordinary status payload.

1. Validate unit selection, powered Actions adapter, service, characteristics,
   readable device name and complete status before allowing any write.
2. Require one of the eight qualified complete profiles and a new status no older than 3 seconds.
   Recheck expected state under the operation lock immediately before each write.
3. Translate direction-specific masks; preserve the observed selector and
   persistent flag. Change one logical output at a time. Do not invent defaults.
4. Run mutations and confirmation sequentially; lock each write and allow only
   the eight captured frames. The standalone workflow has one operation at a time;
   its helper functions are not a concurrent control API.
5. Confirm the complete expected flags in two consecutive new status frames.
   Transport success alone is insufficient. Never optimistically update state.
6. Restore the starting state after the exercise. On failure or cancellation,
   attempt bounded restoration only while connected and with a fresh status equal
   to the expected test state or the original state.
7. If state is stale, unexpected, or disconnected, do not blindly replay a
   snapshot. Print the original state and require physical restoration.
8. Do not retry an unconfirmed command automatically. A delayed notification
   must not be counted as confirmation for another operation.
9. Do not infer initial individual-lamp or flashing modes from the light bit.

There is no observed sequence number, conditional-update command or device-side
stale-snapshot rejection. A combined write can replace another actor's change.
Device behavior under stale state is **UNKNOWN**; exclusive use, fresh snapshots
and client-side checks reduce this risk but cannot make the write atomic against
physical button presses. No intentionally stale multi-field write was performed.

A bounded invalid-checksum trial sent `a56500b10101001b6b` from RX `0e`
(the checksum for the DC-on frame should be `6a`). DC remained off in subsequent
status and the photograph; no error packet was received. This is evidence that
this malformed example had no observed effect, not a guarantee of all invalid
frame handling. The harness's `confirmed` label for this trial only means two
unchanged baseline statuses were received; it is not command acceptance.

## 6. Captures and regression vectors

`allpowers_s300_evidence/regression_vectors.json` records both directions of each
verified canonical operation: starting frame/state, exact TX, first changed RX,
second RX, expected physical effect, camera path and restoration frame/state.
The directory also preserves raw application-level TX/RX JSONL, checksums of
captures/photos, and experimental candidates separately. These are Bleak
application captures, **not HCI sniffing or official-app packet captures**.

Photographs remain in `/tmp/allpowers-camera/` as explicitly requested. The
manifest records their paths and hashes; `/tmp` may be cleared. Images are not
embedded in the repository. Lamp-on photos can be saturated; the first lamp
photo resolves two bright lamp positions, while a later canonical photo is
entirely white and cannot resolve individual hardware details.

Representative canonical command/status pairs:

```text
AC off:
  before a565b1000108010e61000000000c3923
  TX     a56500b10101001869
  RX     a565b1000108010c61000000000c3921
  next   a565b1000108010c61000000001d070e
AC restore:
  TX     a56500b10101001a6b
  RX     a565b1000108010e610000000021c5f2
DC on:
  before a565b1000108010e610000001301eeea
  TX     a56500b10101001b6a
  RX     a565b1000108010f610000001301eeeb
DC restore:
  TX     a56500b10101001a6b
  RX     a565b1000108010e600000001301e4e1
Lamps on:
  before a565b1000108010e600000001301e8ed
  TX     a56500b10101003a4b
  RX     a565b1000108011e600000001301e8fd
Lamps restore:
  TX     a56500b10101001a6b
  RX     a565b1000108010e600000001301aeab
```

Do not assert byte-for-byte equality of future dynamic telemetry with these
examples. Header/checksum/layout and expected complete flags are the regression
invariants; battery, power and time are captured values, not constants.

## 7. Standalone verified-control probe

`control_allpowers.py` is a small research utility, separate from the eventual
`mypowers` application. It reads by default; one explicit exercise changes a
single output, confirms it, holds briefly, restores it and confirms restoration.
It does not implement a TUI, HTTP service, database or application architecture.

```bash
uv run --no-project control_allpowers.py --seconds 5 --debug
uv run --no-project control_allpowers.py --exercise ac --debug
uv run --no-project control_allpowers.py --exercise dc --debug
uv run --no-project control_allpowers.py --exercise light --debug
```

Exercise commands accept the eight qualified profiles above and toggle the
selected field from its observed state, then restore the exact starting state.
Other initial profiles stop without writing. Debug
output includes full transmitted and received frames. Device state confirmation
is distinct from physical observation, so hardware qualification still needs a
camera/operator and an appropriate noncritical load. The default exercise holds
for three seconds. Unsupported states and unknown commands are rejected.
Connection or restoration failure is reported, not hidden.

The original `read_allpowers.py` remains a passive demo; its conservative unknown
bit reporting is historical. Use this document as the protocol input for a later
PRD update. A durable probe could be migrated to `scriptoza` in a separate task.

## 8. Source provenance and limits

External sources were reviewed before hardware writes; field interpretation was
then refined using captured behavior and vendor-app static analysis.

| Source | Revision inspected | What it establishes |
|---|---|---|
| [S300 ESP32 reader](https://github.com/madninjaskillz/AllPowersS300ESP32HomeAssistant/blob/4c55fe179f821394e0193915d6d9efe6852e086d/APS300.ino) | `4c55fe179f821394e0193915d6d9efe6852e086d`, 2023-08-01 | S300 V2 passive status offsets; no write proof |
| [S300 Python writer](https://github.com/madninjaskillz/AllPowersS300Python) | `e7c373f`, 2023-08-28 | Combined output frame hypothesis; only three fields, no safe preservation |
| [allpowers-ble](https://github.com/madninjaskillz/allpowers-ble/tree/7e67941b17f3f199a28e64952436a809aa03eb46) | package `0.0.3`, `7e67941b17f3f199a28e64952436a809aa03eb46` | Same three-field encoder; inspected history, no revision-qualified S300 captures |
| [Home Assistant protocol](https://github.com/dedalodaelus/home-assistant-allpowers-ble/blob/e5c10241ab0ad5d15fc3a08fac1bfb66796b47d6/docs/protocol.md) | `e5c10241ab0ad5d15fc3a08fac1bfb66796b47d6`, v1.1.2 | R600-derived codec, checksum, gating and request; explicitly S300 read-only |
| [Joe-C S300 UART work](https://www.joe-c.de/tech/projekte/powerstation) | Page dated 2025-02-07 | First-hand internal 9600-baud UART frames with baseline `08`; no BLE capture |
| [S300 pairing video](https://www.igeekphone.com/wp-content/uploads/2023/07/Pairing-with-the-APP.mp4) | S300 V2 screen | Three app output controls; not frame evidence |
| [AP-SS-005-NEW manual](https://cdn.manomano.com/pim-dam-img/7585/68428952/fff7dfdb651bcfc4990eef5a91ae7b2e60b79a0a.pdf) | Publisher-hosted PDF | Physical controls, pairing and flashing modes; no BLE mode frames |
| [Manufacturer user guides](https://iallpowers.com/pages/user-guides) | Linked S300 V2 manual and S-series error chart | Thermal protection ranges and S300 USB-C fault categories; no BLE temperature/error format |
| [ALLPOWERS app listing](https://play.google.com/store/apps/details?id=com.allpowers.aipower&hl=en) / [public APK mirror](https://apkpure.net/tw/allpowers/com.allpowers.aipower) | v1.3.0, v1.5.0 and v2.0.1/build 219 | Static model metadata, V2 encoder/decoder and S300 UI path |

The APKs were downloaded from a public mirror, not captured from the user's
installed app, and were not independently authenticated against a vendor signing
certificate. Their static fields are corroborating evidence, not hardware tests.
No full APK or proprietary app source is redistributed here. Paths/hashes are in
the source manifest. v1.5.0 analysis used extracted `app-service.js`;
v2.0.1 analysis used Blutter disassembly (`fit_net_ble.dart`,
`allpowers_v2_model.dart`). Field names in this document are source attributions.

The HA history was also checked: initial R600 work `402cc40`, S300 read-only gate
`fe76ebf`, semantic guards `54fcb3e`, and correction of the R600 revision signature
to 0.3 in `34aa40d`. Its R600 verification is not S300 write evidence. Other
repositories inspected include `jinglemansweep/hacs-allpowers`,
`madninjaskillz/ha-allpowers-ble`, `john-in-france/allpowers-ble-bridge`,
`mattgorle/allpowers-ble`, and `dedalodaelus/esphome-allpowers-ble`; none supplied
independently captured, exact-revision S300 controls beyond the tested family.

## 9. Exact-unit capability matrix

| Function | Classification | Evidence / limitation |
|---|---|---|
| Battery percentage | VERIFIED READ | Valid automatic status frames; LCD observation |
| Aggregate input/output watts | VERIFIED READ | Status captures; LCD output agreement and solar disconnection |
| Remaining-time estimate | VERIFIED READ | Big-endian captured field and LCD; accuracy not guaranteed |
| AC state and on/off | VERIFIED WRITE | Canonical paired TX/RX, camera AC indicator and charger power |
| DC group state and on/off | VERIFIED WRITE | Canonical paired TX/RX, camera DC/USB indicators; no independent voltage measurement |
| Common lamps state and on/off | VERIFIED WRITE | Paired TX/RX and physical illumination/restoration |
| Automatic telemetry | VERIFIED READ | Passive stream before any request/control |
| Device name, services, properties | VERIFIED READ | GATT discovery and `2a00` read |
| Raw status / frequency-selector flag | READ-ONLY | Bit transition and preservation captured; actual 50/60 Hz UNVERIFIED, not an exposed probe control |
| Buzzer, LCD, voice settings | UNVERIFIED | Generic source candidates failed to confirm requested flags |
| Separate USB output / per-port state | UNVERIFIED | S300 app uses merged DC control; no independent BLE field or load test |
| Individual lamp, brightness, SOS/strobe | UNVERIFIED | Physical modes documented; no local BLE command found |
| Bluetooth off/on | UNVERIFIED | Conflicting generic source; no confirmed remote control |
| USB-C alarm/fault state | UNVERIFIED | User could not reproduce it; passive normal-operation capture has no distinct alarm packet/flag |
| Internal temperature / thermal alarm over BLE | UNVERIFIED | Manual documents protection; no sensor reading or exact-unit alarm packet observed |
| Status/settings request | UNVERIFIED | One R600 request trial indistinguishable from passive traffic |
| ECO, charging modes, timers, car setting | UNVERIFIED | Other-model fields only; no S300 settings snapshot/write |
| Hardware/firmware version, temperature, voltage, cell/BMS data | UNVERIFIED | No matching exact-unit data received |

"READ-ONLY" here means exposed only as diagnostic data by this research profile,
not a claim that the hardware can never write that field. "UNVERIFIED" means no
supported command is authorized by these findings, not a proof of nonexistence.
