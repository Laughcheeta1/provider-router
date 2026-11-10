/** Render safe settings values and explicit mutations through the injected IPC port. */
import type { Connection, Method, Model } from '../../../common/schemas/contracts.ts';
import type { SettingsBridge, SettingsCommand, SettingsReply, SettingsSnapshot } from '../schemas/settings-contract.ts';
// RevExt: 1
declare global { interface Window { providerRouter: SettingsBridge } }
const status: HTMLElement = element('status');
const methods: Record<Method, string> = { codex_cli: 'Codex · CLI', codex_http: 'Codex · Direct', openrouter_http: 'OpenRouter' };
let snapshot: SettingsSnapshot | null = null;
let selectedId: string | null = null;
let pending: boolean = false;
// RevExt: 2
function element(id: string): HTMLElement { const value: HTMLElement | null = document.getElementById(id); if (!value) throw new Error('Missing settings element.'); return value; }
function input(form: HTMLFormElement, name: string): HTMLInputElement { return form.elements.namedItem(name) as HTMLInputElement; }
function text(tag: string, value: string, className: string = ''): HTMLElement { const node: HTMLElement = document.createElement(tag); node.textContent = value; node.className = className; return node; }
function button(label: string, action: () => Promise<void>, danger: boolean = false): HTMLButtonElement { const node: HTMLButtonElement = document.createElement('button'); node.type = 'button'; node.textContent = label; node.className = danger ? 'danger' : ''; node.onclick = (): void => { void perform(action); }; return node; }
// RevExt: 3
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
}  // RevExt: 13
async function invoke(command: SettingsCommand): Promise<Extract<SettingsReply, { ok: true }>['value']> { const reply: SettingsReply = await window.providerRouter.invoke(command); if (!reply.ok) throw new Error(reply.error); return reply.value; }
// RevExt: 4
async function refresh(): Promise<void> {
  snapshot = await invoke({ type: 'snapshot' }) as SettingsSnapshot;
  status.textContent = snapshot.error || '';
  updateEnvironmentFields();  // RevExt: 21
  element('endpoint').textContent = snapshot.endpoint || 'API is stopped.';
  element('key-policy').textContent = snapshot.network.require_api_key ? 'Applications must send one of these keys.' : 'Keys are optional. Enable key requirements in API access to restrict applications.';
  renderProviders();
  const clients: HTMLElement = element('clients'); clients.replaceChildren();
  for (const client of snapshot.clients) { const row: HTMLElement = text('div', '', 'card'); row.append(text('span', client.name), button('Revoke', async (): Promise<void> => { await invoke({ type: 'revoke_client', id: client.id }); await refresh(); }, true)); clients.append(row); }
  const network: HTMLFormElement = element('network') as HTMLFormElement;
  (network.elements.namedItem('browser_origins') as HTMLTextAreaElement).value = (snapshot.network.browser_origins ?? []).join('\n');
  for (const [key, value] of Object.entries(snapshot.network)) { const field: HTMLInputElement | null = network.elements.namedItem(key) as HTMLInputElement | null; if (field) { if (typeof value === 'boolean') field.checked = value; else field.value = Array.isArray(value) ? value.join('\n') : String(value || ''); } }
}  // RevExt: 14
// RevExt: 5
/** Keep selection in the renderer while persisted connection identity stays unchanged. */
function renderProviders(): void {
  const connections: readonly Connection[] = snapshot?.connections ?? [];
  if (!connections.some((connection: Connection): boolean => connection.id === selectedId)) selectedId = connections[0]?.id ?? null;
  const list: HTMLElement = element('connections'); list.replaceChildren();
  for (const connection of connections) {
    const row: HTMLButtonElement = document.createElement('button'); row.type = 'button'; row.className = 'provider-row';
    row.setAttribute('aria-pressed', String(connection.id === selectedId));
    const icon: HTMLElement = text('span', connection.method === 'openrouter_http' ? '↗' : '◈', 'provider-icon');
    const name: HTMLElement = text('span', '', 'provider-label'); name.append(text('strong', connection.name), text('small', methods[connection.method]));
    row.append(icon, name, text('span', connection.exposed ? 'Enabled' : 'Disabled', connection.exposed ? 'provider-state enabled' : 'provider-state'));
    row.onclick = (): void => { selectedId = connection.id; renderProviders(); }; list.append(row);
  }  // RevExt: 25
  const add: HTMLButtonElement = document.createElement('button'); add.type = 'button'; add.className = 'add-provider-row'; add.textContent = '+  Add provider'; add.onclick = openProviderDialog; list.append(add);
  const detail: HTMLElement = element('provider-detail'); detail.replaceChildren();
  const connection: Connection | undefined = connections.find((current: Connection): boolean => current.id === selectedId);
  if (!connection) { const empty: HTMLElement = text('div', '', 'empty-state'); empty.append(text('span', '◈', 'empty-icon'), text('h3', 'No providers configured'), text('p', 'Add a provider to make its models available through your API.')); detail.append(empty); return; }
  const header: HTMLElement = text('div', '', 'provider-detail-heading');
  const title: HTMLElement = text('div', ''); title.append(text('h2', connection.name), text('p', methods[connection.method]));
  const enabled: HTMLInputElement = document.createElement('input'); enabled.type = 'checkbox'; enabled.role = 'switch'; enabled.checked = connection.exposed; enabled.setAttribute('aria-label', 'Enable provider');
  enabled.onchange = (): void => { void perform(async (): Promise<void> => { await invoke({ type: 'save_connection', connection: { ...connection, exposed: enabled.checked } }); await refresh(); }); };
  header.append(title, enabled); detail.append(header);
  const configuration: HTMLElement = text('div', '', 'detail-section'); configuration.append(text('h3', 'Configuration'));
  const form: HTMLFormElement = document.createElement('form'); form.className = 'provider-configuration';
  const label: HTMLLabelElement = document.createElement('label'); label.textContent = 'Label'; const name: HTMLInputElement = document.createElement('input'); name.name = 'name'; name.value = connection.name; name.required = true; name.maxLength = 100; label.append(name);
  form.append(label);
  for (const field of connection.method === 'openrouter_http' ? ['api_key'] : ['codex_home', 'executable']) {
    const fieldLabel: HTMLLabelElement = document.createElement('label'); fieldLabel.textContent = field === 'api_key' ? 'Replace provider API key' : field === 'codex_home' ? 'Codex home' : 'Codex executable';
    const value: HTMLInputElement = document.createElement('input'); value.name = field; value.autocomplete = 'off';
    value.type = field === 'api_key' ? 'password' : 'text'; value.value = field === 'codex_home' ? connection.codex_home ?? '' : field === 'executable' ? connection.executable ?? '' : ''; fieldLabel.append(value); form.append(fieldLabel);
  }  // RevExt: 26
  const save: HTMLButtonElement = document.createElement('button'); save.type = 'submit'; save.textContent = 'Save'; form.append(save);
  form.onsubmit = (event: SubmitEvent): void => { event.preventDefault(); void perform(async (): Promise<void> => { await invoke({ type: 'save_connection', connection: { ...connection, name: name.value.trim(), ...(connection.method !== 'openrouter_http' ? { codex_home: input(form, 'codex_home').value.trim() || undefined, executable: input(form, 'executable').value.trim() || undefined } : {}) }, ...(connection.method === 'openrouter_http' ? { api_key: input(form, 'api_key').value || undefined } : {}) }); await refresh(); }); };
  configuration.append(form); detail.append(configuration);
  const authentication: HTMLElement = text('div', '', 'detail-section'); authentication.append(text('h3', 'Authentication'), text('p', connection.method === 'openrouter_http' ? 'Uses the API key saved with this connection.' : 'Uses your existing Codex login.'));
  if (connection.environment === 'wsl') authentication.append(text('p', `WSL · ${connection.wsl_distribution}`, 'muted'));
  detail.append(authentication);
  const models: HTMLElement = text('div', '', 'detail-section'); const modelHeading: HTMLElement = text('div', '', 'section-heading');
  modelHeading.append(text('h3', 'Models'), button('Refresh models', async (): Promise<void> => { const available: Model[] = await invoke({ type: 'refresh_models', id: connection.id }) as Model[]; if (selectedId === connection.id) showModelSelection(models, connection, available); })); models.append(modelHeading);
  if (!connection.models.length) models.append(text('p', 'No models selected. Refresh models to discover available models.'));
  for (const model of connection.models) { const row: HTMLElement = text('div', '', 'model-row'); const description: HTMLElement = text('div', ''); description.append(text('strong', model.name), text('small', `${model.reasoning_efforts.join(' / ') || 'Default reasoning'} · ${model.context_window.toLocaleString()} context`)); row.append(description, text('span', 'Exposed', 'badge')); models.append(row); }
  detail.append(models);
  const danger: HTMLElement = text('div', '', 'detail-section remove-section'); danger.append(text('p', 'Remove this connection from Provider Router.'), button('Remove provider', async (): Promise<void> => { if (await confirmRemoval(connection.name)) { await invoke({ type: 'remove_connection', id: connection.id }); await refresh(); } }, true)); detail.append(danger);
}  // RevExt: 15
// RevExt: 6
function openProviderDialog(): void {
  element('dialog-status').textContent = '';
  updateEnvironmentFields();  // RevExt: 22
  (element('provider-dialog') as HTMLDialogElement).showModal();
}  // RevExt: 16
// RevExt: 7
function confirmRemoval(name: string): Promise<boolean> {
  const dialog: HTMLDialogElement = element('confirm-dialog') as HTMLDialogElement;
  element('confirm-description').textContent = `Applications will no longer be able to use ${name}.`;
  return new Promise((resolve: (value: boolean) => void): void => {
    element('cancel-confirm').onclick = (): void => { dialog.close(); resolve(false); };
    element('accept-confirm').onclick = (): void => { dialog.close(); resolve(true); };
    dialog.oncancel = (): void => { resolve(false); }; dialog.showModal();
  });  // RevExt: 23
}  // RevExt: 17
// RevExt: 8
function navigate(): void {
  const page: string = ['providers', 'applications', 'api-access', 'documentation'].includes(location.hash.slice(1)) ? location.hash.slice(1) : 'providers';
  const names: Record<string, string> = { providers: 'Providers', applications: 'API keys', 'api-access': 'API access', documentation: 'Documentation' };
  for (const id of Object.keys(names)) element(id).hidden = id !== page;
  for (const link of Array.from(document.querySelectorAll<HTMLAnchorElement>('.sidebar-nav a'))) { if (link.hash === `#${page}`) link.setAttribute('aria-current', 'page'); else link.removeAttribute('aria-current'); }
  element('page-title').textContent = names[page]!; element('breadcrumb-page').textContent = names[page]!;
}  // RevExt: 18
// RevExt: 9
function showModelSelection(card: HTMLElement, connection: Connection, available: Model[]): void {
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
  }  // RevExt: 27
  search.oninput = (): void => { for (const row of Array.from(list.querySelectorAll<HTMLElement>('[data-search]'))) { row.hidden = !row.dataset.search!.includes(search.value.toLowerCase()); const group: Element | null = row.nextElementSibling; if (group instanceof HTMLFieldSetElement) group.hidden = row.hidden || !(row.querySelector('input') as HTMLInputElement).checked || !group.querySelector('input'); } };
  if (!available.length) list.append(text('p', 'No models returned by this provider.'));
  selection.append(search, list, button('Save selected models', async (): Promise<void> => { await invoke({ type: 'save_connection', connection: { ...connection, models: available.filter((model: Model): boolean => Boolean(selected.get(model.id)?.checked)).map((model: Model): Model => {
    const permitted: string[] = model.reasoning_efforts.filter((effort: string): boolean => Boolean(efforts.get(model.id)?.get(effort)?.checked));
    if (model.reasoning_efforts.length && !permitted.length) throw new Error(`Choose at least one thinking effort for ${model.name}.`);
    return { ...model, reasoning_efforts: permitted, default_reasoning_effort: model.default_reasoning_effort && permitted.includes(model.default_reasoning_effort) ? model.default_reasoning_effort : permitted[0] ?? null };
  }) } }); await refresh(); })); card.append(selection);
}  // RevExt: 19
// RevExt: 10
element('refresh').onclick = (): void => { void perform(refresh); };
element('updates').onclick = (): void => { void perform(async (): Promise<void> => { await invoke({ type: 'check_updates' }); }); };
element('dismiss-key').onclick = (): void => { (element('client-token') as HTMLInputElement).value = ''; element('new-key').hidden = true; };
(element('add-connection') as HTMLFormElement).onsubmit = (event: SubmitEvent): void => {
  event.preventDefault(); const form: HTMLFormElement = event.currentTarget as HTMLFormElement;
  void perform(async (): Promise<void> => {
    const home: string = input(form, 'codex_home').value.trim(); const executable: string = input(form, 'executable').value.trim();
    const connection: Connection = { id: uuidV7(), name: input(form, 'name').value.trim(), method: input(form, 'method').value as Method, ...(input(form, 'method').value === 'codex_cli' && input(form, 'environment').value === 'wsl' ? { environment: 'wsl' as const, wsl_distribution: input(form, 'wsl_distribution').value } : { environment: 'native' as const }), exposed: false, models: [], ...(home ? { codex_home: home } : {}), ...(executable ? { executable } : {}) };
    await invoke({ type: 'save_connection', connection, api_key: input(form, 'api_key').value || undefined }); selectedId = connection.id; form.reset(); (element('provider-dialog') as HTMLDialogElement).close(); await refresh();
  });  // RevExt: 24
};
(element('add-client') as HTMLFormElement).onsubmit = (event: SubmitEvent): void => {
  event.preventDefault(); const form: HTMLFormElement = event.currentTarget as HTMLFormElement;
  void perform(async (): Promise<void> => { const client = await invoke({ type: 'create_client', name: input(form, 'name').value }) as { id: string; token: string }; (element('client-token') as HTMLInputElement).value = client.token; element('new-key').hidden = false; form.reset(); await refresh(); });
};
(element('network') as HTMLFormElement).onsubmit = (event: SubmitEvent): void => {
  event.preventDefault(); const form: HTMLFormElement = event.currentTarget as HTMLFormElement;
  void perform(async (): Promise<void> => { await invoke({ type: 'save_network', network: { browser_origins: (form.elements.namedItem('browser_origins') as HTMLTextAreaElement).value.split(/\r?\n/).map((origin: string): string => origin.trim()).filter(Boolean), require_api_key: input(form, 'require_api_key').checked, port: Number(input(form, 'port').value), host: input(form, 'host').value.trim(), remote_enabled: input(form, 'remote_enabled').checked, trusted_proxy: input(form, 'trusted_proxy').checked, ...(input(form, 'public_endpoint').value ? { public_endpoint: input(form, 'public_endpoint').value.trim() } : {}), ...(input(form, 'tls_certificate').value ? { tls_certificate: input(form, 'tls_certificate').value.trim() } : {}), ...(input(form, 'tls_key').value ? { tls_key: input(form, 'tls_key').value.trim() } : {}) } }); await refresh(); });
};
// RevExt: 11
/** Renderer identity generation uses only browser cryptography; it never receives Node. */
function uuidV7(): string { const bytes: Uint8Array = crypto.getRandomValues(new Uint8Array(16)); let timestamp: number = Date.now(); for (let index: number = 5; index >= 0; index--) { bytes[index] = timestamp % 256; timestamp = Math.floor(timestamp / 256); } bytes[6] = bytes[6]! & 0x0f | 0x70; bytes[8] = bytes[8]! & 0x3f | 0x80; const value: string = [...bytes].map((byte: number): string => byte.toString(16).padStart(2, '0')).join(''); return `${value.slice(0, 8)}-${value.slice(8, 12)}-${value.slice(12, 16)}-${value.slice(16, 20)}-${value.slice(20)}`; }
window.addEventListener('hashchange', navigate);
navigate();
element('add-provider').onclick = openProviderDialog;
element('close-provider').onclick = (): void => { (element('provider-dialog') as HTMLDialogElement).close(); };
void perform(refresh);
// RevExt: 12
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
  const windows: boolean = snapshot?.platform === 'win32' && method === 'codex_cli';
  element('codex-environment-field').hidden = !windows;
  const wsl: boolean = windows && environment.value === 'wsl';
  element('wsl-distribution-field').hidden = !wsl;
  distributions.required = wsl;
  element('api-key-field').hidden = method !== 'openrouter_http';
  element('codex-advanced-field').hidden = method === 'openrouter_http';
}  // RevExt: 20
(element('add-connection') as HTMLFormElement).querySelector('[name="method"]')?.addEventListener('change', updateEnvironmentFields);
(element('add-connection') as HTMLFormElement).querySelector('[name="environment"]')?.addEventListener('change', updateEnvironmentFields);
