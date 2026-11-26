# API contract

The public inference API implements a documented subset of the OpenAI API. Set an OpenAI SDK's `baseURL` to `http://127.0.0.1:47831/v1` and its `apiKey` to the client key when authentication is enabled. New installations do not require keys; SDKs that require a nonempty key can use `local` as a placeholder while key requirements are disabled. Native application accounts and provider credentials are not API inputs. The API listens on loopback by default. Linux hosts can enable an additional [Docker bridge listener](getting-started.md#connect-docker-containers-on-the-same-linux-server) on the same port.

Local access defaults to no authentication. When **Require client API keys** is enabled, every endpoint except `/health` requires `Authorization: Bearer CLIENT_KEY`. Browser access is disabled by default. In API access, approve each exact HTTPS website origin (for example `https://app.example`) under Approved website origins; loopback HTTP origins are supported for development. Origins cannot contain paths, trailing slashes or wildcards. Approved origins receive CORS headers on successes and errors, and OPTIONS accepts GET/POST with authorization, content-type and x-request-id headers. Requests from other origins are rejected before authentication or inference. Local Host headers remain checked against the listener to prevent DNS rebinding. Browser access does not expose settings, provider credentials or account-management operations.

On Linux, `container_bridge_host` must be an RFC 1918 IPv4 or unique-local IPv6 address assigned to the Linux bridge device `docker0` or a Docker-named bridge `br-<12-hex network ID>`. The bridge listener accepts connections only from addresses inside a local Docker bridge network, so hosts that merely route to the bridge address, for example over a VPN, are refused. Setting it also admits the configured bridge IP and `host.docker.internal` as Host authorities with the configured port, for example `172.17.0.1:47831` and `host.docker.internal:47831`. Use brackets around an IPv6 address in URLs and Host headers. Both listeners use the same client-key policy and exact browser origin rules.

## GET /health

This public identity probe returns installation identity, protocol compatibility and the `authentication_required` flag:

```json
{"name":"provider-router","protocol_version":1,"instance_id":"019a1111-1111-7111-8111-111111111111","version":"0.1.0","authentication_required":false}
```

## GET /v1/connectivity

A lightweight connection check uses the same client-key policy as inference, with no provider request or subscription usage. It returns Router identity, authentication policy, counts of exposed providers/models, and `ready` indicating whether at least one model is exposed. It does not verify upstream account validity; provider errors can still occur during inference.

```json
{"name":"provider-router","protocol_version":1,"instance_id":"019a1111-1111-7111-8111-111111111111","version":"0.1.0","authentication_required":false,"ready":true,"providers":1,"models":2}
```

A browser integration should first ask the user to connect, explain how to approve its own exact origin in Router, and handle local-network permission denial and unavailable services. Browser CORS approval and client API keys are separate controls. Enabling CORS does not disable required-key authentication. Each browser controls access from hosted HTTPS pages to HTTP localhost; use loopback targets and test supported browsers. `GET /health` remains available without a key, but browser origins still need approval. CLI clients without an Origin header remain compatible.

## GET /v1/models

Model discovery uses the OpenAI list shape. Every ID qualifies a model with its exposed connection identity. This allows two subscriptions to expose the same upstream model independently.

```json
{"object":"list","data":[{"id":"019a1111-1111-7111-8111-111111111111/gpt-6-luna","object":"model","created":0,"owned_by":"openai"}]}
```

The `provider_router` extension on each model contains `connection_id`, `connection_name`, `provider`, `method`, upstream `model`, model `name`, allowed `reasoning_efforts`, `default_reasoning_effort`, `context_window`, `max_output_tokens`, `tools`, `structured_output`, and `input_modalities`. Only exposed models and allowed efforts are advertised. An omitted effort uses the configured default or first allowed effort.

An unqualified upstream model ID is accepted only when exactly one exposed connection offers it. Ambiguous IDs are rejected before inference.

## POST /v1/chat/completions

Supported request fields are `model`, `messages`, `tools`, `tool_choice`, `parallel_tool_calls`, `response_format`, `reasoning_effort`, `max_tokens` or `max_completion_tokens`, `stream`, and `stream_options.include_usage`. Messages retain system/developer/user/assistant/tool roles, assistant `tool_calls`, and `tool_call_id` associations. Content accepts text strings and text blocks. User messages also accept inline `file` blocks. Function tools use normal OpenAI JSON schemas. Structured output uses `response_format.type = "json_schema"`.

```json
{
  "model": "019a1111-1111-7111-8111-111111111111/gpt-6-luna",
  "messages": [{
    "role": "user",
    "content": [
      {"type": "text", "text": "What does the note say?"},
      {"type": "file", "file": {"filename": "note.txt", "file_data": "data:text/plain;base64,SGVsbG8h"}}
    ]
  }]
}
```

Responses use standard `chat.completion` objects, assistant messages, function calls, finish reasons and token usage. The optional `provider_router` extension records the actual connection, upstream model, provider, implementation method and reasoning effort. Standard SDKs can ignore this extension.

With `stream: true`, the service returns `chat.completion.chunk` SSE frames, a final finish reason, an optional usage-only chunk, and `data: [DONE]`. Version 1 buffers provider inference and then emits valid completion chunks; it does not promise token-by-token latency.

## POST /v1/responses

Supported request fields are `model`, `instructions`, `input`, `tools`, `tool_choice`, `parallel_tool_calls`, `text.format`, `reasoning.effort`, `max_output_tokens`, `stream` and `store: false`. Input accepts a string, role-bearing messages, `function_call` items and their matching `function_call_output` items. User messages accept inline `input_file` blocks. Tools use Responses' flat function shape. `text.format.type = "json_schema"` requests structured output.

```json
{
  "model": "019a1111-1111-7111-8111-111111111111/gpt-6-luna",
  "input": [{
    "role": "user",
    "content": [
      {"type": "input_text", "text": "What does the note say?"},
      {"type": "input_file", "filename": "note.txt", "file_data": "data:text/plain;base64,SGVsbG8h"}
    ]
  }],
  "store": false
}
```

Completed responses use `object: "response"`, `status: "completed"`, standard message/function output items and Responses usage fields. SSE emits standard response, output-item, content-part, text/function-argument and completion events. Send authoritative history on every request. Response retrieval, stored conversation IDs, background Responses, provider-native encrypted reasoning state, standalone image and audio inputs, and unsupported request fields produce a standard OpenAI error object.

## POST /v1/attachment-checks

Send a Chat Completions request body with the intended model and attachments to validate file types, extraction limits and model image support locally. The endpoint uses the same client authentication. Use a fresh UUIDv7 `X-Request-ID` for each call if you supply correlation IDs. Success returns a `chat.completion` with assistant content `"Attachments fit the request."` and zero token usage. Send a separate inference request for the model answer.

```json
{
  "model": "019a1111-1111-7111-8111-111111111111/gpt-6-luna",
  "messages": [{
    "role": "user",
    "content": [{"type": "file", "file": {"filename": "note.txt", "file_data": "data:text/plain;base64,SGVsbG8h"}}]
  }]
}
```

## Attachment formats and limits

Send a filename and a base64 data URL with the media type for its extension. Supported extensions are `.pdf`, `.docx`, `.txt`, `.md`, `.markdown`, `.csv` and `.xlsx`. Remote URLs and uploaded file IDs are unsupported. Router processes attachments locally for both Codex and OpenRouter. It gives the model one user message per file, immediately before the user message that included the files. For formats other than PDF, the attachment message includes the filename alongside the extracted content.

- PDFs supply only a JPEG image of each page, in page order. The images let a vision model read the page and see tables, charts and layout.
- DOCX supplies text, tables and supported embedded images. Text and Markdown supply their original UTF-8 text.
- XLSX supplies a grid for each sheet with spreadsheet column letters and row numbers. Cells use stored values where available, and formulas are included separately without recalculation. CSV supplies one grid with the same coordinates and preserves quoted fields.

The limits apply across all attachments in a request. Router rejects content that exceeds a limit before sending anything to the provider.

| Error code | HTTP status | Limit |
| --- | --- | --- |
| `FILE_REQUEST_LIMIT` | 413 | Up to five files, 10 MiB per file, and 25 MiB combined before base64 encoding. |
| `FILE_CONTEXT_LIMIT` | 413 | Up to 256 KiB of extracted UTF-8 text including filenames and labels, reduced if the model advertises a smaller context window. Up to 20 images and 12 MiB of encoded image data URLs across the request, and up to 20 pages per PDF. The text cap measures bytes. |
| `REQUEST_TOO_LARGE` | 413 | Up to 36 MiB for the complete JSON request. |

PDFs and documents with extracted images require a model that advertises image inputs. Otherwise Router returns `FILE_IMAGES_UNSUPPORTED`. A file whose extension or media type is unsupported returns `FILE_TYPE_UNSUPPORTED`. Empty or unreadable documents return `FILE_UNREADABLE`.

## GET /v1/providers

This extension returns `{"protocol_version":1,"providers":[...]}`. Each provider entry contains `id`, `name`, `provider`, `method` and `models`. A model includes `id`, `name`, `reasoning_efforts`, `default_reasoning_effort`, `context_window`, `max_output_tokens`, `tools`, `structured_output` and `input_modalities`. The model's ID in this extension is its upstream ID; `/v1/models` publishes the qualified SDK ID.

OpenRouter reasoning choices come from the published model catalog. Codex choices come from the authenticated subscription catalog. Codex inference uses direct subscription HTTP (`codex_http`). The `image` modality indicates support for images extracted from attachments. If a catalog omits a maximum output size, Router applies a conservative local cap of one fifth of the context size; this is a Router policy cap, not a claim about the provider's native limit. Codex output budgets are checked after completion because these transports do not expose a supported hard output cap; a budget does not cap upstream billing or subscription usage.

## Request cancellation and errors

Disconnecting or aborting an inference HTTP request cancels its local provider operation. A client may also supply a UUIDv7 `X-Request-ID` correlation value and call `POST /v1/inference/<request-id>/cancel` with the same client key. Other arbitrary `X-Request-ID` values are accepted as ordinary client metadata while internal inference identities remain UUIDv7. With keys enabled, cancellation cannot target another client's work. Without keys, local applications share a cancellation identity; do not rely on per-application isolation in that mode. There is no provider or funding fallback.

Errors use `{"error":{"message":"Safe explanation","type":"invalid_request_error","param":null,"code":"STABLE_CODE"}}`. Authentication, rate-limit, unsupported capability, invalid output, cancellation and provider failures have stable codes. Upstream raw errors and request contents are never reflected. JSON Schema and tool arguments are validated before success is returned.

The Router maps upstream Codex and OpenRouter inference responses as follows. For embedded OpenRouter errors, it checks the top-level error first and checks the first choice only if the top-level error is absent or null.

| Upstream result | Router HTTP status | `error.code` |
| --- | --- | --- |
| 2xx | Normal completion handling | |
| 400, 413, 422 | 400 | `PROVIDER_REQUEST_REJECTED` |
| 401 | 502 | `PROVIDER_AUTH_REQUIRED` |
| 429 | 429 | `PROVIDER_RATE_LIMIT` |
| All other non-2xx statuses, including 403, 404 and 5xx, or a transport failure | 502 | `PROVIDER_UNAVAILABLE` |
| Embedded OpenRouter error with an integer `code` from 400 through 599 | As if the code were the upstream HTTP status | As if the code were the upstream HTTP status |
| Any other embedded OpenRouter error | 502 | `INCOMPLETE_COMPLETION` |
| Codex stream failure with `server_error` | 502 | `PROVIDER_UNAVAILABLE` |
| Codex stream failure with `rate_limit_exceeded` | 429 | `PROVIDER_RATE_LIMIT` |
| Any other Codex stream failure or incomplete response | 502 | `INCOMPLETE_COMPLETION` |

For example, if the provider rejects the requested output schema, the Router returns this response with HTTP 400.

```json
{"error":{"message":"The provider rejected the request as invalid.","type":"invalid_request_error","param":null,"code":"PROVIDER_REQUEST_REJECTED"}}
```

The Router's inference deadline returns HTTP 504 with `error.code` set to `ROUTER_TIMEOUT`. Explicit client cancellation returns HTTP 499 with `error.code` set to `CANCELLED` on the in-flight inference request.

The versioned extension `POST /v1/inference` accepts an explicit UUIDv7 request and connection ID and returns the normalized completion contract. The standard OpenAI endpoints above work with ordinary SDK clients.

The native response can include `continuation` with its `connection_id`, selected `model` and provider-owned `state`. Add that complete object to the corresponding assistant message on the next request, followed by any matching tool results. Codex state contains the ordered Responses items. OpenRouter state contains its assistant message and resolved upstream route. An optional UUIDv7 `session_id` groups related requests for the provider's cache routing. Usage includes `cached_input_tokens` and `cache_write_tokens` when the provider reports them as unsigned 64-bit JSON integers.

Non-streaming `POST /v1/responses` also returns the continuation under `provider_router.continuation`. An application using this extension can send the complete normalized history as `provider_router_messages` alongside its stateless Responses input.
