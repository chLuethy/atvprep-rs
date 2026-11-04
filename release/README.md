# Release artifacts

Releases are built twice and must agree bit for bit: once locally, where they are signed, and once
in the Gitea Actions run, which publishes only if its own build matches the signed manifest. No
binary is committed.

Files in this directory:

- `release.json`: the signed-installer manifest, with the workspace version and exactly one entry for each platform. Each entry has `platform`, `url` (HTTPS), `sha256` and `size`.
- `release.json.sig`: OpenSSH Ed25519 signature of the exact manifest bytes using namespace `atvprep-release`.
- `release.pub`: the public key the signature belongs to. It proves nothing by itself; the independently distributed key is the `RELEASE_TRUSTED_KEY` secret (see ADR 0003).
- `atvprep-linux-x86_64` and `atvprep-linux-aarch64` are never committed (git-ignored). The workflow writes its own build there to verify it.

## Releasing

1. Bump the workspace version in `rust/Cargo.toml`, run `./rust/check.sh` and the frontend checks, commit.
2. Build locally and write the manifest from the result:

   ```bash
   ./rust/release/build.sh /private/tmp/atvprep-release
   ./scripts/make-manifest.sh /private/tmp/atvprep-release
   ```

   The URLs must point at the release assets (`RELEASE_BASE_URL` overrides the default), be reachable over HTTPS without redirects for the installer's automatic download, or be installed with the installer's `--asset-file` option for a private repository.
3. Sign the manifest with the private key, which never leaves your machine:

   ```bash
   ssh-keygen -Y sign -f /path/to/private-key -n atvprep-release release/release.json
   ```

4. Commit `release/release.json`, `release/release.json.sig` and `release/release.pub`. These files are not an input of the build, so the binaries stay identical.
5. Set the Actions secret `RELEASE_TRUSTED_KEY` once to the independently distributed public key, then create and push the tag `v<workspace-version>`.

The tag workflow checks that the manifest, signature and key are committed and that the tag matches the version, builds both binaries from the tagged source in a pinned container, and runs `scripts/check-release.sh`: signature, hashes and sizes against the freshly built bytes, version, ELF architecture, command entry points and a native signed-install roundtrip. Only then it creates a draft release, uploads the manifest, signature, key and both binaries, checks the uploaded names and sizes through the API, and publishes. A failed upload leaves an unpublished draft for inspection. A manual dispatch does the same checks without publishing.

If the CI build differs from the signed hashes, the run fails and nothing is published: the build is not reproducible, or the source differs from what was signed. Never sign the CI output instead; find the difference.

Gitea's workflow token can upload private release attachments but cannot download a draft through the web attachment endpoint on this server. An operator must therefore compare the draft's bytes independently before resuming it with the `resume_verified_draft` manual input. That input rebuilds and re-verifies everything and checks the signed files against the version tag first.
