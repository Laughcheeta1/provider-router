# Development and verification

Use Node 24 or later and pnpm. This repository is independently installable and has its own workspace and lockfile.

```sh
pnpm install --ignore-scripts
pnpm run typecheck
pnpm test
pnpm run build
pnpm start
```

Dependency lifecycle scripts are disabled by default. The explicit `setup:runtime` command provisions the pinned Electron runtime through Electron's installer; `start` and `dev` run it before launch. Package security settings stay enabled. Codex installation and updates remain separate and user-managed.

```sh
pnpm run dist:win
pnpm run dist:mac
```

Windows packages target x64 and ARM64 NSIS installers; macOS packages target Intel and Apple Silicon DMG and ZIP artifacts. Build macOS packages on macOS. Linux distributions are excluded. Packages are unsigned. GitHub release metadata powers the installed Windows application's explicit download/restart update controls; unsigned macOS releases open the release downloads for manual installation. Automatic release publication and signing are separately managed. A development source checkout does not check for updates.

`pnpm run build` produces `dist/main.cjs`, the isolated `dist/preload.cjs`, the settings renderer, and a headless rehearsal service. Production settings use Electron's operating-system-backed `safeStorage`; saving provider secrets fails when encryption is unavailable. Client keys are hashed, provider keys are encrypted, and discovery contains no secrets. Existing Codex credentials stay in Codex's own configured store.

For a disposable WSL/Linux development rehearsal of the API with the existing Codex CLI login:

```sh
PROVIDER_ROUTER_DATA_DIR=/tmp/provider-router-rehearsal PROVIDER_ROUTER_PORT=47841 node dist/smoke-server.cjs
```

The required absolute data directory receives an isolated settings file and `ready.json` with a temporary client key, connection ID and endpoint. Keep it private and remove it after stopping the process. The rehearsal service rejects provider-secret persistence; it references the existing Codex home and never copies its credential contents. The rehearsal uses the direct subscription HTTP adapter. `SIGTERM` and `SIGINT` cancel active inference and stop the server. The ordinary Electron application also accepts `PROVIDER_ROUTER_DATA_DIR` only in unpackaged development for isolated UI rehearsals.

The deterministic test suite runs the real HTTP server and the official OpenAI SDK against synthetic providers. It covers authentication and revocation, cross-client cancellation ownership, caller disconnection, schema/tool validation, provider wire mappings, credential rotation and guarded storage, process isolation, model discovery, standard Chat Completions/Responses and SSE framing. It does not spend real provider tokens. An authorized live Codex test should use synthetic data and the explicitly selected model; native Windows and macOS installer/runtime checks must be reported separately. A Windows installation can select a native Codex login or a Codex CLI installed in a chosen WSL distribution; this is a Windows feature, not a Linux distribution target.

Codex inference calls the subscription Responses endpoint directly, with native function calls, structured output schemas and caller-supplied history. It does not launch Codex CLI, load coding-agent instructions or wrap answers in a CLI decision envelope. Discovery can still probe Codex installation and login status. Existing CLI connections are persisted as direct connections on upgrade with their identities and selections preserved.

Direct OAuth refresh is internal credential maintenance. Provider Router serializes its own refreshes, reloads before exchanging credentials, preserves account/login identity and persisted OAuth authority, commits through Codex's stable OS lock sidecar and compares the original credential generation. It does not hold the commit lock through network exchange or claim to serialize every refresh by another process. An uncertain rotation is marked by a generation hash and stops automatic replay until the Codex login changes. File and direct native-keyring storage are supported; Codex's separate encrypted `secrets` backend and WSL keyring stores are unsupported. WSL file credentials are accessed in place and refresh commits acquire Linux flock in the owning distribution.
