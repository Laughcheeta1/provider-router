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

Windows packages target x64 and ARM64 NSIS installers. macOS packages target Intel and Apple Silicon DMG and ZIP artifacts. Build macOS packages on macOS. Linux distributions are excluded. Packages are unsigned. GitHub release metadata powers the installed Windows application's explicit download and restart controls. On macOS, the app opens the release downloads for manual installation. A development source checkout does not check for updates.

## Verify document attachments

`pnpm test tests/documents.test.ts` checks extraction from a PDF with a rotated scanned page, DOCX text and embedded images, XLSX cell coordinates, cached values and formulas, and CSV quoting. It also checks the attachment limits and support for image inputs. The SDK tests cover inline attachments on Chat Completions and Responses and verify the extracted messages sent to simulated upstream providers.

To include public documents, set `PROVIDER_ROUTER_PUBLIC_FIXTURES` to a directory with the following files and run `PROVIDER_ROUTER_PUBLIC_FIXTURES=/path/to/fixtures pnpm test tests/documents.test.ts`. Page numbers are one-based positions in the original PDF.

| Fixture | Source and pages |
| --- | --- |
| `isa-excerpt.pdf` | [ISA integrated report 2024](https://isa.co/wp-content/uploads/2026/02/reporte-integrado-isa-2024.pdf), pages 1, 4, 6 and 8 |
| `xm.pdf` | [XM's RIO guide for entering DDV offers](https://www.xm.com.co/sites/default/files/documents/Instructivo%20RIO-Ingreso%20de%20oferta%20DDV.pdf), all 14 pages |
| `epm-excerpt.pdf` | [EPM consolidated financial statements for 2025](https://www.epm.com.co/content/dam/epm/inversionistas/estados-financieros/estados-financieros-grupoepm/2025/Grupo_EPM_EEFF_Consolidados_certificaci%C3%B3n_informe_del_auditor_y_Notas_al_31-12-2025.pdf), pages 2, 3, 8 and 21 |
| `ecopetrol-excerpt.pdf` | [Ecopetrol third-quarter 2026 report](https://files.ecopetrol.com.co/web/esp/inversionista/reporte-3t2026/reporte-3t-2026.pdf), pages 2, 3, 8 and 13 |
| `xm-payment-template.xlsx` | [XM's payment deferral request form](https://www.xm.com.co/sites/default/files/inline-files/Solicitud%20aplicaci%C3%B3n%20Art%C3%ADculo%206%C2%B0%20de%20la%20Resoluci%C3%B3n%20CREG%20101%20029%20de%202022.xlsx), one sheet |

In the Attalid monorepo, `backends/antikythera-back/tests/integration/test_provider_router_attachments.py` exercises the Chat upload route, the agent model pipeline, both Router adapters with simulated upstream responses, and rejection of oversized files before chat admission.

For live verification, connect a development Antikythera session to an exposed vision-capable model, attach synthetic or public files, and ask for a fact found only in each file. For example, ask for the identifier on the rotated scanned page of `attachment-layout.pdf` and the value at `Dispatch!D4` in `attachment-workbook.xlsx`. The expected answers are `SCAN-ONLY-583` and `250`. Stop the rehearsal process and remove its temporary settings when finished.

## Publish a release

GitHub Actions runs type checks, tests, and a build on pull requests targeting `main` and pushes to `main`. To build downloadable Windows and macOS installers for testing without publishing a release, run **Provider Router CI and release** manually from the Actions tab and download the run's artifacts.

To ship a stable version, update `version` in `package.json`, merge the change, and tag the exact commit you want to ship. For example, if that commit sets the version to `0.1.1`:

```sh
git tag -a v0.1.1 -m "Provider Router 0.1.1"
git push origin v0.1.1
```

A pushed tag must match the stable version in `package.json`. After the checks and both platform builds pass, the workflow publishes a GitHub Release with the Windows installers, `latest.yml`, matching `.exe.blockmap` files, and macOS DMG and ZIP files. The Windows updater uses the manifest to locate and verify the installer and uses the block maps for differential downloads when available.

`pnpm run build` produces `dist/main.cjs`, the isolated `dist/preload.cjs`, the settings renderer, and a headless rehearsal service. Production settings use Electron's operating-system-backed `safeStorage`; saving provider secrets fails when encryption is unavailable. Client keys are hashed, provider keys are encrypted, and discovery contains no secrets. Existing Codex credentials stay in Codex's own configured store.

For a disposable WSL/Linux development rehearsal of the API with the existing Codex CLI login:

```sh
PROVIDER_ROUTER_DATA_DIR=/tmp/provider-router-rehearsal PROVIDER_ROUTER_PORT=47841 node dist/smoke-server.cjs
```

The required absolute data directory receives an isolated settings file and `ready.json` with a temporary client key, connection ID and endpoint. Keep it private and remove it after stopping the process. The rehearsal service rejects provider-secret persistence; it references the existing Codex home and never copies its credential contents. The rehearsal uses the direct subscription HTTP adapter. `SIGTERM` and `SIGINT` cancel active inference and stop the server. The ordinary Electron application also accepts `PROVIDER_ROUTER_DATA_DIR` only in unpackaged development for isolated UI rehearsals.

The deterministic test suite runs the real HTTP server and the official OpenAI SDK against synthetic providers. It covers authentication and revocation, cross-client cancellation ownership, caller disconnection, schema/tool validation, provider wire mappings, credential rotation and guarded storage, process isolation, model discovery, standard Chat Completions/Responses and SSE framing. It does not spend real provider tokens. An authorized live Codex test should use synthetic data and the explicitly selected model; native Windows and macOS installer/runtime checks must be reported separately. A Windows installation can select a native Codex login or a Codex CLI installed in a chosen WSL distribution; this is a Windows feature, not a Linux distribution target.

Codex inference calls the subscription Responses endpoint directly, with native function calls, structured output schemas and caller-supplied history. It does not launch Codex CLI, load coding-agent instructions or wrap answers in a CLI decision envelope. Discovery can still probe Codex installation and login status. Existing CLI connections are persisted as direct connections on upgrade with their identities and selections preserved.

Direct OAuth refresh is internal credential maintenance. Provider Router serializes its own refreshes, reloads before exchanging credentials, preserves account/login identity and persisted OAuth authority, commits through Codex's stable OS lock sidecar and compares the original credential generation. It does not hold the commit lock through network exchange or claim to serialize every refresh by another process. An uncertain rotation is marked by a generation hash and stops automatic replay until the Codex login changes. File and direct native-keyring storage are supported; Codex's separate encrypted `secrets` backend and WSL keyring stores are unsupported. WSL file credentials are accessed in place and refresh commits acquire Linux flock in the owning distribution.
