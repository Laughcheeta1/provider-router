/** Own authorization, request limits, replay protection and inference cancellation. */
import { resultSchema, RouterError, type InferenceRequest, type InferenceResult, type Connection, type Model } from '../../../../common/schemas/contracts.ts';
import type { ProviderService } from '../../../../services/provider-service/service.ts';
import type { RouterRepository } from '../../repository/settings/contract.ts';
import { validateRequest, validateResult } from './validation.ts';
interface Pending { client_id: string; abort: AbortController }
export class InferenceUseCase {
  private readonly active: Map<string, Pending> = new Map();
  private readonly recent: Set<string> = new Set();
  constructor(private readonly repository: RouterRepository, private readonly providers: ProviderService) {}
  cancel(clientId: string, requestId: string): void {
    const pending: Pending | undefined = this.active.get(requestId);
    if (!pending || pending.client_id !== clientId) throw new RouterError('REQUEST_NOT_FOUND', 'No active request was found for this client.', 404);
    pending.abort.abort();
  }
  dispose(): void { for (const item of this.active.values()) item.abort.abort(); }
  async execute(clientId: string, token: string | null, input: InferenceRequest, abort: AbortController): Promise<InferenceResult> {
    const connection: Connection | undefined = this.repository.connection(input.connection_id);
    if (!connection?.exposed) throw new RouterError('CONNECTION_UNAVAILABLE', 'The requested connection is not exposed.', 404);
    validateRequest(input, connection.models);
    // An omitted effort still respects the exposed selection instead of falling back to a hidden provider default.
    const configuredModel: Model = connection.models.find((model): boolean => model.id === input.model)!;
    const effectiveInput: InferenceRequest = { ...input, reasoning_effort: input.reasoning_effort ?? configuredModel.default_reasoning_effort ?? configuredModel.reasoning_efforts[0] };

    if (this.active.has(input.request_id) || this.recent.has(input.request_id)) throw new RouterError('REQUEST_REPLAYED', 'This request identifier has already been used.', 409);
    if (this.active.size >= 16 || [...this.active.values()].filter((item: Pending): boolean => item.client_id === clientId).length >= 4) throw new RouterError('CLIENT_BUSY', 'Too many inference requests are active.', 429);
    const deadline: ReturnType<typeof setTimeout> = setTimeout((): void => abort.abort(), 600_000);
    this.active.set(input.request_id, { client_id: clientId, abort });
    try {
      const result: InferenceResult = resultSchema.parse(await this.providers.infer(effectiveInput, connection, abort.signal));
      abort.signal.throwIfAborted();
      if (token !== null ? !this.repository.authenticate(token) : this.repository.publicState().network.require_api_key) throw new RouterError('CLIENT_UNAUTHORIZED', 'The client key was revoked.', 401);
      if (result.request_id !== input.request_id || result.model !== input.model || result.method !== connection.method) throw new RouterError('INVALID_COMPLETION', 'The provider returned a mismatched inference response.', 502);
      validateResult(input, result);
      return result;
    } catch (error: unknown) {
      if (abort.signal.aborted) throw new RouterError('CANCELLED', 'The inference request was cancelled.', 499);
      throw error;
    } finally {
      clearTimeout(deadline); this.active.delete(input.request_id); this.recent.add(input.request_id);
      if (this.recent.size > 4096) this.recent.delete(this.recent.values().next().value!);
    }
  }
  async revokeClient(id: string): Promise<void> {
    await this.repository.revokeClient(id);
    for (const item of this.active.values()) { if (item.client_id === id) item.abort.abort(); }
  }
}
