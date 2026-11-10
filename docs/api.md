# API contract

The public inference API implements a documented subset of the OpenAI API. Set an OpenAI SDK's `baseURL` to `http://127.0.0.1:47831/v1` and its `apiKey` to the client key created in Provider Router. Native application accounts and provider credentials are not API inputs. Use HTTPS for remotely exposed endpoints.

## GET /health

This public identity probe returns only installation identity and protocol compatibility:

```json
{"name":"provider-router","protocol_version":1,"instance_id":"019a1111-1111-7111-8111-111111111111","version":"0.1.0"}
```

## GET /v1/models

Authenticated model discovery uses the OpenAI list shape. Every ID qualifies a model with its exposed connection identity. This allows two subscriptions to expose the same upstream model independently.

```json
{"object":"list","data":[{"id":"019a1111-1111-7111-8111-111111111111/gpt-6-luna","object":"model","created":0,"owned_by":"openai"}]}
```

An unqualified upstream model ID is accepted only when exactly one exposed connection offers it. Ambiguous IDs are rejected before inference.

## POST /v1/chat/completions

Supported request fields are `model`, `messages`, `tools`, `tool_choice`, `parallel_tool_calls`, `response_format`, `reasoning_effort`, `max_tokens` or `max_completion_tokens`, `stream`, and `stream_options.include_usage`. Messages retain system/developer/user/assistant/tool roles, assistant `tool_calls`, and `tool_call_id` associations. Text content strings and text content blocks are supported. Function tools use normal OpenAI JSON schemas. Structured output uses `response_format.type = "json_schema"`.

```javascript
import OpenAI from 'openai';
const client = new OpenAI({ baseURL: 'http://127.0.0.1:47831/v1', apiKey: process.env.PROVIDER_ROUTER_KEY });
const models = await client.models.list();
const answer = await client.chat.completions.create({
  model: models.data[0].id,
  messages: [{ role: 'user', content: 'What is seven plus seven?' }],
  reasoning_effort: 'low',
});
console.log(answer.choices[0].message.content);
```

Responses use standard `chat.completion` objects, assistant messages, function calls, finish reasons and token usage. The optional `provider_router` extension records the actual connection, upstream model, provider, implementation method and reasoning effort. Standard SDKs can ignore this extension.

With `stream: true`, the service returns `chat.completion.chunk` SSE frames, a final finish reason, an optional usage-only chunk, and `data: [DONE]`. Version 1 buffers provider inference and then emits valid completion chunks; it does not promise token-by-token latency.

## POST /v1/responses

Supported request fields are `model`, `instructions`, `input`, `tools`, `tool_choice`, `parallel_tool_calls`, `text.format`, `reasoning.effort`, `max_output_tokens`, `stream` and `store: false`. Input accepts a string, role-bearing text messages, `function_call` items and their matching `function_call_output` items. Tools use Responses' flat function shape. `text.format.type = "json_schema"` requests structured output.

```javascript
const answer = await client.responses.create({
  model: models.data[0].id,
  instructions: 'Answer the user briefly.',
  input: 'What is seven plus seven?',
  reasoning: { effort: 'low' },
  store: false,
});
console.log(answer.output_text);
```

Completed responses use `object: "response"`, `status: "completed"`, standard message/function output items and Responses usage fields. SSE emits standard response, output-item, content-part, text/function-argument and completion events. Inference is stateless: send authoritative history on every request. Response retrieval, stored conversation IDs, background Responses, provider-native encrypted reasoning state, images, files, audio and other unrepresented options are rejected rather than discarded. Unsupported request fields produce a standard OpenAI error object.

## GET /v1/providers

This authenticated extension returns `{"protocol_version":1,"providers":[...]}`. Each provider entry contains `id`, `name`, `provider`, `method` and `models`. A model includes `id`, `name`, `reasoning_efforts`, `default_reasoning_effort`, `context_window`, `max_output_tokens`, `tools`, `structured_output` and `input_modalities`. The model's ID in this extension is its upstream ID; `/v1/models` publishes the qualified SDK ID.

OpenRouter reasoning choices come from the published model catalog. Codex choices come from the installed Codex catalog or authenticated subscription catalog. Text is the only exposed input modality in version 1. If a catalog omits a maximum output size, Router applies a conservative local cap of one fifth of the context size; this is a Router policy cap, not a claim about the provider's native limit. Codex output budgets are checked after completion because these transports do not expose a supported hard output cap; a budget does not cap upstream billing or subscription usage.

## Request cancellation and errors

Disconnecting or aborting an inference HTTP request cancels its local provider operation. A client may also supply a UUIDv7 `X-Request-ID` correlation value and call `POST /v1/inference/<request-id>/cancel` with the same client key. Other arbitrary `X-Request-ID` values are accepted as ordinary client metadata while internal inference identities remain UUIDv7. Cancellation cannot target another client's work. There is no provider or funding fallback.

Errors use `{"error":{"message":"Safe explanation","type":"invalid_request_error","param":null,"code":"STABLE_CODE"}}`. Authentication, rate-limit, unsupported capability, invalid output, cancellation and provider failures have stable codes. Upstream raw errors and request contents are never reflected. JSON Schema and tool arguments are validated before success is returned.

The versioned extension `POST /v1/inference` accepts an explicit UUIDv7 request and connection ID and returns the normalized completion contract. It is retained for protocol integrations; ordinary applications should use the standard OpenAI endpoints above.
