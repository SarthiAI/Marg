# marg-server

The HTTP server library behind [Marg](https://github.com/SarthiAI/Marg), the self-hosted AI gateway.

This crate is the request pipeline: the OpenAI-compatible proxy endpoints, the admin API, budget and rate-limit enforcement, the async write batcher, Prometheus metrics, graceful shutdown, and the Kavach governance integration (default-deny gating, the signed post-quantum audit chain, and signed cross-node key invalidation for clustered deployments).

It is published for reuse as a library, so the Marg server can be embedded in another binary. To run the gateway itself, see the [main repository](https://github.com/SarthiAI/Marg) (one-line installer or Docker image).

## License

Elastic License 2.0. See [LICENSE](https://github.com/SarthiAI/Marg/blob/main/LICENSE).
