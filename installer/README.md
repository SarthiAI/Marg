# Marg installer

A small POSIX shell script that turns the Marg install story into one
command. Reads the latest GitHub release, picks the right archive for the
host operating system and processor, verifies the SHA-256 against the
published `SHA256SUMS`, drops the `marg` binary on `PATH`, and then runs
`marg init --auto` to write a default config and mint the bootstrap admin
token. On Linux as root with systemctl present, it also installs the
bundled systemd unit and enables it.

## Quick start

```bash
# Bare metal, VM, or cloud VM (Linux x64, Linux arm64, macOS Apple Silicon)
curl -fsSL https://github.com/SarthiAI/Marg/releases/latest/download/install.sh | sh
```

The URL above is GitHub's built-in release-asset endpoint: it always
points at the `install.sh` attached to the most recent release. Pin a
specific version by swapping `latest` for the tag, e.g.
`https://github.com/SarthiAI/Marg/releases/download/v0.1.0/install.sh`.
A branded short domain (for example `get.marg.dev`) can be wired up
later as a free Cloudflare or GitHub Pages redirect; the installer does
not depend on it.

The script prints a single post-install summary that lists the config
file path, the proxy URL, the admin URL, and the bootstrap admin token.
The operator's next step is to open the admin URL in a browser and
create their first application API key.

## Environment overrides

The defaults are sized for a clean machine. Override via env vars when
you need something different.

| Variable | Default | Purpose |
|---|---|---|
| `MARG_VERSION` | `latest` | Specific release tag to install, e.g. `v0.1.0`. |
| `MARG_REPO` | `SarthiAI/Marg` | Source repo (org/repo). |
| `MARG_PREFIX` | `/usr/local/bin` | Directory the `marg` binary lands in. |
| `MARG_DEST_DIR` | (auto) | Passed to `marg init --config-dir`. Default: `/etc/marg` for root, `$HOME/.marg` otherwise. |
| `MARG_FORCE` | `0` | `1` overwrites an existing `marg` binary at the install path. |
| `MARG_NO_INIT` | `0` | `1` skips the post-install `marg init`. |
| `MARG_NO_SYSTEMD` | `0` | `1` skips the systemd unit install on Linux as root. |

Examples:

```bash
# Pin a specific version
curl -fsSL https://github.com/SarthiAI/Marg/releases/latest/download/install.sh | MARG_VERSION=v0.1.0 sh

# Skip systemd (Linux dev box where the operator manages the process)
curl -fsSL https://github.com/SarthiAI/Marg/releases/latest/download/install.sh | MARG_NO_SYSTEMD=1 sh

# Install into a user-owned prefix, no sudo, no systemd
curl -fsSL https://github.com/SarthiAI/Marg/releases/latest/download/install.sh | MARG_PREFIX=$HOME/.local/bin sh
```

## Supported targets

| OS | Architecture | Archive label |
|---|---|---|
| Linux | x86_64 | `linux-x64` |
| Linux | aarch64 | `linux-arm64` |
| macOS | Apple Silicon | `macos-arm64` |

Other targets need a from-source build. See `docs/install.md`.

## What the script will NOT do

- It will not run on a host that already has a `marg` binary unless you
  set `MARG_FORCE=1`. This is a guardrail against silently downgrading a
  production deployment.
- It will not change an existing `/etc/marg/marg.toml` or
  `/etc/marg/policy.toml`. `marg init` is idempotent and keeps your
  edits.
- It will not start the server when systemd is not in use. On a non-systemd
  host the script prints the exact `marg start --config <path>` command
  for the operator to run.

## Auditing the script before running it

Piping `curl` into `sh` is reasonable when you can read the script first.
Two safe alternatives if you would rather inspect first:

```bash
curl -fsSL https://github.com/SarthiAI/Marg/releases/latest/download/install.sh -o install.sh
less install.sh
sh install.sh
```

The script is short (under 200 lines), POSIX-clean, has no `eval`, and
the SHA-256 check on every downloaded artefact is a hard requirement.

## Uninstall

```bash
# systemd installs
sudo systemctl disable --now marg
sudo rm /etc/systemd/system/marg.service
sudo systemctl daemon-reload

# both
sudo rm /usr/local/bin/marg
sudo rm -rf /etc/marg /var/lib/marg
```

For per-user installs the equivalent is `rm "$HOME/.local/bin/marg"`
and `rm -rf "$HOME/.marg"`.
