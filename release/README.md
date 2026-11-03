# Release artifacts

Before tagging a release, commit these files in this directory:

- `atvprep-linux-x86_64` and `atvprep-linux-aarch64`: executable, statically linked release binaries.
- `release.json`: the signed-installer manifest, with the workspace version and exactly one entry for each platform. Each entry has `platform`, `url` (HTTPS), `sha256` and `size`.
- `release.json.sig`: OpenSSH Ed25519 signature of the exact manifest bytes using namespace `atvprep-release`.

The manifest has this shape; replace the example values with the actual version, URLs, SHA-256 digests and byte counts:

```json
{"version":"0.5.5","assets":[
  {"platform":"linux-x86_64","url":"https://example.org/atvprep-linux-x86_64","sha256":"<64 hex characters>","size":123},
  {"platform":"linux-aarch64","url":"https://example.org/atvprep-linux-aarch64","sha256":"<64 hex characters>","size":123}
]}
```

Sign it locally with `ssh-keygen -Y sign -f /path/to/private-key -n atvprep-release release/release.json`. Obtain the digest and size from the final files, for example with `sha256sum` and `wc -c` on Linux.

Build and sign outside the release action. Set the Gitea Actions secret `RELEASE_TRUSTED_KEY` to the independently distributed OpenSSH Ed25519 public key; do not store the private key in the repository. The URLs in the manifest must point to the actual binary bytes and be directly accessible over HTTPS without redirects to use the installer's automatic download. For a private Gitea repository, download the release asset with appropriate credentials and use the installer's `--asset-file` option instead.

Commit and push the artifacts, then create and push a `v<workspace-version>` tag. A manual workflow dispatch checks the committed files without publishing. The tag workflow verifies signatures, hashes, versions, ELF architectures, command entry points and a native signed-install roundtrip, then creates a draft Gitea release, uploads and downloads each file for byte comparison, and publishes it. A failed upload leaves an unpublished draft for inspection. It never compiles release binaries.
