/** Construct product workflows with explicit repository and service boundaries. */
import type { Method, ProviderAdapter } from '../../common/schemas/contracts.ts';
import { ConfiguredProviderService } from '../../services/provider-service/service.ts';
import type { RouterRepository } from './repository/settings/contract.ts';
import { InferenceUseCase } from './use-cases/inference/use-case.ts';
export function buildInferenceUseCase(repository: RouterRepository, adapters: Record<Method, ProviderAdapter>): InferenceUseCase {
  return new InferenceUseCase(repository, new ConfiguredProviderService(adapters));
}  // RevExt: 1

import { ManageUseCase, type SettingsRuntime } from './use-cases/manage/use-case.ts';
export function buildManageUseCase(repository: RouterRepository, adapters: Record<Method, ProviderAdapter>, runtime: SettingsRuntime): ManageUseCase {
  return new ManageUseCase(repository, new ConfiguredProviderService(adapters), runtime);
}  // RevExt: 2
