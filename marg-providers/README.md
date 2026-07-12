# marg-providers

Provider adapter trait and per-provider clients for [Marg](https://github.com/SarthiAI/Marg), the self-hosted AI gateway.

This crate holds the upstream adapters Marg uses to talk to model providers. Marg speaks the OpenAI Chat Completions API on both sides, with adapters for Anthropic, Google, and Bedrock behind a single provider trait, so routing and failover work uniformly across providers.

This crate is a building block of Marg, published for reuse as a library. To run the gateway itself, see the [main repository](https://github.com/SarthiAI/Marg) (one-line installer or Docker image).

## License

Elastic License 2.0. See [LICENSE](https://github.com/SarthiAI/Marg/blob/main/LICENSE).
