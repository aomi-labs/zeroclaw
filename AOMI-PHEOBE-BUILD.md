# AOMI / Phoebe Build And Deploy

This is the current reproducible path for building `zeroclaw` on a local macOS machine and deploying it to the current Phoebe server.

## Current topology

- Local dev machine:
  - repo: `/Users/cecilia/zeroclaw`
  - host OS/arch: `aarch64-apple-darwin`
- Remote server:
  - host: `root@100.69.50.58`
  - app repo: `/root/zeroclaw-aomi`
  - live binary used by the daemon: `/root/.cargo/bin/zeroclaw`
  - systemd user unit: `/root/.config/systemd/user/zeroclaw.service`
  - injected service env: `/root/.config/systemd/user/zeroclaw.service.d/env.conf`

## The important local vs server difference

The server daemon gets API keys and other runtime secrets from the user-systemd unit drop-in, not from an interactive shell.

That means:

- `systemctl --user restart zeroclaw.service` preserves the real production env.
- Manually running `/root/.cargo/bin/zeroclaw daemon` does **not** automatically get those keys.
- If a manual smoke test is needed, load the same env source first or test through the systemd-managed service.

This is the main reason a local build can work after deploy but a manually started remote process can appear broken.

## What must stay the same for a true drop-in deploy

A binary replacement is only a real drop-in if the runtime contract stays compatible.

These should remain the same unless the deploy also updates the server setup:

- service command and binary path
  - current expectation: `ExecStart=/root/.cargo/bin/zeroclaw daemon`
- environment variables
  - current expectation: values come from `/root/.config/systemd/user/zeroclaw.service.d/env.conf`
- config schema and required config fields
  - the new binary must still accept the server’s existing config file(s)
- filesystem layout
  - same working directories, SQLite/session paths, cache paths, and repo-relative assets if referenced
- ports and listeners
  - same dashboard / health endpoints, same channel listeners, same webhook expectations
- provider/runtime integrations
  - same auth model, proxy assumptions, outbound network requirements, and external API expectations
- persistent data format
  - same DB/session compatibility, or a safe forward migration that the new binary performs on startup

If any of those change, the deploy is no longer “just replace the binary and restart”.

## One-time local setup

Install the Linux cross-build toolchain on the local Mac:

```bash
rustup target add x86_64-unknown-linux-musl
brew install zig
cargo install cargo-zigbuild
```

## Build the Linux binary locally

Use the repo root:

```bash
cd /Users/cecilia/zeroclaw
cargo zigbuild --profile ci --target x86_64-unknown-linux-musl --bin zeroclaw
```

Artifact:

```bash
target/x86_64-unknown-linux-musl/ci/zeroclaw
```

Why `--profile ci`:

- the default `release` profile uses fat LTO and is much slower
- `ci` still produces an optimized binary but is practical for repeat deploys

## Verify the local artifact

```bash
file target/x86_64-unknown-linux-musl/ci/zeroclaw
shasum -a 256 target/x86_64-unknown-linux-musl/ci/zeroclaw
```

Expected binary type:

```text
ELF 64-bit LSB executable, x86-64, statically linked
```

## Copy to the server

Ship the built artifact to the repo directory on the remote host first:

```bash
scp target/x86_64-unknown-linux-musl/ci/zeroclaw \
  root@100.69.50.58:/root/zeroclaw-aomi/zeroclaw.x86_64-unknown-linux-musl
```

Verify remotely:

```bash
ssh root@100.69.50.58 '
  file /root/zeroclaw-aomi/zeroclaw.x86_64-unknown-linux-musl &&
  sha256sum /root/zeroclaw-aomi/zeroclaw.x86_64-unknown-linux-musl
'
```

## Replace the live binary safely

Back up the current live binary, then replace it:

```bash
ssh root@100.69.50.58 '
  set -e
  ts=$(date +%Y%m%d%H%M%S)
  cp /root/.cargo/bin/zeroclaw /root/.cargo/bin/zeroclaw.backup-$ts
  install -m 0755 /root/zeroclaw-aomi/zeroclaw.x86_64-unknown-linux-musl /root/.cargo/bin/zeroclaw
'
```

## Restart the real daemon

Always restart the user-systemd unit, not a manual background process:

```bash
ssh root@100.69.50.58 '
  systemctl --user restart zeroclaw.service &&
  systemctl --user status zeroclaw.service --no-pager
'
```

Useful checks:

```bash
ssh root@100.69.50.58 '
  systemctl --user show zeroclaw.service -p MainPID -p ExecStart -p Environment --value
'
```

## Smoke test after deploy

Quick one-shot CLI test:

```bash
ssh root@100.69.50.58 '
  bash -lc '\''eval "$(grep "^Environment=" /root/.config/systemd/user/zeroclaw.service.d/env.conf | sed "s/^Environment=/export /")"
  /root/.cargo/bin/zeroclaw agent -m "Reply with exactly: PONG"'\'''
'
```

If that is awkward, the more important test is simply:

```bash
ssh root@100.69.50.58 '
  systemctl --user restart zeroclaw.service &&
  journalctl --user -u zeroclaw.service -n 100 --no-pager
'
```

## Current production-specific paths

These are the paths the current Phoebe deployment depends on:

- repo checkout: `/root/zeroclaw-aomi`
- staged uploaded binary: `/root/zeroclaw-aomi/zeroclaw.x86_64-unknown-linux-musl`
- live binary: `/root/.cargo/bin/zeroclaw`
- systemd unit: `/root/.config/systemd/user/zeroclaw.service`
- env drop-in: `/root/.config/systemd/user/zeroclaw.service.d/env.conf`
- stdout log file used during earlier manual debugging: `/root/zeroclaw-daemon.out`
- stderr log file used during earlier manual debugging: `/root/zeroclaw-daemon.err`

## Reproducing this on a different machine

To reproduce the same deploy flow on another local machine:

1. Clone the repo to any local path.
2. Install `zig`, `cargo-zigbuild`, and the `x86_64-unknown-linux-musl` target.
3. Build with:

```bash
cargo zigbuild --profile ci --target x86_64-unknown-linux-musl --bin zeroclaw
```

4. Copy the resulting artifact to the remote Linux server.
5. Replace the live binary with `install -m 0755`.
6. Restart via `systemctl --user restart zeroclaw.service`.
7. Do not rely on a manually started daemon unless you intentionally replicate the service env.

## Current status

As of this note, the latest local code changes may exist in `/Users/cecilia/zeroclaw` before they are rebuilt and deployed. Rebuild and restart are separate steps; editing local code does not update the server automatically.
