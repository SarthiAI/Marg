# Sample systemd assets

Drop these in `/etc/systemd/system/` and `/etc/security/limits.d/` to run Marg as a long-lived daemon on a single Linux host. They encode the production-grade defaults validated at saturating throughput.

## Files

| File | Where it goes | What it does |
|------|---------------|--------------|
| `marg.service` | `/etc/systemd/system/marg.service` | systemd unit for `marg start`. Pins `LimitNOFILE=1048576`, drains 45 s on stop, restarts on crash, locks down the unit with `ProtectSystem=strict` and friends. |
| `limits.d-marg.conf` | `/etc/security/limits.d/marg.conf` | Only needed for operators running Marg outside systemd (tmux, supervisord, screen). systemd's `LimitNOFILE` is authoritative when the unit is in use. |

## Install

```
sudo install -d -m 0750 -o marg -g marg /var/lib/marg /etc/marg
sudo install -m 0640 -o marg -g marg marg.toml.example /etc/marg/marg.toml
sudo install -m 0755 target/release/marg /usr/local/bin/marg
sudo install -m 0644 dist/systemd/marg.service /etc/systemd/system/marg.service
sudo systemctl daemon-reload
sudo systemctl enable --now marg
journalctl -u marg -f
```

`marg start` writes a 0600 admin bootstrap token to the path in `[admin].bootstrap_token_path` on first boot. Use it to mint your first long-lived admin token, then revoke the bootstrap token through the admin API.

## Why `LimitNOFILE=1048576`

At a few thousand concurrent connections, the Ubuntu default 1024-fd soft limit saturates and the request path starts failing with `accept error: Too many open files`. Production deployments handling more than a few thousand concurrent connections need the higher cap. SECURITY.md has carried this minimum since v1.0; this unit is the operational form of the same number.

If the soft limit is below 65,536 at startup, `marg start` emits a `tracing::warn` line pointing back at this README.
