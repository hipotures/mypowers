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
