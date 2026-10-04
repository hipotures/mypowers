# Bluetooth environment inspection

## 2026-10-04

- Checked available hardware with `lsusb`, `lspci -nn`, `rfkill list`, and `/sys/class/bluetooth`.
- No Bluetooth USB or PCI adapter was visible; `rfkill list` was empty and `/sys/class/bluetooth` did not exist.
- `bluetoothctl list` produced no output and did not finish within 10 seconds; it was interrupted. This check alone is inconclusive.
- `systemd-detect-virt` reported `qemu`. These findings concern this guest environment, not the physical host. A host adapter may exist without being passed through to the guest.
- USB devices included Logitech M185 and Logi Bolt receivers, an ASUS AURA LED controller, and a Yubikey; no general Bluetooth controller was identified.
- No system configuration was changed. This file is the only generated artifact.

### Follow-up: adapter now present

- A repeat check detected USB device `0a12:0001`, Cambridge Silicon Radio Bluetooth Dongle, product `CSR8510 A10`.
- Kernel logs recorded its arrival on guest USB port `9-10` at 12:04:51 and initialization through `btusb` as `hci0`.
- `bluetoothctl list` reported controller `00:1A:7D:DA:71:11` (`cachyos`). `bluetoothctl show` confirmed `Powered: yes`, `Pairable: yes`, and `Discovering: yes`.
- `rfkill list` showed neither a software nor hardware block; `systemctl is-active bluetooth` returned `active`.
- The earlier absence is superseded by this observation. No configuration changes or pairing operations were performed.

### Additional adapters passed through

- Subsequent USB passthrough exposed Intel AX200 Bluetooth (`8087:0029`) as `hci1` and Actions general adapter (`10d7:b012`) as `hci2`, alongside CSR `hci0`.
- `bluetoothctl list` and `rfkill list` confirmed all three controllers; none was blocked. Actions is now positively identified as a Bluetooth controller.
- Intel AX200 Bluetooth is visible through USB in the guest even though the host screenshot lists the Wi-Fi function as PCIe. No PCIe passthrough was performed by the agent.
- ALLPOWERS telemetry succeeded through Actions `hci2`; see `ALLPOWERS.md` and `read_allpowers.py`.
