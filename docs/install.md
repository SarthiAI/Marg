# Install

Marg ships as a single static binary. Pick the variant that matches
your machine and you are running in under a minute.

## Released binaries

Every release on GitHub publishes three archives:

- `marg-<version>-linux-x64.tar.gz`
- `marg-<version>-linux-arm64.tar.gz`
- `marg-<version>-macos-arm64.tar.gz`

Each archive contains a single `marg` binary plus a `SHA256SUMS` file
signed with the release key.

```bash
tar -xzf marg-<version>-linux-x64.tar.gz
sudo install -m 0755 marg /usr/local/bin/marg
marg --version
```

## Verifying the download

The release page publishes both the SHA256 sum and a detached signature.

```bash
sha256sum -c SHA256SUMS
```

For air-gapped or compliance setups, the source tag is reproducible
from `cargo build --release` on the same toolchain version listed in
the release notes.

## Building from source

```bash
git clone https://github.com/chirotpal/marg
cd marg/marg
cargo build --release
```

The release binary lands at `target/release/marg`. The build is
self-contained: no system OpenSSL, no Node.js (the console bundle is
pre-built and embedded), no runtime services. Rust toolchain version
is pinned in `rust-toolchain.toml`.

## Container image

```bash
docker run --rm -p 8080:8080 -p 8081:8081 \
    -v $(pwd)/marg.toml:/etc/marg/marg.toml \
    ghcr.io/chirotpal/marg:<version>
```

The container is `FROM scratch` plus the static binary plus the
console bundle. Image size sits around 16 MB without Kavach.

## Production prerequisites

The defaults above are enough to take Marg for a spin. For a node serving real traffic, two host-level settings need to be raised before the first start. Marg checks both and writes a `tracing::warn` line at boot if either is too low.

### File descriptors

Marg holds two sockets per in-flight request (client + upstream provider) plus a small per-process overhead. At a few thousand concurrent connections the Linux default of 1024 file descriptors saturates and the request path starts failing with `accept error: Too many open files`.

The production floor is **1,048,576 soft + hard**. There are two ways to set it:

```bash
# Option A: systemd unit (recommended). The included unit pins it.
sudo install -m 0644 dist/systemd/marg.service /etc/systemd/system/marg.service
sudo systemctl daemon-reload
sudo systemctl enable --now marg
cat /proc/$(pgrep -f 'marg start')/limits | grep 'Max open files'
```

```bash
# Option B: PAM limits.d (for non-systemd setups). Requires a fresh login.
sudo install -m 0644 dist/systemd/limits.d-marg.conf /etc/security/limits.d/marg.conf
```

If Marg starts with a soft limit below 65,536 (a reasonable dev floor), the boot log carries:

```
RLIMIT_NOFILE soft limit is below the recommended production floor; saturating
throughput may surface as 'accept error: Too many open files'.
```

That line is the single signal that the host needs tuning. It is logged once at startup, not per request.

### Postgres `max_connections`

Marg's Postgres pool defaults to 200 connections (`[storage].max_connections` in `marg.toml`). Postgres itself defaults to 100. If you keep Marg's default pool size, raise Postgres correspondingly:

```sql
-- On the Postgres server, as superuser:
ALTER SYSTEM SET max_connections = 300;
-- Then restart Postgres for the change to take effect.
```

Sizing rule: `postgres.max_connections >= sum(marg_instance.storage.max_connections) + admin_pool_headroom`. A 50-connection headroom is generous for `psql` sessions, migrations, and monitoring.

`/ready` returns 503 the moment a Marg instance cannot reach Postgres, so a misconfigured pool surfaces immediately via the load balancer health check.

### Process supervision

The `dist/systemd/` directory in the source tree (or in the release tarball) carries a vetted `marg.service`. Drop it into `/etc/systemd/system/`, reload, enable. Logs land in journald. SIGTERM drains in-flight requests up to 45 s before the unit is killed.

## Production checklist (seven steps, ten minutes)

These are the only operator steps a fresh Linux deployment needs to reach the measured throughput of ~6,000 req/s on a 16-core node. Skip nothing.

1. **Provision Postgres and Redis** on the same network as the Marg node. RDS + ElastiCache on AWS, equivalents on other clouds, or self-hosted on the same VPC. Cross-region adds latency to every hot-path call; do not do that.
2. **Raise Postgres `max_connections`** to at least `sum(marg_node.storage.max_connections) + 50 headroom`. Marg's default pool is 200, so a single-node deployment needs `max_connections >= 250`. Postgres default is 100. One command on the Postgres side, then restart Postgres:
   ```sql
   ALTER SYSTEM SET max_connections = 300;
   ```
   Skipping this surfaces as `too many clients already` in `marg.log` under load; `/ready` returns 503; the load balancer drops the node. No silent failure.
3. **Install the binary**: `sudo install -m 0755 marg /usr/local/bin/marg`.
4. **Install the shipped systemd unit**:
   ```bash
   sudo install -m 0644 dist/systemd/marg.service /etc/systemd/system/marg.service
   sudo systemctl daemon-reload
   ```
   The unit pins `LimitNOFILE=1048576`, sets `Restart=on-failure`, drains 45 s on stop, locks down the unit with `ProtectSystem=strict`.
5. **Drop your `marg.toml`** at `/etc/marg/marg.toml` with your real provider keys and Postgres / Redis URLs. Start from `marg.toml.example`.
6. **Start the unit and watch the journal**:
   ```bash
   sudo systemctl enable --now marg
   journalctl -u marg -f
   ```
   The first three lines should include `RLIMIT_NOFILE check passed` (info level). If you see `RLIMIT_NOFILE soft limit is below the recommended production floor` (warn level), step 4 was skipped.
7. **Point your apps at the Marg endpoint.** Use the OpenAI SDK in any language with `base_url` pointed at Marg and `Authorization: Bearer <marg-key>`. No SDK change beyond that.

Done. The single-instance number was validated on exactly this shape minus the production network hops; the only operator-side variable is step 2.

## What "out of the box performance" means

The headline single-instance number (~6,000 req/s on a 16-core box) measures **Marg's own capacity to route, budget-check, and log chats per second**. It is not the end-to-end throughput your app will see, because that depends on how long the upstream LLM takes to answer.

- Marg adds about 1 ms of its own work per request, regardless of upstream speed.
- If the upstream LLM takes 800 ms to answer, each client connection is tied up for ~810 ms.
- To hit Marg's 6,000 req/s ceiling end-to-end, you need enough concurrent client connections to absorb that wait. With 800 ms upstream, that is roughly 4,800 concurrent connections per Marg node.
- Most real apps do not have that much concurrency in front of one node and will see Marg's overhead as effectively zero on top of the upstream latency they were already paying.

The cluster scaling story: one node ≈ 6,000 req/s. Three nodes behind a load balancer ≈ 18,000 req/s. Ten nodes ≈ 60,000 req/s. Marg is stateless, so the multiplier is linear; the cluster-3 and cluster-10 acceptance runs land in P10 to confirm.

## First-run smoke test

```bash
marg start --config marg.toml.example
```

In a second terminal:

```bash
curl -s http://127.0.0.1:8080/health
curl -s http://127.0.0.1:8080/version
curl -s http://127.0.0.1:8081/                # admin console (login page)
```

The bootstrap admin token is written to `./marg-admin.token` (mode
0600) the first time `marg start` runs. Use it to log into the
console at `http://127.0.0.1:8081/`.

## Where to next

- Configuration reference: `config-reference.md`
- Routing policy reference: `routing-policy.md`
- Cluster deployment: `cluster-deployment.md`
- Operations: `troubleshooting.md`, `faq.md`
