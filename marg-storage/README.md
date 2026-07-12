# marg-storage

Storage trait and backend implementations for [Marg](https://github.com/SarthiAI/Marg), the self-hosted AI gateway.

This crate defines the pluggable storage backend used by the Marg server: keys, budgets, request logs, and route configuration. It ships three backends behind one trait:

- SQLite (default, zero-config, single file)
- Postgres (recommended for production)
- Redis (hot store for rate-limit counters and budgets)

This crate is a building block of Marg, published for reuse as a library. To run the gateway itself, see the [main repository](https://github.com/SarthiAI/Marg) (one-line installer or Docker image).

## License

Elastic License 2.0. See [LICENSE](https://github.com/SarthiAI/Marg/blob/main/LICENSE).
