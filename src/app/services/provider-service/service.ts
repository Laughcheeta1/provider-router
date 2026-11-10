/** Execute configured provider methods through injected external adapters. */
import type { Connection, InferenceRequest, InferenceResult, Method, Model, ProviderAdapter } from '../../common/schemas/contracts.ts';
export interface ProviderService {
  infer(input: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult>;
  models(connection: Connection, signal: AbortSignal): Promise<Model[]>;
}
export class ConfiguredProviderService implements ProviderService {
  constructor(private readonly adapters: Record<Method, ProviderAdapter>) {}
  infer(input: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult> { return this.adapters[connection.method].infer(input, connection, signal); }
  models(connection: Connection, signal: AbortSignal): Promise<Model[]> { return this.adapters[connection.method].models(connection, signal); }
}
