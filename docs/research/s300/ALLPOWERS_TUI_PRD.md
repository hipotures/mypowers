# ALLPOWERS S300 live terminal monitor PRD

This document specifies a Python application that displays live telemetry from the user's ALLPOWERS S300 in a Rich terminal dashboard. It gives a future implementer the verified Bluetooth route, data contract, screen design, connection lifecycle and acceptance criteria. This task delivers the specification only; it does not implement the application.

Status: ready for implementation against the tested device. Evidence date: 2026-10-04. Repository documents and future application source use English.

## Protocol research update (2026-10-04)

The exact-unit protocol findings in [ALLPOWERS_S300_PROTOCOL.md](ALLPOWERS_S300_PROTOCOL.md)
supersede the earlier protocol assumptions below. AC, DC and common lamp writes
now have captured hardware evidence under a narrow state profile. This document
has not been redesigned into a control application PRD; no application was built.

## Product objective

The user should be able to launch one command and see battery percentage, input and output power, the station's remaining-time estimate and known output flags without opening the phone app. The dashboard must clearly distinguish current readings from old readings and remain responsive while scanning, connecting or recovering from a lost connection.

The initial product monitors one station on Linux with BlueZ. Rich provides the terminal UI; Bleak provides BLE communication. No vendor cloud, account, database or HTTP service is required. The initial product subscribes to telemetry and does not expose controls for AC, DC, lights or device settings.

## Verified hardware and connection

### Station identity

| Parameter | Verified value or evidence |
| --- | --- |
| User's product model | AP-SS-005, described as ALLPOWERS S300 |
| Advertised name | `AP S300 V2.0` |
| Station Bluetooth address | `2A:02:01:48:6B:D0` |
| Address type reported by BlueZ | Public |
| Transport | Bluetooth Low Energy GATT |
| Manufacturer identifier in advertising | `0x05d6` / 1494 |
| Manufacturer payload observed | `08004a4c414953444b` |
| Nominal capacity supplied by the user | 288 Wh, Li-Ion |
| Nominal continuous and peak power supplied by the user | 300 W / 500 W |

The address identifies this specific unit. The nominal specifications are product metadata, not values read from Bluetooth. The manufacturer payload helps diagnostics but must not be treated as a unique identifier or telemetry.

### Available adapters in this environment

| Adapter at discovery | Controller address | USB identifier | Device | Observed result |
| --- | --- | --- | --- | --- |
| `hci0` | `00:1A:7D:DA:71:11` | `0a12:0001` | CSR8510 A10 | Timeouts and disconnection before service discovery completed |
| `hci1` | `64:BC:58:16:C6:59` | `8087:0029` | Intel AX200 Bluetooth | Station detected; connection attempt timed out |
| `hci2` | `F4:4E:FC:A1:CB:FF` | `10d7:b012` | Actions general adapter | GATT connection and repeated valid notifications succeeded |

Use the Actions controller by default. These results establish the tested route, not a diagnosis that the other controllers are defective. Adapter numbering may change after a reboot or USB reattachment. Resolve the preferred controller by its Bluetooth address at startup, then pass its current `hciN` identifier to Bleak. If it is absent, display the available controllers and ask the user to select one through the CLI; do not silently switch to an unverified controller.

The current environment is a QEMU guest. All three controllers now appear through USB passthrough. AX200 Bluetooth was passed through as USB; passing the PCIe Wi-Fi function was not needed for this connection.

### Connection prerequisites

- The adapter must be present, powered and unblocked, and the BlueZ service must be active.
- The station's Bluetooth mode must be enabled. Its Bluetooth icon alone did not guarantee that a new connection would succeed during earlier attempts.
- Keep the ALLPOWERS phone app disconnected while monitoring from the computer. The successful test used a phone with Bluetooth disabled.
- Run under a user allowed to access BlueZ through system D-Bus. The verified telemetry read did not require root, pairing, bonding, a PIN or `bluetoothctl trust`.
- In the VM, verify USB passthrough before investigating the BLE protocol.

Useful diagnostics are `lsusb`, `bluetoothctl list`, `rfkill list`, `systemctl is-active bluetooth` and `ls -la /sys/class/bluetooth`. `btmon` requires additional privileges here and is not an application prerequisite.

## Bluetooth services and data acquisition

All UUIDs below are complete values. The application must verify the expected telemetry service and characteristic after connecting.

| Role | UUID | Observed properties | Product use |
| --- | --- | --- | --- |
| Telemetry service | `0000fff0-0000-1000-8000-00805f9b34fb` | Service | Required |
| Status notifications | `0000fff1-0000-1000-8000-00805f9b34fb` | Notify | Subscribe and decode |
| Command characteristic | `0000fff2-0000-1000-8000-00805f9b34fb` | Write, write without response | No application writes |
| Additional characteristic | `0000ff03-0000-1000-8000-00805f9b34fb` | Notify | Meaning unknown; leave unused |
| Generic Access service | `00001800-0000-1000-8000-00805f9b34fb` | Service | Optional identity information |
| Device name | `00002a00-0000-1000-8000-00805f9b34fb` | Read, write | Optional read; never rename |
| Additional vendor service | `0000ae00-0000-1000-8000-00805f9b34fb` | Service | Leave unused |
| Additional vendor characteristic | `0000ae01-0000-1000-8000-00805f9b34fb` | Write without response | Leave unused |
| Additional vendor characteristic | `0000ae02-0000-1000-8000-00805f9b34fb` | Notify | Meaning unknown; leave unused |

Notifications arrived automatically after subscribing to `fff1`; no polling command was needed. The measured interval in the final short session was about 1.3 seconds, but this is an observation rather than a guaranteed device frequency.

### Required connection sequence

1. Resolve the requested controller from BlueZ adapter objects and check availability. Stop with actionable information if it is absent or disabled.
2. Actively scan on that controller, matching the configured station address. Show the station name and last advertising RSSI when available. Allow up to 20 seconds for discovery.
3. Stop scanning before connecting. Do not filter the scan by service UUID: this station was observed advertising without a service UUID list.
4. Pass the discovered `BLEDevice` to `BleakClient` and explicitly use the same controller. Apply a 25-second overall deadline around connection and service discovery.
5. Verify service `fff0` and the notify property on `fff1`, then subscribe with `start_notify`.
6. Wait for a valid status frame before marking telemetry as live. A connected BLE link is insufficient.
7. Keep notifications active for the dashboard session. On exit, stop notifications when possible, disconnect and restore the terminal.

For Bleak 3.0.2, use `bluez={"adapter": "hciN"}` for both scanner and client. Avoid the deprecated top-level `adapter` argument. Use an asynchronous client context manager and `disconnected_callback` to handle cleanup and link loss. These APIs and the recommendation to connect using a discovered `BLEDevice` are documented in the [Bleak scanner reference](https://bleak.readthedocs.io/en/latest/api/scanner.html) and [Bleak client reference](https://bleak.readthedocs.io/en/latest/api/client.html).

RSSI is the last advertising measurement, with its own timestamp. Do not present it as continuously measured signal strength once scanning stops. Concurrent scanning during a connected session is not required.

## Status frame contract

### Captured example

```text
Hex:     a5 65 b1 00 01 08 01 0e 63 00 37 00 19 01 90 ab
Offset:  00 01 02 03 04 05 06 07 08 09 10 11 12 13 14 15
```

This frame was captured from the user's station. It represents battery 99%, input 55 W, output 25 W, reported remaining time 400 minutes and flags `0x0e`.

### Field mapping

| Byte offsets, starting at zero | Field | Encoding | Example |
| --- | --- | --- | --- |
| 0–1 | Header | Exact bytes `a5 65` | `a565` |
| 2–4 | Protocol metadata | Observed `b1 00 01`; retain raw, meaning not established | `b10001` |
| 5 | Payload length | Unsigned byte; observed status payload length 8 | 8 |
| 6 | Command | `0x01` for the verified status frame | 1 |
| 7 | Flags | Unsigned bit field | `0x0e` |
| 8 | Battery percentage | Unsigned integer, valid range 0–100 | 99% |
| 9–10 | Input power | Unsigned 16-bit integer, big-endian, watts | 55 W |
| 11–12 | Output power | Unsigned 16-bit integer, big-endian, watts | 25 W |
| 13–14 | Reported remaining time | Unsigned 16-bit integer, big-endian, minutes | 400 min |
| 15 | Checksum for this 16-byte frame | XOR of all bytes, including this byte, must equal zero | `ab` |

The initial product supports the observed status layout: 16 bytes, payload length 8 and command 1. Require the header, encoded total length `8 + frame[5]`, zero XOR checksum and battery range to be valid before publishing a sample. Treat different lengths or commands as unsupported frames; preserve diagnostic information without interpreting them as this layout. Do not reject a valid frame solely because metadata bytes 2–4 differ, since their semantics are not established.

The working reader received complete frames in each notification. The initial implementation may use the same contract. If a notification is truncated, reject it and leave telemetry freshness unchanged; record the raw bytes. Fragmented or concatenated frames require captured evidence and a separate parser extension before support is claimed.

### Known flag interpretation

| Mask | Meaning in the referenced S300 mapping | Observed value for `0x0e` |
| --- | --- | --- |
| `0x01`, bit 0 | DC enabled | False |
| `0x02`, bit 1 | AC enabled | True |
| `0x10`, bit 4 | Light enabled | False |
| All other bits | Not interpreted | Preserve as raw flags |

This mapping matches the [S300 implementation used as the protocol reference](https://github.com/madninjaskillz/AllPowersS300ESP32HomeAssistant/blob/main/APS300.ino). Parsing and transport were verified here; the flag meanings were not independently tested by toggling outputs. Display them as reported flags. Unknown bits 2 and 3 were set in the captured sample. A false DC flag must not be expanded into a claim that every USB or DC-related port is off.

## Values the dashboard must display

| Value | Source | Display rule |
| --- | --- | --- |
| Battery | Validated byte 8 | Large percentage and bar; 0 is a valid value |
| Input power | Validated bytes 9–10 | Integer watts, with a short history trend |
| Output power | Validated bytes 11–12 | Integer watts, with a short history trend |
| Remaining time | Validated bytes 13–14 | Hours and minutes, explicitly labeled as station estimate |
| AC, DC and light flags | Validated byte 7 | On, Off or Unknown before the first valid sample |
| Connection and telemetry state | Application lifecycle and freshness | Always visible, using text as well as color |
| Last valid update | Application timestamp | Local clock time and elapsed age in seconds |
| Station and adapter identity | Discovery and adapter resolution | Name, station address, controller name and address |
| Last scan RSSI | Advertising data | dBm, labeled Last scan, with age |
| Nominal capacity and power ratings | User-supplied specifications | 288 Wh / 300 W / 500 W in a small Specifications area |
| Notification health | Session counters | Valid samples, rejected frames and reconnect attempts |
| Raw frame and flags | Last received bytes | Compact diagnostics area, visually secondary |

The station's remaining-time estimate fluctuated with output power. Show the raw reported value without smoothing it into a supposedly more accurate estimate. Display zero as `0h 00m`; the meaning of zero during idle or charging remains unverified.

An optional small arithmetic value may show `input_power_w - output_power_w`, labeled Power difference. It must not be labeled battery charging power: internal consumption, conversion losses and the station's measurement boundaries are unknown. No battery runtime or charging-time prediction should be calculated from this difference.

The verified telemetry does not supply battery voltage, current, temperature, health, cycle count, actual remaining Wh, charge limit, individual USB port power, individual socket power, solar-only power or a verified charging/discharging mode. The UI must not invent these readings. Nominal capacity multiplied by percentage is not an actual stored-energy measurement and is excluded from the main dashboard.

## Live terminal experience

### Visual structure

Use a restrained dark-terminal palette with clear spacing. Battery is the largest value. Input and output power have equal visual weight. Use green or cyan for live data, amber for stale data and red for disconnected or invalid configuration, always accompanied by a text label. Honor terminal color capability and provide readable output without color. Avoid dependencies on emoji or Nerd Fonts.

Use Rich `Live`, `Layout`, `Panel`, `Table` and a battery progress bar. A full-screen alternate buffer should restore the user's prompt on exit. Render at a maximum of four refreshes per second so ages and connection progress update smoothly; new measurements still arrive at the station's pace. These capabilities are described in the [Rich Live documentation](https://rich.readthedocs.io/en/stable/live.html).

The following wireframe illustrates one captured sample, not current readings:

```text
ALLPOWERS S300                                  LIVE   Last update 0.4s ago
AP S300 V2.0  |  2A:02:01:48:6B:D0  |  Actions hci2

BATTERY  99%  [=============================== ]

INPUT                OUTPUT                STATION TIME ESTIMATE
55 W                 25 W                  6h 40m
trend over 120s       trend over 120s        Reported by station

REPORTED FLAGS       AC On     DC Off     Light Off

CONNECTION           Actions F4:4E:FC:A1:CB:FF
Last scan RSSI       -85 dBm, measured during discovery
SPECIFICATIONS       288 Wh nominal  |  300 W continuous  |  500 W peak

DIAGNOSTICS          Valid 12  |  Rejected 0  |  Reconnects 0
Flags 0x0e           Frame a565b1000108010e63003700190190ab
EVENTS               12:25:42 Telemetry available

Ctrl+C Exit
```

At 100 columns by 28 rows or larger, show the complete dashboard. At 80 by 24, stack the main readings and condense identity, diagnostics and events. For smaller terminals, retain connection state, battery, input, output, remaining time and update age; hide secondary areas first. Below 40 by 12, show a readable request to enlarge the terminal. Resize must not crash the session.

Keep a 120-second in-memory history for input and output trends and the ten latest connection or validation events. Trends represent actual sample timestamps, with gaps on disconnect; do not fabricate samples or bridge outages. No persistent history or export is required initially.

The event list contains transitions and concise failures, not every incoming notification. Prevent device-provided names or error text from being interpreted as Rich markup or terminal control sequences.

### Data freshness and recovery

All thresholds below are proposed product defaults, not claims about the station firmware.

| State | Trigger | Required behavior |
| --- | --- | --- |
| Scanning | Discovery underway | Show adapter, progress and no live readings |
| Connecting | Device found, GATT setup underway | Show device identity and no live readings |
| Waiting for telemetry | Notifications enabled, no valid frame yet | Do not show LIVE; values remain Unknown |
| Live | Connected, valid sample age under 10 seconds | Normal dashboard |
| Stale | Connected, sample age at least 10 seconds | Mark readings stale and show their age |
| Reconnecting | Link lost, or no valid sample for 30 seconds | Keep explicitly labeled last-known values; run recovery |
| Configuration error | Missing adapter, blocked adapter, unavailable service or permission denied | Show an actionable message; avoid a retry loop that cannot fix configuration |

Use monotonic time for sample age, deadlines and trend windows; wall-clock time is for display. Invalid notifications do not reset the freshness clock. On every new connection, wait for a new valid frame before returning to Live; values from an earlier session remain last-known only.

For recoverable connection failures, retry serially after 2, 5, 10 and then 30 seconds, repeating the 30-second delay thereafter. Reset the delay after valid telemetry is received. Use a fresh scan and `BLEDevice` for each attempt. Never run overlapping scan/connect attempts. Do not automatically power-cycle adapters, change pairing or switch controllers.

After repeated failures, display a concise suggestion to disconnect the phone app, check USB passthrough or toggle Bluetooth on the station. Keep these troubleshooting details in the status/event area. Ctrl+C must cancel discovery, retries and monitoring, disconnect where possible and restore the terminal promptly.

## Application structure and command interface

Use Python 3.12 or newer, `asyncio`, Bleak 3.0.2 as the tested transport baseline, Rich and `dbus-fast` for explicit BlueZ adapter discovery. Declare direct dependencies and lock tested versions when implementation begins. Manage the environment with `uv`; use `uv pip` if package installation is needed.

Keep four small concerns separate: validated protocol parsing, BLE session lifecycle, telemetry/history state and Rich rendering. A CLI entry point connects them in one process with one asyncio event loop. A database, plugin system, generalized device framework or subprocess wrapper around `bluetoothctl` is not required. The existing `read_allpowers.py` is a verified reference; the future application should own its transport lifecycle rather than launch that script and parse its stdout.

The BLE callback should validate a frame and publish a small immutable sample. Rendering and connection work happen outside it. Store the latest sample and a bounded history; do not create an unbounded queue of redraw requests. Failed frames leave the last valid sample intact but update diagnostic counters.

The proposed command is not implemented by this PRD:

```text
allpowers-tui
allpowers-tui --address 2A:02:01:48:6B:D0 --adapter F4:4E:FC:A1:CB:FF
allpowers-tui --adapter hci2
allpowers-tui --list-adapters
```

Defaults are this station's address and the verified Actions controller address. Accept either an adapter address or an `hciN` name for an explicit override. `--list-adapters` should print controller name, address, current `hciN` and availability, then exit without contacting the station. No configuration file is needed initially. Ctrl+C is the only required keyboard action; Rich is the renderer, and a separate keyboard framework is unnecessary for this scope.

The normalized sample contains `battery_percent`, `input_power_w`, `output_power_w`, `remaining_minutes`, `ac_enabled`, `dc_enabled`, `light_enabled`, `raw_flags`, `raw_frame`, UTC receive time and monotonic receive time. Store advertising RSSI separately with its measurement time. Do not mix discovery metadata into the station status packet.

## Acceptance criteria

1. With station Bluetooth enabled and the phone disconnected, the application resolves the Actions controller, connects and shows verified live values from `fff1` without sending control commands. Targets are at most 20 seconds for discovery, 25 seconds for connection setup and 10 seconds for the first valid sample; exceeding a target produces a visible state and recovery behavior.
2. The captured frame `a565b1000108010e63003700190190ab` produces exactly 99%, 55 W, 25 W, 400 minutes, AC true, DC false, light false and flags 14.
3. Truncated data, a changed checksum byte, an invalid header, an unsupported command/layout and battery values above 100 never update live measurements. Valid zeros and integer boundaries decode correctly.
4. Freshness is based only on valid samples. Injected time advances cause Stale at 10 seconds and recovery at 30 seconds, and disconnected readings are visibly last-known rather than live.
5. Removing the adapter or disabling station Bluetooth updates the UI without a traceback taking over the screen. After availability returns, the serial retry loop restores telemetry without restarting the application.
6. A changed `hciN` number still selects the preferred controller by address. A missing preferred controller is reported instead of silently using another adapter.
7. The dashboard is legible at 100×28 and 80×24, handles resizing and no-color terminals, and does not let device text inject terminal commands or Rich markup.
8. Ctrl+C during scanning, connecting, live monitoring and retry delay restores the terminal and leaves no background process or active application connection.
9. An implementation validation session runs for 60 minutes on the actual Actions adapter, with bounded history and memory, multiple notifications and no control writes. Injected disconnect and corrupt-frame tests cover behavior that is impractical to induce reliably on hardware.

Use parser fixtures from the captured frames, simulated time for freshness/retry tests, a mocked BLE boundary for lifecycle tests and rendering snapshots at the required terminal sizes. Live hardware validation complements these tests. The existing evidence covers short successful sessions, not a completed 60-minute stability run.

## Implementation order

First deliver the smallest working path: resolve Actions, scan, connect, validate `fff1` notifications and show battery, power, time and freshness in Rich. Then add serial reconnection and cleanup. Finally add bounded trends, responsive layout, diagnostics and the acceptance tests. Keep a working live monitor at each stage.

No application files are created by this specification. If implementation is requested later, prefer a dedicated utility in the `scriptoza` repository rather than expanding the unrelated temporary workspace project.

## Evidence and limitations

- [ALLPOWERS.md](ALLPOWERS.md) records the hardware discovery, failed routes and successful captures.
- [read_allpowers.py](read_allpowers.py) is the existing, tested notification reader and parser. It predates this PRD and is not the requested future TUI.
- [BLUETOOTH.md](BLUETOOTH.md) records adapter identification and USB passthrough observations.
- The [S300 reference implementation](https://github.com/madninjaskillz/AllPowersS300ESP32HomeAssistant/blob/main/APS300.ino) supplies the interpreted flag mapping; actual frame offsets, length and checksum were verified against this unit.
- Other firmware revisions, undocumented services, unknown flags, fragmented notifications, individual port telemetry and the semantic meaning of the remaining-time field while charging are unverified. The UI must retain these limits rather than derive unsupported product states.
