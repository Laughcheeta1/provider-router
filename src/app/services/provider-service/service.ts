/** Execute configured provider methods through injected external adapters. */
import { prepareDocuments } from './documents.ts';
import type { Connection, InferenceRequest, InferenceResult, Method, Model, ProviderAdapter } from '../../common/schemas/contracts.ts';
export interface ProviderService {
  checkAttachments(input: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult>;
  infer(input: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult>;
  models(connection: Connection, signal: AbortSignal): Promise<Model[]>;
}
export class ConfiguredProviderService implements ProviderService {
  constructor(private readonly adapters: Record<Method, ProviderAdapter>) {}
  /** Check the same files and budget as inference without contacting an upstream provider. */
  async checkAttachments(input: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult> {
    await this.prepare(input, connection, signal);
    return {
      request_id: input.request_id, content: 'Attachments fit the request.', tool_calls: [],
      usage: { input_tokens: 0, output_tokens: 0 }, method: connection.method,
      provider: connection.method === 'openrouter_http' ? 'openrouter' : 'codex', model: input.model,
    };
  }
  async infer(input: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult> {
    const messages: InferenceRequest['messages'] = await this.prepare(input, connection, signal);
    signal.throwIfAborted();
    return this.adapters[connection.method].infer({ ...input, messages }, connection, signal);
  }
  models(connection: Connection, signal: AbortSignal): Promise<Model[]> { return this.adapters[connection.method].models(connection, signal); }
  private prepare(input: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceRequest['messages']> {
    const model: Model = connection.models.find((entry: Model): boolean => entry.id === input.model)!;
    return prepareDocuments(input.messages, model, signal);
  }
}
