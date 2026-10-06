# Deployment

The application does not provision Proxmox/LXC/HomeStack, accounts, system packages, USB/D-Bus
exposure, DNS or backups. Verify those external prerequisites in the actual target container.
The existing research and local validation use a QEMU guest and do not prove LXC passthrough.

Use Linux, Python 3.12+ with SQLite ≥3.37, BlueZ/system D-Bus and account permissions. Run one
foreground `mypowersd`, never Uvicorn reload/multiple workers for real BLE. An OS file lock is
keyed by adapter/station in the runtime directory; it cannot protect against phones/other hosts
or a daemon deliberately using another runtime directory. Do not start simultaneous probes.

## Locked wheel installation

Build once using the committed lockfile, then install outside the checkout. Server exports
include exact runtime versions and hashes. The deployer chooses a suitable interpreter; MyPowers
never creates an application venv or installs packages during startup.

```bash
uv build
# An appropriately provisioned single-purpose system interpreter:
uv pip install --system --require-hashes -r deploy/requirements-server.txt
uv pip install --system --no-deps dist/mypowers-0.1.0-py3-none-any.whl
# Build both native client binaries separately:
cargo build --release --locked --manifest-path frontends/tui/Cargo.toml
# Copy mypowers and mypowers-tui from frontends/tui/target/release/ to the client PATH.
```

If the OS marks Python externally managed, provision an appropriate interpreter or select an
operator-managed environment. Do not bypass that policy with automatic `--break-system-packages`.
For an operator-selected venv, `uv venv /chosen/path` and `uv pip install --python /chosen/path/bin/python`
are equivalent build/install-time options, never runtime behavior. The Python distribution provides `mypowers[server]`.
CLI and TUI are native Rust binaries and need no Python or client dependency export.
Install both with `cargo install --locked --path frontends/tui`.
Requirements exports allow locked deployment
rather than resolving unspecified new dependencies at the destination.

The deployer creates `/etc/mypowers/config.yaml`, `/etc/mypowers/server.env`, private API token,
`/var/lib/mypowers`, `/var/log/mypowers`, `/run/mypowers` with service-account ownership.
Use `config/production.example.yaml` and `.env.production.example` as starting artifacts; check
executable paths in the supervisor examples. A dedicated `mypowers` account is recommended but
account provisioning is external. BlueZ must expose Actions `F4:4E:FC:A1:CB:FF` to that account.

```bash
mypowersd token generate --output /etc/mypowers/api-token
mypowersd check-config --env-file /etc/mypowers/server.env
mypowersd --env-file /etc/mypowers/server.env
```

Copy/adapt either `systemd/mypowers.service` or `openrc/mypowers` to your supervisor. Both run the
same foreground executable, explicitly select dotenv, allow SIGTERM cleanup and delay restart.
Station absence is an observable API state, not a process crash. Do not enable device-isolation
options without testing BlueZ access. Service/account installation and activation are operator work.

## Caddy and production DNS-01

Caddy shares the LXC with the backend and proxies to `127.0.0.1:8765`. The public origin is exactly
`https://mypower.efez.net`; local DNS points it to the container's LAN address. Public DNS must permit
TXT changes at `_acme-challenge.mypower.efez.net`. DNS-01 needs no public A/AAAA or inbound Internet
ports; it needs outbound DNS-provider/API/ACME access and persistent writable Caddy certificate state.
Caddy handles certificate issuance, renewal and WSS proxying with modern defaults.

The DNS provider, Caddy provider module and credential are **external inputs not supplied here**.
`Caddyfile.production.example` requires `/etc/caddy/mypowers-dns01.caddy`, defining a `dns01` snippet.
The missing snippet fails startup; there is no HTTP-01 or internal-CA production fallback.
Select the real provider module and use its [official syntax](https://caddyserver.com/docs/caddyfile/directives/tls).
An operator-authored snippet has this provider-specific shape:

```caddyfile
(dns01) {
    tls {
        dns REAL_PROVIDER PROVIDER_SPECIFIC_CONFIGURATION
    }
}
```

`REAL_PROVIDER` and configuration must be replaced with the chosen module's documented syntax.
Keep the DNS credential only in the Caddy service's private environment/configuration. It never
belongs in MyPowers dotenv, client files or Git. Verify with `caddy list-modules` and `caddy validate`.
Production certificate issuance, target LXC Bluetooth acquisition and LAN client trust remain
separate acceptance checks; local TLS verification does not prove them.

## Isolated local TLS integration

`Caddyfile.local-test` uses localhost:18443, a Caddy internal CA, disabled automatic trust installation,
no privileged port, and configurable test backend port. State stays in selected temporary XDG paths.
The standalone Caddy integration test starts/stops only its own proxy and verifies both HTTPS and
WSS with the explicit test CA; it also verifies that default trust rejects this certificate.

```bash
MYPOWERS_CADDY_TEST_BINARY=/absolute/path/to/caddy \
  uv run pytest tests/integration/test_tls.py -q
```

The default offline suite skips this optional external-binary test. The release validation report
records its explicit execution and Caddy version. No CA is installed into system trust. For a manual
client, pass `--ca-file PATH_TO_TEST_ROOT_CRT`; TLS verification remains enabled.

## Offline bundle for the Alpine Python LXC

Build a daemon bundle on the development machine:

```bash
uv run python scripts/build-install-bundle.py
```

The default target is CPython 3.14 on musl x86_64. Override
`--python-version` and `--platform` for other targets. The archive in `dist/`
contains the application wheel, all server dependency wheels selected from
`uv.lock`, their hash-locked requirements, installer and Supervisor configuration.
Native CLI/TUI binaries are separate client installations.

Copy the archive to the LXC, extract it, then run as root:

```sh
tar -xzf mypowers-install-py314-musllinux_1_2_x86_64.tar.gz
python3 mypowers-install/install.py \
  --hostname mypowers.lxc.efez.net --startup-python /startup.py
```

The installer uses Alpine repositories for Supervisor, CA certificates and timezone
data; Python dependencies install offline without a compiler. It creates a
`mypowers` service account, a release-specific venv under `/opt/mypowers/releases`
and the `current` symlink. Existing daemon configuration, token and database are
preserved. The initial configuration uses real BLE; unavailable Bluetooth is
reported by the daemon and does not prevent HTTPS/API operation.

This boot integration is specifically for the inspected Python LXC image whose
`/startup.py` starts SSH then sleeps forever. Other entrypoints are rejected.
It backs up that script as `/startup.py.pre-mypowers`, retains SSH startup and
executes Supervisor to run/restart MyPowers and Caddy. It also recreates the
daemon runtime directory after reboot. Caddy currently runs as root to retain
the certificate storage used by the initial manual setup. Its OVH credentials
remain in `/etc/conf.d/caddy`. The Caddyfile is backed up once and changed to
proxy the supplied hostname to `127.0.0.1:8765`.

For the first start, stop any manually running Caddy and run:

```sh
/usr/bin/supervisord -c /etc/mypowers/supervisord.conf
```

After reboot the image entrypoint starts it automatically. Inspect services with:

```sh
supervisorctl -c /etc/mypowers/supervisord.conf status
tail -n 50 /var/log/mypowers/daemon-console.log
tail -n 50 /var/log/mypowers/caddy.log
```

For an update, install a new bundle, then restart only the daemon:

```sh
supervisorctl -c /etc/mypowers/supervisord.conf restart mypowers
```

Older release directories are retained. To roll back the code, point
`/opt/mypowers/current` to the previous release printed by the installer, then
restart the daemon. Database migration compatibility must be checked separately.

## Verified Bluetooth deployment: pve2 / LXC 110 (2026-10-06)

The production endpoint is `https://mypowers.lxc.efez.net`; LXC 110 is
`192.168.100.222`, Alpine 3.24.2 / Python 3.14.8. It remains unprivileged.
PID 1 is Supervisor, reached through the existing `/startup.py`; services
use `/etc/mypowers/supervisord.conf`.

### Controller allocation

| Controller | Identity | Physical host and assignment |
|---|---|---|
| Baseus / Actions | `10d7:b012`, `F4:4E:FC:A1:CB:FF` | pve2 USB `1-5.4`, host BlueZ `hci0`, used by LXC 110 |
| Built-in MediaTek MT7922 | `0e8d:0616`, `A8:3B:76:E6:D4:A0` | pve2 USB `1-7`, VM 210 `usb0: host=1-7`, reserved for dev |
| Built-in Intel AX200 | `8087:0029`, `64:BC:58:16:C6:59` | pve1 USB `1-6`, desktop VM 100 `usb10`, retained |
| Ugreen / CSR8510 A10 | `0a12:0001`, `00:1A:7D:DA:71:11` | Physically disconnected; desktop's existing `usb9: host=5-4` was left unchanged |

Actions was originally on pve1 USB `5-2`, desktop VM 100 `usb13`.
The user physically moved it to pve2; only that stale `usb13` assignment
was removed after backing up VM 100. Brand mapping was established by
plugging each dongle separately and reading USB IDs. CSR BLE reliability
was not qualified. Controller indices and device numbers can change:
resolve by MAC and rediscover the physical USB path before modifying assignments.
The Bluetooth interface of the MediaTek card is USB; its associated Wi-Fi
function is PCI `0000:0c:00.0` on pve2.

The station is `2A:02:01:48:6B:D0` / `AP S300 V2.0`. The configured
adapter address `F4:4E:FC:A1:CB:FF` is now the measured production controller,
rather than an assumed example. Dev's foreground daemon in VM 210 was stopped
with SIGTERM, its station connection was confirmed disconnected, and the user
also disconnected another host before production acquisition. Keep dev paused
while production owns the station.

### Host BlueZ and restricted system D-Bus

LXC shares the host kernel. This installation uses pve2's BlueZ 5.82 and
system D-Bus; no USB device nodes, HCI capabilities, host network namespace,
local BlueZ/OpenRC service, privileged-container conversion or AppArmor
disablement was required. Host Bluetooth starts through systemd, while the
existing Supervisor starts the application inside the LXC.

The inspected UID map is `0 -> 100000, length 65536`; container user
`mypowers` has UID 101 and is seen by the host as UID 100101.
A locked host account, `mypowers-lxc110`, identifies that mapped UID.
Confirm the map again before reproducing this configuration:

```sh
# Run these host commands in a separate shell process, not with set -e
# in the user's interactive SSH shell.
pct exec 110 -- cat /proc/self/uid_map
pct exec 110 -- id mypowers
apt-get update
apt-get install --no-install-recommends bluez
useradd --system --uid 100101 --no-create-home --shell /usr/sbin/nologin mypowers-lxc110
systemctl enable --now bluetooth.service
bluetoothctl list
```

Create `/etc/dbus-1/system.d/90-mypowers-lxc110.conf` on pve2:

```xml
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <policy user="mypowers-lxc110">
    <deny own="*"/>
    <deny send_destination="*"/>
    <allow send_destination="org.bluez"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus" send_member="Hello"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus" send_member="AddMatch"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus" send_member="RemoveMatch"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus" send_member="GetNameOwner"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus" send_member="NameHasOwner"/>
    <allow send_destination="org.freedesktop.DBus" send_interface="org.freedesktop.DBus" send_member="GetId"/>
    <deny receive_sender="*"/>
    <allow receive_sender="org.bluez"/>
    <allow receive_sender="org.freedesktop.DBus"/>
    <allow receive_requested_reply="true"/>
  </policy>
</busconfig>
```

Reload policy without restarting host D-Bus, then persist the read-only
directory mount outside the image's volatile `/run`. Mounting the directory
also accommodates socket recreation:

```sh
dbus-send --system --type=method_call --print-reply \
  --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig
pct exec 110 -- mkdir -p /host-dbus
pct set 110 -mp0 /run/dbus,mp=/host-dbus,ro=1
```

The resulting `/etc/pve/lxc/110.conf` contains:

```text
unprivileged: 1
mp0: /run/dbus,mp=/host-dbus,ro=1
```

The application's explicit adapter resolver uses
`AuthExternal(uid=UID_NOT_SPECIFIED)`, allowing D-Bus to authenticate the
kernel's peer credentials across the user namespace. Bleak opens its own
connections, so `[program:mypowers]` also needs this Supervisor setting,
included in `deploy/bundle/supervisord.conf`:

```ini
environment=BLEAK_DBUS_AUTH_UID="-1",DBUS_SYSTEM_BUS_ADDRESS="unix:path=/host-dbus/system_bus_socket"
```

The initial `/run/dbus` destination disappeared when the image recreated `/run`
after reboot. `/host-dbus` and `DBUS_SYSTEM_BUS_ADDRESS` were verified across
a subsequent container restart. Both connections use EXTERNAL authentication; anonymous authentication is not enabled.
After a Supervisor configuration change, stop any other station clients,
then use `reread` and `update mypowers`. No system bus daemon is started
inside this container.

The read-only mount does not make D-Bus calls read-only. The policy restricts
the mapped service account to BlueZ and required bus operations, but grants
access to **all adapters and devices exposed by host BlueZ**. The application's
MAC selection is not an adapter security boundary or an exclusive reservation.
Container root can assume the service UID; this setup trusts the container's
administration. Dev isolation currently comes from its separate USB passthrough
into VM 210, outside host BlueZ. A separate VM or more restrictive broker would
be needed for stronger hardware isolation.

References: [Bleak Linux authentication](https://bleak.readthedocs.io/en/latest/backends/linux.html),
[D-Bus authentication and policy](https://dbus.freedesktop.org/doc/dbus-daemon.1.html).

### Package deployment and recovery

Runtime authentication fix: commit `2620bc8c38a014928f975b4806080179b97624ee`.
The dev Codex verified 24 Bluetooth tests, Ruff lint/format and mypy, then built
`dist/mypowers-0.1.0-py3-none-any.whl`, SHA256
`6a191a0763e27319987b5dea8e040245826a8a96791bb7f72fab57d93a65a02d`.
Transfer was performed through Proxmox's guest agent and `pct push`;
no private SSH/YubiKey keys or API/DNS credentials were copied into the repository.

Deployed release: `/opt/mypowers/releases/2620bc8c38a0-1791297900`.
Its `deployment.json` records source commit, wheel hash and previous release.
A fresh venv was created with the container's Python and uv 0.11.19. Locked
server dependencies were installed offline from the existing Alpine bundle
using `uv pip install --require-hashes --no-index --find-links`; the
application wheel was then installed with `uv pip install --no-deps --no-index`.
`uv pip check` and `mypowersd check-config` passed before switching
`/opt/mypowers/current` atomically. Configuration, token, database and
Caddy/OVH credentials were retained. The updated Supervisor bundle includes
the Bleak authentication environment for future bundle deployments.

Before changes, configuration backups were created in
`/root/mypowers-bluetooth-20261006T142857Z`:

- On pve2: LXC 110 and VM 210 configuration, the original VM 100 configuration,
  host D-Bus configuration and the deployed wheel.
- Inside LXC 110: `/etc/mypowers`, `/startup.py`, the original installed
  Bluetooth transport and `previous-release.txt`.
- On pve1: VM 100 configuration before removing only Actions' `usb13`.
- Inside LXC: `database-before-wheel.db`, created with SQLite Online Backup
  API and checked with `PRAGMA integrity_check` (`ok`). No migration
  experiments were performed on the production database.

The previous release remains at
`/opt/mypowers/releases/551117b89ba1-1791292280982506015`, including the
initially verified authentication fix applied during this rollout.
To roll back the package, stop MyPowers, atomically repoint `current` to
that release and start MyPowers. Keep the shared D-Bus/Supervisor configuration;
do not automatically restore the database snapshot, which would discard newer
telemetry.

To remove this Bluetooth integration, stop production acquisition first,
remove `mp0` from LXC 110, remove the dedicated D-Bus policy and reload D-Bus,
and restore the saved Supervisor configuration. Apply mount removal with
a separately approved container restart if needed. Disable host BlueZ only
if no other workload has begun using it. Restore VM 100's `usb13: host=5-2`
only if Actions has physically returned to that port on pve1. Do not overwrite
unrelated subsequent VM configuration changes with a whole-file backup.

### Verification

- As container user `mypowers`: resolver selected `hci0` /
  `F4:4E:FC:A1:CB:FF`; Bleak scan found the exact S300, RSSI -82 dBm.
- The same account received `org.freedesktop.DBus.Error.AccessDenied`
  for host systemd `ListUnits`.
- HTTPS with normal certificate validation and the existing bearer token:
  health HTTP 200, connection `connected`, telemetry `live`,
  sample age below 3 seconds. Token values were never printed.
- WSS `/api/v1/events` returned distinct increasing live sample sequences.
  Example after package deployment: sequences 2 and 3, battery 87%,
  input 3 W, output 0 W, age 0.195 s.
- After the approved container restart, boot ID changed from
  `6ee77503-cd1a-4b96-a32f-7f61d46309fb` to
  `bd3a40f7-ef50-4a41-9307-87c98de36f89`; PID 1 remained Supervisor,
  both services started automatically, and the `/host-dbus` socket persisted.
  HTTPS returned LIVE sample age 0.679 s; WSS delivered sequences 1 and 2.
  The host and VMs were not restarted.
- No AC/DC/LIGHT commands were sent. Output state values above were read only.

Check services using
`supervisorctl -c /etc/mypowers/supervisord.conf status`, inspect
`/var/log/mypowers/daemon-console.log`, and verify the authenticated
`/api/v1/status` plus WSS events. Read the token locally from
`/etc/mypowers/api-token`; never put it in command-line URLs or chat.
Before any dev BLE test, stop or pause production acquisition and verify its
BlueZ connection has been released. Only then start dev. Pause dev again before
returning station ownership to production.

### Restored production history (2026-10-06)

At the user's request, MyPowers was stopped and its active database was replaced
from `/root/data/mypowers.db`. This source uses schema v2 and contains 22,071
telemetry states for the exact S300, covering
`2026-10-04T18:52:48.237Z` through `2026-10-06T14:37:54.131Z`.
The neighboring `/root/data/mypowers_old.db` is an older schema-v1 database
with a shorter range; it was not used or migrated.

The source was copied with SQLite Online Backup API into a staged database on
the destination filesystem. Integrity, foreign keys, schema/columns and station
identity were checked before replacement. The stopped destination was also
backed up with SQLite Online Backup API to
`/root/mypowers-bluetooth-20261006T142857Z/database-before-history-import.db`.
After a successful WAL checkpoint and verification that no process held the
destination database open, stale sidecars were removed and the staged file was
atomically installed with the original service ownership and permissions.
No plain file copy of a SQLite database was used; both source databases remain
available. Import metadata is recorded in the same backup directory as
`history-import.json`.

MyPowers was started again and HTTPS/WSS returned fresh LIVE telemetry
(sample age 0.204 s; increasing sequences 2 and 3). An authenticated HTTPS
history query for October 4 returned the imported records. The active database's
integrity check remained `ok`, with the imported rows present and new
production telemetry continuing. Dev was verified still stopped.

To revert this data replacement, stop MyPowers and use SQLite Online Backup API
to stage `database-before-history-import.db` for the active database, check
integrity and schema, then atomically replace it after releasing all connections
and handling its WAL/shm sidecars. Restore service ownership before starting
MyPowers. This deliberately discards telemetry recorded after the import.
