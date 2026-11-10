# Provider Router

Provider Router is an independent, MIT-licensed desktop application that exposes your configured inference providers through a common, OpenAI-compatible API. It runs locally, requires no Provider Router account, and can serve any application you authorize.

- Reuse the existing login in your installed Codex CLI, or select direct HTTP inference using existing Codex subscription credentials.
- Connect OpenRouter with your own API key.
- Choose which connections and models to expose, restrict allowed thinking efforts, and optionally issue separate revocable keys to client applications.
- Keep the API running in the background while the settings window is closed.
- Use loopback access on port 47831 without client keys by default, optionally require keys, or explicitly enable remote access through TLS or your own HTTPS reverse proxy.
- Install on Windows x64/ARM64 and macOS Intel/Apple Silicon. Linux installers are not distributed.

Provider Router does not execute the application tools declared in an inference request. It returns function calls for the calling application's harness to execute.

Open the built-in Documentation page for endpoint descriptions, configuration help and runnable examples.

See [Getting started](docs/getting-started.md), the [API contract](docs/api.md), and [development and verification](docs/development.md). The current release is unsigned; signing is maintained separately from application functionality.

Your provider's account eligibility, available models, usage limits and terms still apply. An open-source license describes this project's source distribution; it does not change upstream provider terms.
