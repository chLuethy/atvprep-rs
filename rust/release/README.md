# Linux/musl releases

Run from a clean, tested checkout. Rust, Node and Alpine base images are pinned by digest;
Cargo and npm use the committed lockfiles. Cargo remaps the source path and uses a fixed
`SOURCE_DATE_EPOCH`. Alpine's package repositories supply the external media tools; their
package versions must be recorded with the resulting image for a particular release.

```bash
./rust/release/build.sh /private/tmp/atvprep-release
```

This exports `linux-x86_64/atvprep` and `linux-aarch64/atvprep` without writing build output
into the checkout. Docker Buildx and execution/emulation support for both architectures are
required. Every artifact must pass the signed installer roundtrip on its target architecture
before publication. Signing uses an independently provisioned Ed25519 key; no signing key is
embedded in the build or container.

```bash
docker buildx build --platform linux/arm64 --file rust/release/Dockerfile --target runtime --load --tag atvprep-rust:test .
./rust/release/container_smoke.sh atvprep-rust:test /private/tmp/atvprep-container-smoke
```

The image contains the Rust multicall binary, the unchanged SPA, ffmpeg/ffprobe and Bento4.
Python and uv are absent. Run it as the owner of private `/config` and `/state` mounts, and
mount the library at `/media`. Adapt `config.toml`, supply a private master-token file, and
configure the actual HTTPS origin. Keep existing services and databases separate.
The default command runs `manager api` with integrated worker threads; the image healthcheck
uses `/api/health/ready`. An intentionally closed deployment gate makes readiness fail.

The smoke uses a temporary configuration/database, checks health and all executable aliases,
checks that Python/uv are absent, records the image ID and logs, removes its container, and
deletes its temporary master credential. It does not change any existing container or service.

A production release still needs the platform evidence and operator decision described in
[the deployment guide](../../DEPLOYMENT.md).

The Gitea tag workflow builds unsigned Linux binaries as workflow artifacts. It requires a
runner with Docker Buildx and both architecture execution support; it does not sign, publish,
or deploy a release. Complete the platform checks and signing separately before distribution.
