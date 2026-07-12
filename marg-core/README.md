# marg-core

Core types, configuration loader, and error definitions for [Marg](https://github.com/SarthiAI/Marg), the self-hosted AI gateway.

This crate holds the shared building blocks the rest of the Marg workspace depends on: the config model (`marg.toml` and the separate Kavach policy file), the common error types, and the core request/decision types. It has no server or storage logic of its own.

This crate is a building block of Marg, published for reuse as a library. To run the gateway itself, see the [main repository](https://github.com/SarthiAI/Marg) (one-line installer or Docker image).

## License

Elastic License 2.0. See [LICENSE](https://github.com/SarthiAI/Marg/blob/main/LICENSE).
