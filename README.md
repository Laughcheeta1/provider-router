# Provider Router

Provider Router is an MIT-licensed native terminal application that exposes your configured inference providers through an OpenAI-compatible API. It runs locally and uses the Codex subscriptions and OpenRouter keys you choose to configure.

- Discover native Codex CLI installations and Windows WSL logins, then use an existing Codex subscription directly over HTTP.
- Connect OpenRouter with your own API key and choose which providers, models, and reasoning efforts are available to other applications.
- Send PDF, DOCX, text, Markdown, CSV, and XLSX attachments. The router extracts each document locally and adds it to the model request.
- Issue revocable client keys, approve exact browser origins, and check connectivity without consuming inference usage.
- Choose **Run on startup**, **Keep running after open**, or **Run only while open**. Foreground operation is also available with `provider-router serve`.

Running `provider-router` opens the dashboard. Subcommands such as `provider-router status`, `provider-router providers list`, and `provider-router clients create` use the same settings and emit JSON for scripting. The Rust executable targets Windows, macOS, and Linux on x64 and ARM64.

See [Getting started](docs/getting-started.md), the [API contract](docs/api.md), and [development and verification](docs/development.md).
