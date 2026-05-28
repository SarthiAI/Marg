# syntax=docker/dockerfile:1.6
#
# Marg container image. Built from the per-arch release binary produced by
# .github/workflows/release.yml. Expects the binary at ./marg.<arch> in the
# build context where <arch> matches TARGETARCH (amd64 or arm64). The
# release workflow places these files before invoking buildx.
#
# The image is FROM scratch so the size sits around 16 MB and the image
# ships no shell, no package manager, and no leftover OS surface. First
# boot semantics: `marg start` auto-runs `marg init --auto` when
# /etc/marg/marg.toml is missing, so a clean container (or an empty volume
# mounted at /etc/marg) becomes a working install on the first start.
# Persist /etc/marg via a named volume to keep the SQLite db, audit log,
# signing keypair, and admin token across container restarts.

FROM scratch

ARG TARGETARCH
COPY marg.${TARGETARCH} /marg

EXPOSE 8080 8081

ENTRYPOINT ["/marg", "start", "--config", "/etc/marg/marg.toml"]
