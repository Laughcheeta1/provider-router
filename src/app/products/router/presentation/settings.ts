/** Render safe settings values and explicit mutations through the injected IPC port. */
import type { Connection, Method, Model } from '../../../common/schemas/contracts.ts';
import type { DesktopSettingsBridge, DesktopCommand, SettingsReply, SettingsSnapshot } from '../schemas/settings-contract.ts';
declare global { interface Window { providerRouter: DesktopSettingsBridge } }
const status: HTMLElement = element('status');
const methods: Record<Method, string> = { codex_cli: 'Codex · Direct', codex_http: 'Codex · Direct', openrouter_http: 'OpenRouter' };
let snapshot: SettingsSnapshot | null = null;
let selectedId: string | null = null;
let pending: boolean = false;
function element(id: string): HTMLElement { const value: HTMLElement | null = document.getElementById(id); if (!value) throw new Error('Missing settings element.'); return value; }
function input(form: HTMLFormElement, name: string): HTMLInputElement { return form.elements.namedItem(name) as HTMLInputElement; }
function text(tag: string, value: string, className: string = ''): HTMLElement { const node: HTMLElement = document.createElement(tag); node.textContent = value; node.className = className; return node; }
function button(label: string, action: () => Promise<void>, danger: boolean = false): HTMLButtonElement { const node: HTMLButtonElement = document.createElement('button'); node.type = 'button'; node.textContent = label; node.className = danger ? 'danger' : ''; node.onclick = (): void => { void perform(action); }; return node; }
/** Serialize mutations and keep failures visible in the active dialog or page. */
async function perform(action: () => Promise<void>): Promise<void> {
  if (pending) return;
  pending = true; status.textContent = ''; element('dialog-status').textContent = '';
  const controls: (HTMLButtonElement | HTMLInputElement | HTMLSelectElement)[] = Array.from(document.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>('button:not(#accept-confirm):not(#cancel-confirm), input, select'));
  const disabled: boolean[] = controls.map((control): boolean => control.disabled);
  controls.forEach((control): void => { control.disabled = true; });
  element('refresh').setAttribute('aria-busy', 'true');
  try { await action(); } catch (error: unknown) { const target: HTMLElement = (element('provider-dialog') as HTMLDialogElement).open ? element('dialog-status') : status; target.textContent = error instanceof Error ? error.message : 'The operation failed.'; }
  finally { pending = false; controls.forEach((control, index): void => { control.disabled = disabled[index]!; }); element('refresh').removeAttribute('aria-busy'); }
}
async function invoke(command: DesktopCommand): Promise<Extract<SettingsReply, { ok: true }>['value']> { const reply: SettingsReply = await window.providerRouter.invoke(command); if (!reply.ok) throw new Error(reply.error); return reply.value; }
async function refresh(): Promise<void> {
  snapshot = await invoke({ type: 'snapshot' }) as SettingsSnapshot;
  status.textContent = snapshot.error || '';
  updateEnvironmentFields();
  element('endpoint').textContent = snapshot.endpoint || 'API is stopped.';
  element('toggle-api').textContent = snapshot.endpoint ? 'Stop API' : 'Start API';
  (element('startup-mode') as HTMLSelectElement).value = snapshot.startup_mode;
  element('key-policy').textContent = snapshot.network.require_api_key ? 'Applications must send one of these keys.' : 'Keys are optional. Enable key requirements in API access to restrict applications.';
  renderProviders();
  const clients: HTMLElement = element('clients'); clients.replaceChildren();
  for (const client of snapshot.clients) { const row: HTMLElement = text('div', '', 'card'); row.append(text('span', client.name), button('Revoke', async (): Promise<void> => { await invoke({ type: 'revoke_client', id: client.id }); await refresh(); }, true)); clients.append(row); }
  const network: HTMLFormElement = element('network') as HTMLFormElement;
  (network.elements.namedItem('browser_origins') as HTMLTextAreaElement).value = (snapshot.network.browser_origins ?? []).join('\n');
  for (const [key, value] of Object.entries(snapshot.network)) { const field: HTMLInputElement | null = network.elements.namedItem(key) as HTMLInputElement | null; if (field) { if (typeof value === 'boolean') field.checked = value; else field.value = Array.isArray(value) ? value.join('\n') : String(value || ''); } }
}
/** Keep selection in the renderer while persisted connection identity stays unchanged. */
function renderProviders(): void {
  const saved: readonly Connection[] = snapshot?.connections ?? [];
  const detected = (snapshot?.detected_providers ?? []).filter((candidate): boolean => !saved.some((connection: Connection): boolean => connection.method === candidate.connection.method && (connection.environment ?? 'native') === (candidate.connection.environment ?? 'native') && connection.wsl_distribution === candidate.connection.wsl_distribution && !connection.codex_home && !connection.executable));
  const connections: readonly Connection[] = [...saved, ...detected.map((candidate): Connection => candidate.connection)];
  if (!connections.some((connection: Connection): boolean => connection.id === selectedId)) selectedId = connections[0]?.id ?? null;
  const list: HTMLElement = element('connections'); list.replaceChildren();
  for (const connection of connections) {
    const row: HTMLElement = text('div', '', 'provider-row');
    const select: HTMLButtonElement = document.createElement('button'); select.type = 'button'; select.className = 'provider-select'; select.setAttribute('aria-label', `Select ${connection.name}`);
    select.setAttribute('aria-pressed', String(connection.id === selectedId)); row.dataset.selected = String(connection.id === selectedId);
    const icon: HTMLImageElement = document.createElement('img'); icon.className = 'provider-icon'; icon.alt = ''; icon.src = connection.method === 'openrouter_http' ? 'assets/openrouter.svg' : 'assets/codex.svg';
    const name: HTMLElement = text('span', '', 'provider-label'); name.append(text('strong', connection.name), text('small', detected.find((candidate): boolean => candidate.connection.id === connection.id)?.detail ?? (connection.exposed ? 'Enabled' : 'Disabled')));
    select.append(icon, name);
    select.onclick = (): void => { selectedId = connection.id; renderProviders(); };
    const toggle: HTMLInputElement = document.createElement('input'); toggle.type = 'checkbox'; toggle.role = 'switch'; toggle.checked = connection.exposed; toggle.setAttribute('aria-label', `Enable ${connection.name}`);
    toggle.disabled = detected.some((candidate): boolean => candidate.connection.id === connection.id && candidate.status !== 'ready');
    toggle.onchange = (): void => { void perform(async (): Promise<void> => { await invoke({ type: 'save_connection', connection: { ...connection, exposed: toggle.checked } }); await refresh(); }); };
    row.append(select, toggle); list.append(row);
  }
  const add: HTMLButtonElement = document.createElement('button'); add.type = 'button'; add.className = 'add-provider-row'; add.textContent = '+  Add provider'; add.onclick = openProviderDialog; list.append(add);
  const editor: HTMLElement = element('provider-detail'); editor.replaceChildren();
  const detail: HTMLElement = text('div', '', 'provider-detail-content'); editor.append(detail);
  const connection: Connection | undefined = connections.find((current: Connection): boolean => current.id === selectedId);
  if (!connection) { const empty: HTMLElement = text('div', '', 'empty-state'); empty.append(text('span', '◈', 'empty-icon'), text('h3', 'No providers configured'), text('p', 'Add a provider to make its models available through your API.')); detail.append(empty); return; }
  const header: HTMLElement = text('div', '', 'provider-detail-heading');
  const title: HTMLElement = text('div', ''); title.append(text('h2', connection.name), text('p', methods[connection.method]));
  header.append(title); detail.append(header);
  const candidate = detected.find((current): boolean => current.connection.id === connection.id);
  if (candidate) {
    const setup: HTMLElement = text('div', '', 'detail-section'); setup.append(text('h3', 'Setup'), text('p', candidate.detail));
    if (candidate.status === 'ready') setup.append(button('Use this provider', async (): Promise<void> => { await invoke({ type: 'save_connection', connection }); await refresh(); }));
    else setup.append(text('p', candidate.status === 'login_required' ? 'Run codex login in this environment, then refresh provider status.' : 'Install Codex in this environment, then refresh provider status.'));
    detail.append(setup);
    return;
  }
  const configuration: HTMLElement = text('div', '', 'detail-section configuration-section');
  const form: HTMLFormElement = document.createElement('form'); form.className = 'provider-configuration';
  const label: HTMLLabelElement = document.createElement('label'); label.textContent = 'Display name'; const name: HTMLInputElement = document.createElement('input'); name.name = 'name'; name.value = connection.name; name.required = true; name.maxLength = 100; label.append(name);
  form.append(label);
  const advanced: HTMLDetailsElement = document.createElement('details'); advanced.className = 'advanced-settings provider-runtime'; advanced.append(text('summary', 'Advanced connection settings'));
  for (const field of connection.method === 'openrouter_http' ? ['api_key'] : ['codex_home']) {
    const fieldLabel: HTMLLabelElement = document.createElement('label'); fieldLabel.textContent = field === 'api_key' ? 'Replace provider API key' : 'Codex home';
    const value: HTMLInputElement = document.createElement('input'); value.name = field; value.autocomplete = 'off';
    value.placeholder = field === 'codex_home' ? 'Default Codex home' : 'Enter a new API key'; value.type = field === 'api_key' ? 'password' : 'text'; value.value = field === 'codex_home' ? connection.codex_home ?? '' : ''; fieldLabel.append(value); if (field === 'api_key') form.append(fieldLabel); else advanced.append(fieldLabel);
  }
  if (connection.method !== 'openrouter_http') form.append(advanced);
  let readModels: () => Model[] = (): Model[] => connection.models;
  form.id = 'provider-configuration';
  const footer: HTMLElement = text('div', '', 'provider-save-bar');
  const feedback: HTMLElement = text('span', '', 'provider-save-status'); feedback.setAttribute('role', 'status');
  const save: HTMLButtonElement = document.createElement('button'); save.type = 'submit'; save.textContent = 'Save'; save.className = 'primary'; save.setAttribute('form', form.id); footer.append(feedback, save); editor.append(footer);
  form.onsubmit = (event: SubmitEvent): void => { event.preventDefault(); void perform(async (): Promise<void> => { try { await invoke({ type: 'save_connection', connection: { ...connection, models: readModels(), name: name.value.trim(), ...(connection.method !== 'openrouter_http' ? { codex_home: input(form, 'codex_home').value.trim() || undefined } : {}) }, ...(connection.method === 'openrouter_http' ? { api_key: input(form, 'api_key').value || undefined } : {}) }); await refresh(); } catch (error: unknown) { feedback.textContent = error instanceof Error ? error.message : 'Could not save provider.'; } }); };
  configuration.append(form); detail.append(configuration);
  const authentication: HTMLElement = text('div', '', 'provider-authentication'); authentication.append(text('p', connection.method === 'openrouter_http' ? 'Uses the API key saved with this connection.' : 'Uses your existing Codex login.'));
  if (connection.environment === 'wsl') authentication.append(text('p', `WSL · ${connection.wsl_distribution}`, 'muted'));
  header.append(authentication);
  const models: HTMLElement = text('div', '', 'detail-section'); const modelHeading: HTMLElement = text('div', '', 'section-heading');
  const updateSelection = (available: Model[]): void => { readModels = showModelSelection(models, { ...connection, models: readModels() }, available); };
  const reload: HTMLButtonElement = button('Refresh', async (): Promise<void> => { await detectModels(models, connection, reload, updateSelection); });
  modelHeading.append(text('h3', 'Models'), reload); models.append(modelHeading);
  const progress: HTMLElement = text('p', 'Detecting models…', 'model-detection-status'); progress.setAttribute('role', 'status'); models.append(progress);
  updateSelection(connection.models);
  detail.append(models);
  void detectModels(models, connection, reload, updateSelection);
  const danger: HTMLElement = text('div', '', 'detail-section remove-section'); danger.append(text('p', 'Remove this provider'), button('Remove provider', async (): Promise<void> => { if (await confirmRemoval(connection.name)) { await invoke({ type: 'remove_connection', id: connection.id }); await refresh(); } }, true)); detail.append(danger);
}
function openProviderDialog(): void {
  element('dialog-status').textContent = '';
  const form: HTMLFormElement = element('add-connection') as HTMLFormElement;
  if (!input(form, 'name').value) input(form, 'name').value = input(form, 'method').value === 'openrouter_http' ? 'OpenRouter' : 'Codex';
  setWizardStep(0);
  updateEnvironmentFields();
  (element('provider-dialog') as HTMLDialogElement).showModal();
}
function confirmRemoval(name: string): Promise<boolean> {
  const dialog: HTMLDialogElement = element('confirm-dialog') as HTMLDialogElement;
  element('confirm-description').textContent = `Applications will no longer be able to use ${name}.`;
  return new Promise((resolve: (value: boolean) => void): void => {
    element('cancel-confirm').onclick = (): void => { dialog.close(); resolve(false); };
    element('accept-confirm').onclick = (): void => { dialog.close(); resolve(true); };
    dialog.oncancel = (): void => { resolve(false); }; dialog.showModal();
  });
}
function navigate(): void {
  if (location.hash === '#applications') { location.replace('#api-access'); return; }
  const page: string = ['providers', 'api-access', 'documentation'].includes(location.hash.slice(1)) ? location.hash.slice(1) : 'providers';
  const names: Record<string, string> = { providers: 'Providers', 'api-access': 'API access', documentation: 'Documentation' };
  for (const id of Object.keys(names)) element(id).hidden = id !== page;
  for (const link of Array.from(document.querySelectorAll<HTMLAnchorElement>('.sidebar-nav a'))) { if (link.hash === `#${page}`) link.setAttribute('aria-current', 'page'); else link.removeAttribute('aria-current'); }
  element('page-title').textContent = names[page]!;
}
function showModelSelection(card: HTMLElement, connection: Connection, available: Model[]): () => Model[] {
  card.querySelector('.model-selection')?.remove();
  const selection: HTMLElement = text('div', '', 'model-selection');
  const search: HTMLInputElement = document.createElement('input'); search.type = 'search'; search.placeholder = 'Search models…'; search.setAttribute('aria-label', 'Search models');
  const list: HTMLElement = text('div', '', 'models'); const selected: Map<string, HTMLInputElement> = new Map(); const efforts: Map<string, Map<string, HTMLInputElement>> = new Map();
  for (const model of available) {
    const label: HTMLLabelElement = document.createElement('label'); label.className = 'model-row'; const checkbox: HTMLInputElement = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.checked = connection.models.some((current: Model): boolean => current.id === model.id); selected.set(model.id, checkbox);
    const description: HTMLElement = text('span', ''); description.append(text('strong', model.name), text('small', model.reasoning_efforts.join(' / ') || 'Default reasoning')); label.append(checkbox, description); label.dataset.search = (model.name + ' ' + model.id).toLowerCase(); list.append(label);
    const choices: Map<string, HTMLInputElement> = new Map(); efforts.set(model.id, choices);
    const allowed: Model | undefined = connection.models.find((current: Model): boolean => current.id === model.id);
    const effortGroup: HTMLFieldSetElement = document.createElement('fieldset'); effortGroup.append(text('legend', `${model.name} thinking efforts`));
    for (const effort of model.reasoning_efforts) {
      const effortLabel: HTMLLabelElement = document.createElement('label'); effortLabel.className = 'check';
      const option: HTMLInputElement = document.createElement('input'); option.type = 'checkbox'; option.checked = !allowed || allowed.reasoning_efforts.includes(effort); option.setAttribute('aria-label', `${model.name}: allow ${effort}`); choices.set(effort, option);
      effortLabel.append(option, text('span', effort)); effortGroup.append(effortLabel);
    }
    effortGroup.hidden = !checkbox.checked || !choices.size;
    checkbox.onchange = (): void => { effortGroup.hidden = !checkbox.checked || !choices.size; };
    label.after(effortGroup);
  }
  search.oninput = (): void => { for (const row of Array.from(list.querySelectorAll<HTMLElement>('[data-search]'))) { row.hidden = !row.dataset.search!.includes(search.value.toLowerCase()); const group: Element | null = row.nextElementSibling; if (group instanceof HTMLFieldSetElement) group.hidden = row.hidden || !(row.querySelector('input') as HTMLInputElement).checked || !group.querySelector('input'); } };
  if (!available.length) list.append(text('p', 'No models returned by this provider.'));
  selection.append(search, list); card.append(selection);
  return (): Model[] => available.filter((model: Model): boolean => Boolean(selected.get(model.id)?.checked)).map((model: Model): Model => {
    const permitted: string[] = model.reasoning_efforts.filter((effort: string): boolean => Boolean(efforts.get(model.id)?.get(effort)?.checked));
    if (model.reasoning_efforts.length && !permitted.length) throw new Error(`Choose at least one thinking effort for ${model.name}.`);
    return { ...model, reasoning_efforts: permitted, default_reasoning_effort: model.default_reasoning_effort && permitted.includes(model.default_reasoning_effort) ? model.default_reasoning_effort : permitted[0] ?? null };
  });
}
element('refresh').onclick = (): void => { void perform(refresh); };
element('updates').onclick = (): void => { void perform(async (): Promise<void> => { await invoke({ type: 'check_updates' }); }); };
element('dismiss-key').onclick = (): void => { (element('client-token') as HTMLInputElement).value = ''; element('new-key').hidden = true; };
(element('add-connection') as HTMLFormElement).onsubmit = (event: SubmitEvent): void => {
  event.preventDefault(); const form: HTMLFormElement = event.currentTarget as HTMLFormElement;
  if (wizardStep !== 2) { setWizardStep(wizardStep + 1); return; }
  void perform(async (): Promise<void> => {
    const home: string = input(form, 'codex_home').value.trim();
    const connection: Connection = { id: uuidV7(), name: input(form, 'name').value.trim(), method: input(form, 'method').value as Connection['method'], ...(input(form, 'method').value === 'codex_http' && input(form, 'environment').value === 'wsl' ? { environment: 'wsl' as const, wsl_distribution: input(form, 'wsl_distribution').value } : { environment: 'native' as const }), exposed: false, models: [], ...(home ? { codex_home: home } : {}) };
    await invoke({ type: 'save_connection', connection, api_key: input(form, 'api_key').value || undefined }); selectedId = connection.id; form.reset(); (element('provider-dialog') as HTMLDialogElement).close(); await refresh();
  });
};
(element('add-client') as HTMLFormElement).onsubmit = (event: SubmitEvent): void => {
  event.preventDefault(); const form: HTMLFormElement = event.currentTarget as HTMLFormElement;
  void perform(async (): Promise<void> => { const client = await invoke({ type: 'create_client', name: input(form, 'name').value }) as { id: string; token: string }; (element('client-token') as HTMLInputElement).value = client.token; element('new-key').hidden = false; form.reset(); await refresh(); });
};
element('toggle-api').onclick = (): void => {
  void perform(async (): Promise<void> => { await invoke({ type: snapshot?.endpoint ? 'stop_api' : 'start_api' }); await refresh(); });
};
const startupMode: HTMLSelectElement = element('startup-mode') as HTMLSelectElement;
startupMode.onchange = (): void => {
  const mode: string = startupMode.value;
  if (mode === 'startup' || mode === 'keep-after-open' || mode === 'while-open') {
    void perform(async (): Promise<void> => {
      try { await invoke({ type: 'save_startup', mode }); }
      finally { await refresh(); }
    });
  }
};
(element('network') as HTMLFormElement).onsubmit = (event: SubmitEvent): void => {
  event.preventDefault(); const form: HTMLFormElement = event.currentTarget as HTMLFormElement;
  void perform(async (): Promise<void> => { await invoke({ type: 'save_network', network: { browser_origins: (form.elements.namedItem('browser_origins') as HTMLTextAreaElement).value.split(/\r?\n/).map((origin: string): string => origin.trim()).filter(Boolean), require_api_key: input(form, 'require_api_key').checked, port: Number(input(form, 'port').value), host: input(form, 'host').value.trim() } }); await refresh(); });
};
/** Renderer identity generation uses only browser cryptography; it never receives Node. */
function uuidV7(): string { const bytes: Uint8Array = crypto.getRandomValues(new Uint8Array(16)); let timestamp: number = Date.now(); for (let index: number = 5; index >= 0; index--) { bytes[index] = timestamp % 256; timestamp = Math.floor(timestamp / 256); } bytes[6] = bytes[6]! & 0x0f | 0x70; bytes[8] = bytes[8]! & 0x3f | 0x80; const value: string = [...bytes].map((byte: number): string => byte.toString(16).padStart(2, '0')).join(''); return `${value.slice(0, 8)}-${value.slice(8, 12)}-${value.slice(12, 16)}-${value.slice(16, 20)}-${value.slice(20)}`; }
window.addEventListener('hashchange', navigate);
window.providerRouter.onShow((): void => { void perform(refresh); });
navigate();
element('close-provider').onclick = (): void => { (element('provider-dialog') as HTMLDialogElement).close(); };
void perform(refresh);
/** Keep environment choices local to Windows; each distribution retains its own Codex login. */
function updateEnvironmentFields(): void {
  const form: HTMLFormElement = element('add-connection') as HTMLFormElement;
  const method: string = input(form, 'method').value;
  const environment: HTMLSelectElement = form.elements.namedItem('environment') as HTMLSelectElement;
  const distributions: HTMLSelectElement = form.elements.namedItem('wsl_distribution') as HTMLSelectElement;
  const selected: string = distributions.value;
  const available: readonly string[] = snapshot?.wsl_distributions ?? [];
  distributions.replaceChildren(...available.map((name: string): HTMLOptionElement => new Option(name, name)));
  if (available.includes(selected)) distributions.value = selected;
  const windows: boolean = snapshot?.platform === 'win32' && method === 'codex_http';
  element('codex-environment-field').hidden = !windows;
  const wsl: boolean = windows && environment.value === 'wsl';
  element('wsl-distribution-field').hidden = !wsl;
  distributions.required = wsl;
  element('connection-method-field').hidden = true;
  element('api-key-field').hidden = method !== 'openrouter_http';
  input(form, 'api_key').required = method === 'openrouter_http';
  element('codex-advanced-field').hidden = method === 'openrouter_http';
}
(element('add-connection') as HTMLFormElement).querySelector('[name="method"]')?.addEventListener('change', updateEnvironmentFields);
(element('add-connection') as HTMLFormElement).querySelector('[name="environment"]')?.addEventListener('change', updateEnvironmentFields);
let wizardStep: number = 0;
/** Keep the picker, identity and configuration panels aligned with the reference wizard. */
function setWizardStep(step: number): void {
  const form: HTMLFormElement = element('add-connection') as HTMLFormElement;
  if (step > 1 && !input(form, 'name').reportValidity()) return;
  wizardStep = step;
  for (const panel of Array.from(document.querySelectorAll<HTMLElement>('[data-panel]'))) panel.hidden = Number(panel.dataset.panel) !== step;
  for (const control of Array.from(document.querySelectorAll<HTMLButtonElement>('[data-step]'))) {
    const index: number = Number(control.dataset.step);
    if (index === step) control.setAttribute('aria-current', 'step'); else control.removeAttribute('aria-current');
    control.disabled = index > step;
  }
  element('wizard-back').textContent = step === 0 ? 'Cancel' : 'Back';
  element('wizard-next').hidden = step === 2;
  element('wizard-submit').hidden = step !== 2;
  element('wizard-next').textContent = step === 0 && input(form, 'method').value !== 'openrouter_http' ? 'Configure manually' : 'Next';
}
element('wizard-back').onclick = (): void => { if (wizardStep === 0) (element('provider-dialog') as HTMLDialogElement).close(); else setWizardStep(wizardStep - 1); };
element('wizard-next').onclick = (): void => { setWizardStep(wizardStep + 1); };
for (const control of Array.from(document.querySelectorAll<HTMLButtonElement>('[data-step]'))) control.onclick = (): void => { setWizardStep(Number(control.dataset.step)); };
for (const radio of Array.from(document.querySelectorAll<HTMLInputElement>('[name="provider"]'))) radio.onchange = (): void => {
  const form: HTMLFormElement = element('add-connection') as HTMLFormElement;
  input(form, 'method').value = radio.value === 'openrouter' ? 'openrouter_http' : 'codex_http';
  if (!input(form, 'name').value || ['Codex', 'OpenRouter'].includes(input(form, 'name').value)) input(form, 'name').value = radio.value === 'openrouter' ? 'OpenRouter' : 'Codex';
  element('connection-method-field').hidden = true;
  updateEnvironmentFields(); setWizardStep(0);
};
/** Refresh metadata on entry without changing the persisted model allow-list. Detached views ignore late results. */
async function detectModels(section: HTMLElement, connection: Connection, reload: HTMLButtonElement, updateSelection: (available: Model[]) => void): Promise<void> {
  if (section.getAttribute('aria-busy') === 'true') return;
  section.setAttribute('aria-busy', 'true'); reload.disabled = true;
  const progress: HTMLElement = section.querySelector<HTMLElement>('.model-detection-status')!;
  progress.textContent = 'Detecting models…';
  try {
    const available: Model[] = await invoke({ type: 'refresh_models', id: connection.id }) as Model[];
    if (!section.isConnected) return;
    for (const row of Array.from(section.querySelectorAll(':scope > .model-row'))) row.remove();
    progress.textContent = available.length ? 'Choose the models available to applications.' : 'No models returned by this provider.';
    updateSelection(available);
  } catch (error: unknown) {
    if (section.isConnected) progress.textContent = error instanceof Error ? error.message : 'Could not detect models. Try Refresh.';
  } finally {
    section.removeAttribute('aria-busy'); reload.disabled = false;
  }
}
