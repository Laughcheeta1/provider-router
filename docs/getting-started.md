# Getting started

1. Install Provider Router for your operating system and architecture. Open its settings from the application or tray menu.
2. Add a provider connection. For **Codex CLI**, install and sign in to Codex yourself first. Provider Router reuses that login and never installs or updates Codex. On Windows, choose **Windows** for a native login or **WSL** and a distribution for its existing Linux CLI login. An optional Codex home selects a separate existing account profile. For **direct subscription HTTP**, select the existing native Codex credential store. For **OpenRouter**, enter your API key; it is encrypted by the operating system credential vault and is not returned to client applications.
3. Refresh models, choose models and their allowed thinking efforts, save the selection, and enable the provider. Refreshing metadata does not automatically expose newly added models.
4. Open **API access**. New installations listen on `127.0.0.1:47831` without requiring client keys. Change the port if needed. To restrict access, enable **Require client API keys** and create keys under **API keys**. Copy each key when shown; only its hash is persisted. Existing installations preserve their previous authentication requirement.
5. Configure your application with the API base URL shown in settings, followed by `/v1`, and, when required, its client key. Call `/v1/models` to discover qualified model identifiers. No custom connection field is needed in standard OpenAI SDK requests.

Closing the settings window keeps the service available. Use **Quit** to stop Provider Router and cancel its active requests. Client applications do not own the service lifecycle. A revoked key immediately loses access and its active inferences are cancelled.

Provider Router is available independently of any particular client application. A client may find an existing installation through its local discovery descriptor, but that descriptor contains no access key and does not authorize the client. When authentication is enabled, create and supply a separate client key explicitly.

Open **Documentation** inside Router for endpoints, examples and troubleshooting.

For remote access, require client keys, enable it in API settings and provide either a TLS certificate/private-key pair for a direct listener, or your own HTTPS reverse proxy forwarding to a loopback listener with **Use a TLS reverse proxy** enabled. Provider Router does not provision a tunnel. Keep the same per-application keys and revocation controls. Browser-origin access is rejected; use a trusted application or backend client.

If direct Codex credentials use an unsupported encrypted storage backend, select Codex CLI mode. If a refresh outcome is uncertain, Provider Router stops rather than replaying a possibly consumed refresh grant; repair or renew the login using Codex. It does not start a separate Provider Router login flow.
