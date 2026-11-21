# Getting started

## Install Provider Router

Download the installer from your version's GitHub Release. Choose `x86_64` for an Intel or AMD computer and `aarch64` for an ARM64 computer, including an Apple silicon Mac.

| System | Installer | Open Provider Router |
| --- | --- | --- |
| Windows | Run `provider-router-windows-<arch>.exe`. | Choose **Provider Router** from the Start menu. |
| macOS | Open `provider-router-macos-<arch>.pkg`. | Open **Provider Router.command** in the Applications folder inside your home directory. |
| Linux | From the download directory, run `bash provider-router-linux-<arch>.sh`. | Run `~/.local/bin/provider-router` in your terminal. |

Run a newer installer to upgrade. It preserves your profile and startup choice. If the router is running, the installer stops it before replacing the executable and starts it again afterward. Finish active requests before installing an upgrade.

## Set up a provider

1. Open Provider Router using the launcher or command above. Use the arrow keys and Enter or press the letter shown next to an action.
2. In **Providers**, discover an existing Codex login or add a provider manually. Install and sign in to Codex with `codex login` first. On Windows, choose the native installation or a named WSL distribution. A custom Codex home selects another existing profile. To add OpenRouter, paste your API key into the masked prompt.
3. Open the provider, choose which models and reasoning efforts to expose, save the selection, and enable the provider. The model catalog comes from the provider.
4. In **API access**, review the listener address, key requirement, and approved website origins. A new profile listens on `127.0.0.1:47831`. Use **Client keys** to issue a token if you enabled key authentication. Copy the token when it is displayed and share it only with its intended application.
5. Set your application's OpenAI API base URL to the listener address with `/v1` appended. OpenAI SDK clients can list models with `GET /v1/models` and use the returned model ID directly.

The account's profile is stored in the OS configuration directory under `provider-router-rust`. Use `--data-dir DIRECTORY` for a separate account-private profile. Linux uses a private local encryption key for the OpenRouter vault so the router can open it on a headless server. macOS uses Keychain and Windows uses Credential Manager. Router client keys are stored as hashes.

## Set the lifecycle

New profiles default to **Run on startup**. Opening the dashboard registers the executable with your account's startup service. Keep the executable at its installed path. For a command-line setup, `provider-router startup startup` performs the same registration.

The **Startup behavior** setting and its corresponding commands control when the router runs.

| Terminal setting | Command | When it runs |
| --- | --- | --- |
| Run on startup | `provider-router startup startup` | When your account's startup service runs. To start before sign-in on Linux, enable systemd user lingering for that account. |
| Keep running after open | `provider-router startup keep-after-open` | Starting when you open the terminal app, until explicitly stopped. |
| Run only while open | `provider-router startup while-open` | Until the last interactive terminal session closes. |

Use `provider-router serve` to start the listener in the foreground, `provider-router status` for its endpoint, or `provider-router stop` to stop the listener. Run `provider-router --help` for all scriptable commands.

GitHub installers use manual upgrades. The `provider-router status` command shows the worker and supervisor versions while the router is running.

## Connect a hosted website

Add the website's exact origin, such as `https://app.example`, in API access. In the application, enter the loopback endpoint and its Router client key if key authentication is enabled. The application's connectivity test can check Router access and exposed models without spending provider inference usage. If access fails, check the origin, port, client key, and browser local-network permission.

Direct Codex access supports file and native keyring credentials. Use file credentials for WSL. If the router reports an uncertain credential refresh, renew the login in Codex before trying again.
