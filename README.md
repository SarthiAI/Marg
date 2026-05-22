# Marg

Self-hosted AI gateway written in Rust. Applications point their LLM client at Marg instead of directly at OpenAI, Anthropic, Google, or Bedrock. Marg enforces budgets, routes between providers, gives one observability surface, and (in v2.0 with Kavach enabled) becomes a default-deny, cryptographically auditable governance gateway.

This is the v0.1 scaffold. The build is being assembled phase by phase. See `../build-state/INDEX.md` for the full roadmap.

## What works today (P00)

- A single static binary called `marg`.
- `marg start` boots an axum HTTP server on the configured bind address (default `0.0.0.0:8080`) and serves:
  - `GET /health` liveness check
  - `GET /ready` readiness check
  - `GET /version` build metadata
- `marg version` prints the version. `marg version --verbose` prints the full version JSON.
- TOML config loading from `./marg.toml` by default, override with `--config <path>`. Missing config file is fine, defaults are used.
- Graceful shutdown on SIGTERM / SIGINT.

Provider proxying, budgets, multi-provider routing, observability, admin API, and console UI all land in P01 through P06. Kavach governance lands in P08 and P09. Roadmap in `../build-state/INDEX.md`.

## Build

```bash
cd marg
cargo build --release
```

The release binary lands at `./target/release/marg`.

## Run

```bash
./target/release/marg start --config ./marg.toml
```

In another terminal:

```bash
curl http://localhost:8080/health
curl http://localhost:8080/version
```

Stop with `Ctrl-C` or `kill -TERM <pid>`.

## Configuration

See `marg.toml.example` for the documented config shape. Copy it to `marg.toml` and edit. P00 only honors the `[server]` block; later phases fill in the rest.

## Workspace layout

```
marg/
├── Cargo.toml                    workspace root
├── marg-cli/                     binary entry point (the `marg` command)
├── marg-core/                    core types, config loader, error definitions
├── marg-server/                  axum server, routes, graceful shutdown
├── marg-storage/                 storage trait + backends (sqlite, postgres, redis) - P01, P03
├── marg-providers/               provider adapter trait + clients - P01, P02
└── bench/
    ├── provider-stub/            deterministic fake provider for benchmarks - P01
    ├── data/                     synthetic prompt corpus and key fixtures
    ├── scenarios/                benchmark scenario scripts
    ├── rigs/                     hardware tier configs and run scripts
    └── results/                  benchmark results checked in per release
```

## License

[Elastic License 2.0](LICENSE).

## Documentation

The full project documentation lives in the parent folder under `../build-state/`. Start at `../CLAUDE.md` for the project overview and `../build-state/INDEX.md` for the phase roadmap.
