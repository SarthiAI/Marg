# syntax=docker/dockerfile:1.6
#
# Marg container image. Built from the per-arch release binary produced
# by .github/workflows/release.yml. Expects the binary at
# docker/bin/marg-<arch> in the build context (<arch> matches
# TARGETARCH: amd64 or arm64). No compiler runs inside the image; the
# binary inside the image is byte-identical to the one shipped in the
# release archive for the same version.
#
# Base is debian:bookworm-slim plus ca-certificates, tini, and curl.
# Image is around 30 MB. ca-certificates lets Marg open TLS to upstream
# LLM providers without ca-bundle gymnastics; tini handles PID 1 /
# zombie reaping cleanly; curl is included for the in-container
# HEALTHCHECK probe operators can wire up.
#
# First-boot semantics: `marg start` auto-runs `marg init --auto` when
# /etc/marg/marg.toml is missing, so a clean container (or an empty
# named volume mounted at /etc/marg) becomes a working install on the
# first start. Persist /etc/marg via a named volume to keep the SQLite
# db, signed audit chain, signing keypair, and admin token across
# container restarts.

FROM debian:bookworm-slim AS runtime

ARG TARGETARCH

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
        curl \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

# Non-root marg system user. uid/gid match the system user the
# curl-pipe-sh installer creates on bare metal, so file permissions
# stay consistent across the two install paths.
RUN groupadd --system --gid 1001 marg \
    && useradd --system --uid 1001 --gid marg --no-create-home --shell /usr/sbin/nologin marg

RUN mkdir -p /etc/marg \
    && chown marg:marg /etc/marg

COPY docker/bin/marg-${TARGETARCH} /usr/local/bin/marg
RUN chmod +x /usr/local/bin/marg

USER marg
WORKDIR /etc/marg

EXPOSE 8080 8081
VOLUME ["/etc/marg"]

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/marg", "start", "--config", "/etc/marg/marg.toml"]
