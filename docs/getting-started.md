# Getting started

1. Install Provider Router for your operating system and architecture. Open its settings from the application or tray menu.
2. Settings automatically detects Codex CLI on this device and in available Windows WSL distributions. Select a detected provider and choose **Use this provider**, then configure its models. If login is needed, run `codex login` in that environment and refresh settings. Detection does not enable providers or expose models. Use **Add provider** for a custom connection or another provider. For **Codex**, install and sign in to Codex yourself first. Inference calls the subscription API directly; Codex CLI is used only to discover installations and check login status. Existing CLI connections migrate automatically while keeping their IDs, model selections and enabled state. Provider Router reuses that login and never installs or updates Codex. On Windows, choose **Windows** for a native login or **WSL** and a distribution for its existing Linux CLI login. An optional Codex home selects a separate existing account profile. For **OpenRouter**, enter your API key; it is encrypted by the operating system credential vault and is not returned to client applications.
3. Models load automatically when you open a configured provider. Choose models and their allowed thinking efforts, save the selection, and enable the provider. Use Refresh to retry or update the list. Detecting models does not automatically expose them.
4. Open **API access**. New installations listen on `127.0.0.1:47831` without requiring client keys. Change the port if needed. To restrict access, enable **Require client API keys** and create keys in the **API keys** section on that page. Copy each key when shown; only its hash is persisted. Existing installations preserve their previous authentication requirement.
5. Configure your application with the API base URL shown in settings, followed by `/v1`, and, when required, its client key. Call `/v1/models` to discover qualified model identifiers. No custom connection field is needed in standard OpenAI SDK requests.

Closing the settings window keeps the service available. Use **Quit** to stop Provider Router and cancel its active requests. Client applications do not own the service lifecycle. A revoked key immediately loses access and its active inferences are cancelled.

Provider Router is available independently of any particular client application. A client may find an existing installation through its local discovery descriptor, but that descriptor contains no access key and does not authorize the client. When authentication is enabled, create and supply a separate client key explicitly.

Open **Documentation** inside Router for endpoints, examples and troubleshooting.

The API is available only on this computer through a loopback address. Remote listeners, TLS configuration and reverse-proxy setup are not supported.

Direct Codex access supports file and native keyring credentials. Encrypted secrets storage and WSL keyring credentials are unsupported; use an existing supported file credential profile for these connections. If a refresh outcome is uncertain, Provider Router stops rather than replaying a possibly consumed refresh grant; repair or renew the login using Codex. It does not start a separate Provider Router login flow.

## Connect a hosted website

1. Open API access and add the website's exact HTTPS origin under Approved website origins, for example `https://app.example`, then save. Do not include a path or trailing slash. Browser access is disabled when the list is empty.
2. In the website integration settings, enter the Router endpoint shown in API access. The default is `http://127.0.0.1:47831`. Add a Router client key only if Require client API keys is enabled.
3. Choose the website's connectivity test and allow local-network access if the browser requests it. A successful connection check confirms Router access and exposed models; it does not spend provider inference usage or verify the upstream provider account.
4. If connection fails, check Router is running, the origin matches exactly, the endpoint port is correct, the client key is current when required, and browser local-network permission is enabled.

Only approve websites you trust to make inference requests through your accounts. Provider credentials remain in Router. Removing an approved origin blocks future browser requests; already accepted requests may finish.
