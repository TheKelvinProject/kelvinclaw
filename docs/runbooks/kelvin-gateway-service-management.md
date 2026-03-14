# Kelvin Gateway Service Management

KelvinClaw now ships two supported service-management layers for `kelvin-gateway`:

- ad hoc local background management via `scripts/kelvin-gateway-daemon.sh`
- user-service definitions for `systemd` and `launchd` via `scripts/kelvin-gateway-service.sh`

The service definitions execute `scripts/kelvin-gateway-service-run.sh`, which:

- sources an optional env file (`KELVIN_GATEWAY_SERVICE_ENV_FILE`, default `./.env`)
- builds `target/debug/kelvin-gateway` if it is missing and Cargo is available
- starts the gateway in the foreground with persisted state and HTTP ingress enabled by default

## systemd user service

Render the unit:

```bash
scripts/kelvin-gateway-service.sh render-systemd-user
```

Install the unit into `~/.config/systemd/user/kelvin-gateway.service`:

```bash
scripts/kelvin-gateway-service.sh install-systemd-user
systemctl --user daemon-reload
systemctl --user enable --now kelvin-gateway.service
```

Override paths and binds if needed:

```bash
scripts/kelvin-gateway-service.sh install-systemd-user \
  --env-file "$PWD/.env" \
  --workspace "$PWD" \
  --state-dir "$PWD/.kelvin/gateway-state" \
  --bind 127.0.0.1:34617 \
  --ingress-bind 127.0.0.1:34618
```

## launchd user agent

Render the plist:

```bash
scripts/kelvin-gateway-service.sh render-launchd
```

Install the LaunchAgent into `~/Library/LaunchAgents/dev.kelvinclaw.gateway.plist`:

```bash
scripts/kelvin-gateway-service.sh install-launchd
launchctl bootout gui/$(id -u) dev.kelvinclaw.gateway 2>/dev/null || true
launchctl bootstrap gui/$(id -u) "$HOME/Library/LaunchAgents/dev.kelvinclaw.gateway.plist"
```

Launchd logs default to `./.kelvin/gateway-daemon/kelvin-gateway.out.log` and
`./.kelvin/gateway-daemon/kelvin-gateway.err.log`.

## Environment model

Put secrets and channel credentials in the env file consumed by the service runner, for example:

- `KELVIN_GATEWAY_TOKEN`
- `KELVIN_GATEWAY_TLS_CERT_PATH`
- `KELVIN_GATEWAY_TLS_KEY_PATH`
- `KELVIN_TELEGRAM_WEBHOOK_SECRET_TOKEN`
- `KELVIN_SLACK_SIGNING_SECRET`
- `KELVIN_DISCORD_INTERACTIONS_PUBLIC_KEY`

The service wrapper separately accepts non-secret runtime defaults:

- `KELVIN_GATEWAY_SERVICE_BINARY`
- `KELVIN_GATEWAY_SERVICE_ENV_FILE`
- `KELVIN_GATEWAY_WORKSPACE`
- `KELVIN_GATEWAY_STATE_DIR`
- `KELVIN_GATEWAY_BIND`
- `KELVIN_GATEWAY_INGRESS_BIND`

## Cache hygiene

Shared Docker build caches live under `./.cache/docker`. To keep them bounded:

```bash
scripts/docker-cache-prune.sh --dry-run
scripts/docker-cache-prune.sh --max-age-days 14
```
