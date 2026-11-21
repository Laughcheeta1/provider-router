# Development and verification

Use Rust 1.92 or newer. The project includes a Cargo lockfile and builds one executable. Its local verification uses isolated profiles and a loopback listener.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
cargo run --locked
```

The CI matrix builds and tests on Linux, macOS, and Windows on x64 and ARM64. On Linux, the terminal dashboard can be exercised in WSL and the startup service on an active systemd user manager.

## Build manual installers

Run the **Provider Router CI** workflow manually to build the six installer candidates and download them from the run's artifacts. For a versioned release, run it from the `v<version>` tag matching `Cargo.toml`. Extract the installer from each artifact ZIP before attaching it to the matching GitHub Release. Release assets use the `provider-router-<platform>-<arch>.<extension>` names listed in [Getting started](getting-started.md).

Installer payloads use the `manual-installers` Cargo feature. Build a local release payload with `cargo build --locked --release --features manual-installers`, then run the packaging command on its target platform.

| System | Packaging command |
| --- | --- |
| Windows | `pwsh -File installers/windows/build.ps1 -Binary target/release/provider-router.exe -Output release/provider-router-windows-x86_64.exe` |
| macOS | `bash installers/unix/build-macos.sh target/release/provider-router release/provider-router-macos-aarch64.pkg` |
| Linux | `bash installers/unix/build-linux.sh target/release/provider-router release/provider-router-linux-x86_64.sh` |

Use the output filename for the architecture of the payload you built. The Windows command uses Inno Setup. CI artifacts are available for testing. Sign the Windows executable and installer, and sign and notarize the macOS package before publishing user downloads.

For Windows release builds, sign the executable before packaging and pass the Inno Setup signing command through `-SignCommand`. For macOS release builds, set `MACOS_APPLICATION_SIGNING_IDENTITY`, `MACOS_INSTALLER_SIGNING_IDENTITY`, and `MACOS_NOTARY_PROFILE` to the identities and notarization profile configured on the build machine.

## Use an isolated profile

```sh
profile="$(mktemp -d)"
cargo run --locked -- --data-dir "$profile" network set --port 47841
cargo run --locked -- --data-dir "$profile" serve
```

In another terminal, run `provider-router --data-dir DIRECTORY status` with the same directory. To shut down, run `provider-router --data-dir DIRECTORY stop` or press Ctrl+C in the foreground process. Remove the temporary profile once it has stopped.

## Verify providers and attachments

The automated tests use synthetic inputs and synthetic provider responses. The API tests exercise standard Chat Completions and Responses, request validation, local authentication, browser origins, documents, and streaming response formats. For an authorized live Codex run, use a dedicated Router profile, an existing Codex CLI login, one exposed model from the catalog, and a synthetic prompt.

The fixture folder contains a PDF with a rotated scanned page, a workbook with cell addresses and formulas, and a Word document with embedded images. The expected scan identifier is `SCAN-ONLY-583` and the expected value at `Dispatch!D4` is `250`. These fixtures can also be used for an authorized live vision smoke test. To validate extraction without calling a provider, send the intended request to `POST /v1/attachment-checks`.

For local startup verification on Linux, use an isolated profile and the generated systemd user unit. Verify start, stop, and cleanup. Starting at system boot before the account signs in also requires user lingering.

## Build and verify automatic updates for feed-enabled builds

Set `PROVIDER_ROUTER_UPDATE_FEED` and `PROVIDER_ROUTER_UPDATE_PUBLIC_KEY` when building the executable for both installers and subsequent releases. The feed is the HTTPS URL of the signed manifest for that OS and architecture. The key is a 32-byte Ed25519 public key encoded as 64 hex characters. Keep the private signing key separate from the build and hosting infrastructure. A local test profile can point to a loopback HTTP feed through `updates.toml` in its data directory.

```toml
feed = "http://127.0.0.1:47905/linux-x86_64/manifest.json"
public_key = "<64 hex characters>"
```

Build the executable with the release version in `Cargo.toml`. Publish that native executable beside its manifest. The signed payload is a UTF-8 JSON document with these fields.

```json
{
  "format": 1,
  "expires": "2026-12-01T00:00:00Z",
  "version": "0.1.1",
  "os": "linux",
  "arch": "x86_64",
  "artifact": "provider-router-0.1.1",
  "size": 20000000,
  "sha256": "<lowercase SHA-256 of the executable>"
}
```

Sign the exact payload bytes with Ed25519, for example with `openssl pkeyutl -sign -rawin -inkey release-key.pem -in payload.json -out signature.bin`. Encode the payload and raw signature separately as standard base64 for the manifest envelope. Upload the executable before publishing the manifest. Each platform uses the Rust `std::env::consts` OS and architecture names, such as `windows` and `aarch64`.

```json
{
  "signed": "<base64 of payload.json>",
  "signature": "<base64 of signature.bin>"
}
```

The service checks at startup and every 15 minutes. It verifies metadata and executable contents, starts a private API worker, and switches at the HTTP request boundary. It then starts the replacement supervisor and passes it the existing listener. The account startup entry points to the stable installed executable, which starts the service and dispatches new dashboard and CLI invocations to the active release. Use `provider-router status` to inspect the running worker and supervisor versions.

For a live update smoke test, start with an isolated profile, hold an API request open, publish a signed newer release, and verify the same API port serves that version while the old request completes. A corrupted artifact or a candidate that fails readiness should leave a working worker available. Rollback retains the highest accepted release number, so publish a higher version for its replacement.

## Native architecture

- [Core library](../native/lib.rs) compiles the [domain rules](../native/domain/mod.rs) and [application use cases](../native/application/mod.rs) separately from the executable. Use cases define the contracts for persistence, provider inference, and document preparation.
- [Presentation](../native/presentation/mod.rs) owns the CLI, terminal screens, and local HTTP protocol translation. It calls shared settings and inference workflows through typed interfaces.
- [Infrastructure](../native/infrastructure/mod.rs) implements profile storage, platform startup, provider clients, document processing, and supervision. The supervisor keeps socket transfer, the control channel, and signed release verification in focused modules.
- [The executable entry point](../native/main.rs) supplies the concrete adapters and dispatches the private worker processes.
