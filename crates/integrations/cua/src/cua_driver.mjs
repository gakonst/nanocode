import { AsyncLocalStorage } from 'node:async_hooks';
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath, pathToFileURL } from 'node:url';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import util from 'node:util';
import vm from 'node:vm';

const CATALOG = __CATALOG__;
const VERSION = '__VERSION__';
const env = process.env;
const SETTLE_MS = Number(env.CUA_REPL_SETTLE_MS ?? 400);
const LAUNCH_TIMEOUT_MS = Number(env.CUA_REPL_LAUNCH_TIMEOUT_MS ?? 20000);
const BROWSER_WAIT_MS = Number(env.CUA_REPL_BROWSER_WAIT_MS ?? 10000);
const PAGE_SETTLE_LIMIT_MS = Number(env.CUA_REPL_PAGE_SETTLE_MS ?? 5000);
const PAGE_SETTLE_POLL_MS = 150;
const DEFAULT_TIMEOUT_MS = 30000;
const CHROME_PAGE_STEP_FRACTION = 0.875;
const DRAG_STEPS = 12;
const MAC = process.platform === 'darwin';

const stdoutWrite = process.stdout.write.bind(process.stdout);
process.stdout.write = (...args) => process.stderr.write(...args);
const send = message => stdoutWrite(JSON.stringify(message) + '\n');
const internal = createRequire(path.join(process.cwd(), '[cua_repl]'));
const acorn = internal('internal/deps/acorn/acorn/dist/acorn');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

const DOCS = {
  core: `## Computer use

\`cua_repl\` runs JavaScript in a persistent Node.js session with a \`cua\` object that reads and operates native apps and browser tabs on the user's computer. Prefer purpose-built connectors, APIs or CLIs when they can do the job.

- Native apps go through cua-driver in the background: the app is not raised or focused and the user's pointer and keyboard focus stay where they are.
- Browser tabs go through the nanocode extension in the user's own Chrome (or another Chromium browser), in a tab group of this session's own, opened in the background; the user's own tabs are never read or touched.
- Bindings persist across calls: \`let app = await cua.getApp("Calculator")\` in one call, \`await app.click(12)\` in the next.
- \`cua.getApp\`, \`cua.getTab\` and \`cua.createBrowserTab\` print the target's initial UI state.

## API

\`\`\`typescript
type Vec2 = [x: number, y: number];
type ObservationOptions = { emit?: boolean };
type StateOptions = ObservationOptions & { disableDiffing?: boolean };
type StateAndScreenshot = { state: string; screenshot?: Uint8Array };
type PasteOptions = { format?: "text" | "md" | "html" };
type ClickOptions = { mouseButton?: MouseButton; clickCount?: number };
type SelectTextOptions = { prefix?: string; suffix?: string; selectionType?: SelectionType };
type Direction = "up" | "down" | "left" | "right" | "u" | "d" | "l" | "r";
type SelectionType = "text" | "cursor_before" | "cursor_after";
type MouseButton = "left" | "right" | "middle" | "l" | "r" | "m";

interface Target {
  getAXState(options?: StateOptions): Promise<string>;
  getScreenshot(options?: ObservationOptions): Promise<Uint8Array>;
  getAXStateAndScreenshot(options?: StateOptions): Promise<StateAndScreenshot>;
  paste(text: string, options?: PasteOptions): Promise<void>;
  click(target: number | Vec2, options?: ClickOptions): Promise<void>;
  drag(from: Vec2, to: Vec2): Promise<void>;
  pressKey(key: string): Promise<void>;
  scroll(target: number | Vec2, direction: Direction, pages?: number): Promise<void>;
  selectText(elementIndex: number, text: string, options?: SelectTextOptions): Promise<void>;
  setValue(elementIndex: number, value: string): Promise<void>;
  typeText(text: string): Promise<void>;
  performSecondaryAction(elementIndex: number, action: string): Promise<void>;
}

type AppInfo = { id: string; displayName?: string; lastUsedDate?: string; isRunning?: boolean };
interface App extends Target {}

type BrowserInfo = { id: string; name?: string; family?: string; type?: "extension" };
type TabInfo = { id: string; providerTabId?: string; browserId: string; title?: string; url?: string };
interface Browser { readonly browserId: string; documentation(): Promise<string> }
type State = { apps: AppInfo[]; browsers: (BrowserInfo & { tabs: TabInfo[] })[] };
type CreateBrowserTabOptions = { sessionName?: string };

interface Tab extends Target {
  readonly id: string;
  goto(url: string): Promise<void>;
  back(): Promise<void>;
  forward(): Promise<void>;
  reload(): Promise<void>;
  close(): Promise<void>;
  markDeliverable(): Promise<void>;
  markHandoff(): Promise<void>;
}

declare const cua: {
  getState(options?: ObservationOptions): Promise<State>;
  getApp(app: string): Promise<App>;
  listApps(options?: ObservationOptions): Promise<AppInfo[]>;
  getBrowser(options?: { id?: string; url?: string }): Promise<Browser>;
  createBrowserTab(browserId: string, url?: string, options?: CreateBrowserTabOptions): Promise<Tab>;
  getTab(id: string, options?: { browser?: string }): Promise<Tab>;
  listBrowsers(options?: ObservationOptions): Promise<BrowserInfo[]>;
  listTabs(options?: { browser?: string } & ObservationOptions): Promise<TabInfo[]>;
  rewriteDocumentation(): Promise<void>;
  driver: { call(tool: string, args?: object): Promise<object>; tools(): Promise<string[]> };
};
\`\`\`

## Workflow

After one or more actions, call \`getAXState()\` before deciding what to do next, and use the element indexes of that latest state. By default it returns only what changed since the previous state of that target (added, changed and removed rows); pass \`{ disableDiffing: true }\` for the whole tree. Batch deterministic actions and the \`getAXState()\` that checks them into one call:

\`\`\`typescript
await app.click(42);
await app.setValue(17, "openai.com");
await app.pressKey("Return");
await app.typeText("hello");
await app.scroll(42, "down", 1);
await app.scroll([640, 480], "down", 1);
await app.performSecondaryAction(42, "AXShowMenu");
await app.getAXState();
\`\`\`

Observation methods wait for the UI to settle before reading it, so do not sleep before them. Once the requested result is visible in the state, stop and answer.

## Output

- \`nodeRepl.write(value)\` adds text (non-strings are inspected); \`await nodeRepl.emitImage(image)\` adds an image (PNG, JPEG or WebP bytes, a data: or file: URL, or \`{ bytes, mimeType }\`).
- \`getAXState()\`, \`getScreenshot()\`, \`getAXStateAndScreenshot()\`, \`cua.getState()\`, \`cua.getApp()\`, \`cua.getTab()\`, \`cua.createBrowserTab()\`, \`cua.listApps()\`, \`cua.listBrowsers()\` and \`cua.listTabs()\` print their result themselves; pass \`{ emit: false }\` to observation and listing methods to keep it out. A cell's final expression is not printed.

## Notes

- Prefer element indexes over coordinates. Coordinates are pixels of the target's latest screenshot. Use screenshots when the accessibility tree misses what you need.
- \`cua.getApp()\` takes a display name, bundle identifier or app path and launches a missing app in the background. If a display name does not resolve, retry with the bundle identifier from \`cua.listApps()\`.
- \`performSecondaryAction()\` invokes an action the element lists (such as AXShowMenu, AXPick, AXConfirm, AXCancel, AXOpen); do not guess names.
- \`pressKey()\` takes xdotool-style keys and chords: \`"Return"\`, \`"Tab"\`, \`"Up"\`, \`"Page_Down"\`, \`"super+c"\` (super is Command on macOS), \`"ctrl+shift+z"\`, \`"KP_0"\`.
- Native \`paste\` puts the text on the system clipboard, pastes it and restores the previous clipboard text.
- \`selectText()\` is supported in browser tabs; native apps have no text-selection route in cua-driver, so click into the field and use keys instead.
- \`cua.driver.call(tool, args)\` calls a cua-driver MCP tool directly, for what the API above does not cover.

Persist until the request is done end to end and the latest state visibly shows the result, or explain the concrete blocker.`,
  browser: `## Browser tabs

\`chrome\` is the user's own Chromium browser (Chrome, Edge, Brave, ...), driven through the nanocode extension. \`cua.createBrowserTab("chrome", url, { sessionName: "🔎 Task" })\` opens a background tab in a tab group named after \`sessionName\` (pass a short, emoji-prefixed name when starting a task). The tab never takes focus, and the user's own tabs cannot be read or operated. Only tabs this session opened are reachable with \`cua.getTab()\` and \`cua.listTabs()\`.

- A tab's state is its page's accessibility tree. \`[N]\` indexes stay with the same element across later states of that tab.
- Coordinates are CSS pixels of the tab's viewport screenshot.
- \`paste()\` inserts the text where the focus is; \`md\` inserts the Markdown source and \`html\` inserts HTML.
- The tabs a turn used close when the turn ends. Call \`tab.markDeliverable()\` (a result the user should see) or \`tab.markHandoff()\` (the user continues there) to leave them open.`,
};

const format = value => typeof value === 'string' ? value : util.inspect(value, { depth: 6, breakLength: 120, maxArrayLength: 200, maxStringLength: 20000 });
const cells = new AsyncLocalStorage();
let currentMeta = {};
const writeText = text => {
  const cell = cells.getStore();
  if (cell && !cell.done) cell.items.push({ type: 'text', text });
};
const sniff = bytes => {
  if (bytes.length >= 8 && bytes.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) return 'image/png';
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) return 'image/jpeg';
  if (bytes.length >= 12 && bytes.subarray(0, 4).toString('latin1') === 'RIFF' && bytes.subarray(8, 12).toString('latin1') === 'WEBP') return 'image/webp';
  return null;
};
const imageBytes = async image => {
  if (typeof image === 'string') {
    if (image.startsWith('data:')) {
      const comma = image.indexOf(',');
      const head = image.slice(5, comma);
      return Buffer.from(image.slice(comma + 1), head.includes(';base64') ? 'base64' : 'utf8');
    }
    return fs.promises.readFile(image.startsWith('file:') ? fileURLToPath(image) : image);
  }
  if (image instanceof ArrayBuffer) return Buffer.from(image);
  if (ArrayBuffer.isView(image)) return Buffer.from(image.buffer, image.byteOffset, image.byteLength);
  if (image && typeof image === 'object') {
    if (image.bytes !== undefined) return imageBytes(image.bytes);
    if (typeof image.data === 'string') return Buffer.from(image.data, 'base64');
    if (typeof image.url === 'string') return imageBytes(image.url);
  }
  throw new TypeError('emitImage expects PNG, JPEG or WebP bytes, a data: or file: URL, or { bytes, mimeType }');
};
const emitImageItem = async image => {
  const bytes = await imageBytes(image);
  const mimeType = sniff(bytes);
  if (!mimeType) throw new TypeError('emitImage accepts PNG, JPEG or WebP images only');
  const cell = cells.getStore();
  if (cell && !cell.done) cell.items.push({ type: 'image', data: bytes.toString('base64'), mimeType });
};
const callIdentity = () => {
  const turn = currentMeta?.['x-codex-turn-metadata'] ?? {};
  return { session: String(turn.session_id ?? 'cua_repl'), turn: turn.turn_id === undefined ? null : String(turn.turn_id) };
};
const nodeRepl = Object.freeze({
  cwd: process.cwd(),
  homeDir: os.homedir(),
  tmpDir: os.tmpdir(),
  env: Object.freeze({ ...env }),
  get requestMeta() { return currentMeta; },
  write(value) { writeText(format(value)); },
  async emitImage(image) { await emitImageItem(image); },
});

const moduleDirs = [];
const moduleRequire = specifier => {
  for (const dir of moduleDirs) {
    try { return createRequire(path.join(dir, '[cua_repl]'))(specifier); } catch (error) { if (error.code !== 'MODULE_NOT_FOUND') throw error; }
  }
  return createRequire(path.join(process.cwd(), '[cua_repl]'))(specifier);
};
const moduleImport = async specifier => {
  if (specifier.startsWith('node:') || specifier.startsWith('file:') || specifier.startsWith('data:')) return import(specifier);
  if (path.isAbsolute(specifier)) return import(pathToFileURL(specifier).href);
  for (const dir of [...moduleDirs, process.cwd()]) {
    try { return await import(pathToFileURL(createRequire(path.join(dir, '[cua_repl]')).resolve(specifier)).href); } catch (error) { if (error.code !== 'MODULE_NOT_FOUND') throw error; }
  }
  return import(specifier);
};

const capture = (...values) => writeText(values.map(format).join(' '));
const captureConsole = Object.freeze({ log: capture, info: capture, warn: capture, error: capture, debug: capture, trace: capture, dir: value => capture(value), table: value => capture(value) });

const transform = code => {
  const ast = acorn.parse(code, { ecmaVersion: 'latest', sourceType: 'module', allowAwaitOutsideFunction: true, allowReturnOutsideFunction: true, allowImportExportEverywhere: true, allowHashBang: true });
  const edits = [];
  const hoisted = [];
  let imports = 0;
  const walk = node => {
    if (!node || typeof node.type !== 'string') return;
    if (node.type === 'ImportExpression') edits.push({ start: node.start, end: node.start + 6, text: '__cua_import' });
    for (const key of Object.keys(node)) {
      const value = node[key];
      if (Array.isArray(value)) value.forEach(walk);
      else if (value && typeof value.type === 'string') walk(value);
    }
  };
  const declaration = node => {
    if (node.type === 'VariableDeclaration') {
      const first = node.declarations[0];
      const last = node.declarations[node.declarations.length - 1];
      edits.push({ start: node.start, end: first.start, text: ';(' });
      for (const declarator of node.declarations) {
        if (!declarator.init) edits.push({ start: declarator.id.end, end: declarator.id.end, text: ' = undefined' });
        walk(declarator.init);
      }
      edits.push({ start: last.end, end: last.end, text: ')' });
      return true;
    }
    if (node.type === 'FunctionDeclaration') {
      hoisted.push(`globalThis[${JSON.stringify(node.id.name)}] = ${node.id.name};`);
      walk(node.body);
      return true;
    }
    if (node.type === 'ClassDeclaration') {
      edits.push({ start: node.start, end: node.start, text: `;(${node.id.name} = ` });
      edits.push({ start: node.end, end: node.end, text: ');' });
      walk(node.body);
      if (node.superClass) walk(node.superClass);
      return true;
    }
    return false;
  };
  for (const node of ast.body) {
    if (node.type === 'ImportDeclaration') {
      const binding = `__cua_module_${imports++}`;
      const parts = [`const ${binding} = await __cua_import(${JSON.stringify(node.source.value)});`];
      for (const specifier of node.specifiers) {
        if (specifier.type === 'ImportDefaultSpecifier') parts.push(`${specifier.local.name} = ${binding}.default;`);
        else if (specifier.type === 'ImportNamespaceSpecifier') parts.push(`${specifier.local.name} = ${binding};`);
        else parts.push(`${specifier.local.name} = ${binding}[${JSON.stringify(specifier.imported.name ?? specifier.imported.value)}];`);
      }
      edits.push({ start: node.start, end: node.end, text: parts.join(' ') });
      continue;
    }
    if (node.type === 'ExportNamedDeclaration' || node.type === 'ExportDefaultDeclaration' || node.type === 'ExportAllDeclaration') {
      if (node.type === 'ExportNamedDeclaration' && node.declaration) {
        edits.push({ start: node.start, end: node.declaration.start, text: '' });
        declaration(node.declaration);
        continue;
      }
      throw new SyntaxError('export is not supported in cua_repl cells');
    }
    if (!declaration(node)) walk(node);
  }
  edits.sort((a, b) => b.start - a.start || b.end - a.end);
  let body = code;
  for (const edit of edits) body = body.slice(0, edit.start) + edit.text + body.slice(edit.end);
  return `(async () => {${hoisted.join('')}\n${body}\n})()`;
};

class Driver {
  #child = null;
  #ready = null;
  #pending = new Map();
  #next = 0;
  #tools = new Map();
  #stderr = '';
  #executable = env.CUA_DRIVER_PATH || 'cua-driver';

  start() {
    if (!this.#ready) {
      this.#ready = this.#launch().catch(error => { this.#ready = null; this.#stop(); throw error; });
    }
    return this.#ready;
  }

  async #launch() {
    const child = spawn(this.#executable, ['mcp'], { stdio: ['pipe', 'pipe', 'pipe'], env: process.env });
    this.#child = child;
    child.on('error', error => this.#fail(child, new Error(`Cannot start cua-driver (${this.#executable}): ${error.message}`)));
    child.on('exit', (code, signal) => this.#fail(child, new Error(`cua-driver exited (${signal ?? code})${this.#stderr ? ': ' + this.#stderr.trim().split('\n').slice(-3).join(' ') : ''}`)));
    child.stderr.setEncoding('utf8');
    child.stderr.on('data', data => { this.#stderr = (this.#stderr + data).slice(-4000); });
    child.stdin.on('error', () => {});
    readline.createInterface({ input: child.stdout }).on('line', line => this.#receive(line));
    await this.#rpc('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'cua_repl', version: VERSION } });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    const listed = await this.#rpc('tools/list', {});
    for (const tool of listed.tools ?? []) this.#tools.set(tool.name, tool);
  }

  #fail(child, error) {
    if (this.#child !== child) return;
    this.#child = null;
    this.#ready = null;
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
  }

  #stop() {
    const child = this.#child;
    this.#child = null;
    if (child) child.kill();
  }

  close() { this.#stop(); }

  #receive(line) {
    let message;
    try { message = JSON.parse(line); } catch { return; }
    if (message.method !== undefined) {
      if (message.id !== undefined) this.#child?.stdin.write(JSON.stringify({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: 'cua_repl does not serve requests' } }) + '\n');
      return;
    }
    const pending = this.#pending.get(message.id);
    if (!pending) return;
    this.#pending.delete(message.id);
    if (message.error) pending.reject(new Error(`cua-driver: ${message.error.message ?? 'request failed'}`));
    else pending.resolve(message.result);
  }

  #rpc(method, params) {
    const child = this.#child;
    if (!child) return Promise.reject(new Error('cua-driver is not running'));
    const id = ++this.#next;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
    });
  }

  async tools() {
    await this.start();
    return [...this.#tools.keys()];
  }

  async call(name, args = {}) {
    await this.start();
    const tool = this.#tools.get(name);
    if (!tool) throw new Error(`cua-driver has no ${name} tool`);
    const clean = Object.fromEntries(Object.entries(args).filter(([, value]) => value !== undefined));
    if (tool.inputSchema?.properties?.session && clean.session === undefined) clean.session = sessionLabel();
    const result = await this.#rpc('tools/call', { name, arguments: clean });
    if (result?.isError) {
      const text = (result.content ?? []).filter(item => item.type === 'text').map(item => item.text).join('\n').trim();
      const error = new Error(text || `cua-driver ${name} failed`);
      error.code = result.structuredContent?.code;
      error.detail = result.structuredContent;
      throw error;
    }
    return result;
  }
}

const sessionLabel = () => 'cua-' + callIdentity().session.replace(/[^A-Za-z0-9]/g, '').slice(0, 12) + '-' + process.pid;
const driver = new Driver();
const textOf = result => (result.content ?? []).filter(item => item.type === 'text').map(item => item.text).join('\n');
const imageOf = result => {
  const image = (result.content ?? []).find(item => item.type === 'image');
  return image ? Buffer.from(image.data, 'base64') : undefined;
};
const BUTTONS = { l: 'left', left: 'left', r: 'right', right: 'right', m: 'middle', middle: 'middle' };
const DIRECTIONS = { u: 'up', up: 'up', d: 'down', down: 'down', l: 'left', left: 'left', r: 'right', right: 'right' };
const button = value => {
  const resolved = BUTTONS[String(value ?? 'left').toLowerCase()];
  if (!resolved) throw new TypeError(`Unknown mouse button ${value}`);
  return resolved;
};
const direction = value => {
  const resolved = DIRECTIONS[String(value).toLowerCase()];
  if (!resolved) throw new TypeError(`Unknown scroll direction ${value}`);
  return resolved;
};
const isPoint = value => Array.isArray(value) && value.length === 2 && value.every(Number.isFinite);
const index = value => {
  if (!Number.isInteger(value) || value < 0) throw new TypeError(`Expected an element index from the latest state, got ${format(value)}`);
  return value;
};

const KEY_ALIASES = {
  return: 'return', enter: 'return', kp_enter: 'return', tab: 'tab', iso_left_tab: 'tab', escape: 'escape', esc: 'escape',
  backspace: 'backspace', delete: 'delete', del: 'delete', insert: 'insert', home: 'home', end: 'end',
  page_up: 'pageup', prior: 'pageup', pageup: 'pageup', page_down: 'pagedown', next: 'pagedown', pagedown: 'pagedown',
  up: 'up', down: 'down', left: 'left', right: 'right', space: 'space', minus: '-', plus: '+', equal: '=', comma: ',', period: '.', slash: '/',
};
const MODIFIERS = { super: 'cmd', cmd: 'cmd', command: 'cmd', meta: 'cmd', win: 'cmd', ctrl: 'ctrl', control: 'ctrl', alt: 'alt', option: 'alt', opt: 'alt', shift: 'shift' };
const parseChord = spec => {
  const text = String(spec);
  const parts = text.length > 1 ? text.split('+') : [text];
  const key = parts.pop() || '+';
  const modifiers = parts.map(part => {
    const modifier = MODIFIERS[part.trim().toLowerCase()];
    if (!modifier) throw new TypeError(`Unknown modifier ${part} in ${text}`);
    return modifier;
  });
  return { key: key.trim() || key, modifiers };
};
const nativeKey = spec => {
  const { key, modifiers } = parseChord(spec);
  const lower = key.toLowerCase();
  let name = KEY_ALIASES[lower];
  if (!name && /^kp_\d$/.test(lower)) name = lower.slice(3);
  if (!name && /^f([1-9]|1[0-2])$/.test(lower)) name = lower;
  if (!name && [...key].length === 1) name = key;
  if (!name) throw new TypeError(`Unknown key ${key}`);
  const mods = modifiers.map(modifier => modifier === 'cmd' && !MAC ? 'super' : modifier);
  return { key: name, modifiers: mods };
};
const AX_ACTIONS = { axpress: 'press', press: 'press', axshowmenu: 'show_menu', showmenu: 'show_menu', axpick: 'pick', pick: 'pick', axconfirm: 'confirm', confirm: 'confirm', axcancel: 'cancel', cancel: 'cancel', axopen: 'open', open: 'open', axshowalternateui: 'show_alternate_ui', showalternateui: 'show_alternate_ui', axshowdefaultui: 'show_default_ui', showdefaultui: 'show_default_ui' };

const emitState = (text, options) => { if (options?.emit !== false && text) writeText(text); };
const emitScreenshot = async (bytes, options) => { if (options?.emit !== false && bytes) await emitImageItem(bytes); };

class NativeApp {
  #info;
  #window = null;
  #snapshot = null;
  #lastAction = 0;

  constructor(info) {
    this.#info = info;
    Object.defineProperty(this, 'id', { value: info.bundle_id ?? info.name, enumerable: true });
    Object.defineProperty(this, 'displayName', { value: info.name, enumerable: true });
  }

  get pid() { return this.#info.pid; }

  async #resolveWindow() {
    const result = await driver.call('list_windows', { pid: this.#info.pid });
    const windows = result.structuredContent?.windows ?? [];
    if (!windows.length) throw new Error(`${this.#info.name} has no window to read; open one first or check that it is running`);
    const rank = window => [window.is_on_screen ? 1 : 0, window.title ? 1 : 0, Number.isInteger(window.z_index) ? window.z_index : -1];
    windows.sort((a, b) => {
      const [x, y] = [rank(a), rank(b)];
      return y[0] - x[0] || y[1] - x[1] || y[2] - x[2];
    });
    const chosen = windows.find(window => window.window_id === this.#window?.window_id && window.is_on_screen) ?? windows[0];
    this.#window = chosen;
    return chosen;
  }

  async #settle() {
    const wait = this.#lastAction + SETTLE_MS - Date.now();
    if (wait > 0) await sleep(wait);
  }

  async read({ tree = true, screenshot = false, diff = true } = {}) {
    await this.#settle();
    const window = await this.#resolveWindow();
    const args = { pid: this.#info.pid, window_id: window.window_id, include_screenshot: screenshot, include_accessibility_tree: tree };
    if (!tree) args.display_only = true;
    else if (diff && this.#snapshot) args.since = 'latest';
    const result = await driver.call('get_window_state', args);
    const state = result.structuredContent ?? {};
    let text;
    if (tree) {
      if (state.snapshot_id) this.#snapshot = state.snapshot_id;
      const title = state.window_title ?? window.title ?? '';
      const header = `${this.#info.name} (pid ${this.#info.pid})${title ? ` — window "${title}"` : ''}`;
      let body;
      if (state.since_status === 'no_change') {
        body = 'No accessibility-tree change since the previous state.';
      } else if (state.since_status === 'diff' && typeof state.tree_diff === 'string') {
        body = state.tree_diff.replace(/ Use element_tokens [^\n]*?\.(?= |\n|$)/, '').replace(/ - ids are from [^\n]*$/m, '').trim();
        if (!body) body = 'No accessibility-tree change since the previous state.';
      } else if (typeof state.tree_markdown === 'string') {
        body = state.tree_markdown.replace(/\n+$/, '');
      } else {
        body = textOf(result).split('\n').filter(line => !/^element_token for row|^Invalidated snapshots/.test(line)).join('\n').trim();
      }
      const notes = [];
      if (state.truncated || (state.elements_complete === false && state.returned_element_count < state.total_element_count)) notes.push(`(tree cut at ${state.returned_element_count ?? '?'} of ${state.total_element_count ?? '?'} elements)`);
      if (state.degraded_reason) notes.push(`(${state.degraded_reason})`);
      text = [header, body, ...notes].join('\n');
    }
    return { text, screenshot: imageOf(result) };
  }

  #token(value) {
    if (!this.#snapshot) throw new Error('Read the app first with getAXState(); element indexes come from its latest state');
    return `${this.#snapshot}:${index(value)}`;
  }

  async #act(name, args) {
    const window = this.#window ?? await this.#resolveWindow();
    try {
      return await driver.call(name, { ...args, ...(args.element_token ? {} : { pid: this.#info.pid, window_id: window.window_id }) });
    } finally {
      this.#lastAction = Date.now();
    }
  }

  async getAXState(options) {
    const { text } = await this.read({ diff: options?.disableDiffing !== true });
    emitState(text, options);
    return text;
  }

  async getScreenshot(options) {
    const { screenshot } = await this.read({ tree: false, screenshot: true });
    if (!screenshot) throw new Error(`Screenshot unavailable for ${this.#info.name}; grant Screen Recording to cua-driver`);
    await emitScreenshot(screenshot, options);
    return screenshot;
  }

  async getAXStateAndScreenshot(options) {
    const { text, screenshot } = await this.read({ screenshot: true, diff: options?.disableDiffing !== true });
    emitState(text, options);
    await emitScreenshot(screenshot, options);
    return screenshot ? { state: text, screenshot } : { state: text };
  }

  async click(target, options) {
    const which = button(options?.mouseButton);
    const count = options?.clickCount ?? 1;
    if (isPoint(target)) {
      await this.#act('click', { x: target[0], y: target[1], button: which, count });
      return;
    }
    const element_token = this.#token(target);
    if (count === 2 && which === 'left') await this.#act('double_click', { element_token });
    else if (which === 'right') await this.#act('right_click', { element_token });
    else await this.#act('click', { element_token, button: which === 'left' ? undefined : which, count: count === 1 ? undefined : count });
  }

  async drag(from, to) {
    if (!isPoint(from) || !isPoint(to)) throw new TypeError('drag expects two [x, y] points');
    await this.#act('drag', { from_x: from[0], from_y: from[1], to_x: to[0], to_y: to[1] });
  }

  async pressKey(key) {
    const chord = nativeKey(key);
    await this.#act('press_key', { key: chord.key, modifiers: chord.modifiers.length ? chord.modifiers : undefined });
  }

  async scroll(target, dir, pages = 1) {
    const where = isPoint(target) ? { x: target[0], y: target[1] } : { element_token: this.#token(target) };
    await this.#act('scroll', { ...where, direction: direction(dir), by: 'page', amount: Math.max(1, Math.round(pages)) });
  }

  async selectText() {
    throw new Error('selectText is not available for native apps through cua-driver: click into the field, then use pressKey (for example "super+a", "End") or setValue');
  }

  async setValue(elementIndex, value) {
    await this.#act('set_value', { element_token: this.#token(elementIndex), value: String(value) });
  }

  async typeText(text) {
    await this.#act('type_text', { text: String(text) });
  }

  async performSecondaryAction(elementIndex, action) {
    const mapped = AX_ACTIONS[String(action).toLowerCase().replace(/[^a-z]/g, '')] ?? String(action);
    await this.#act('click', { element_token: this.#token(elementIndex), action: mapped });
  }

  async paste(text, options) {
    const formatName = options?.format ?? 'text';
    if (!['text', 'md', 'html'].includes(formatName)) throw new TypeError(`Unknown paste format ${formatName}`);
    let previous;
    try { previous = (await driver.call('clipboard_read', { include_text: true })).structuredContent?.text; } catch {}
    await driver.call('clipboard_write', { text: String(text) });
    try {
      await this.#act('press_key', { key: 'v', modifiers: [MAC ? 'cmd' : 'ctrl'] });
      await sleep(SETTLE_MS);
    } finally {
      if (typeof previous === 'string') await driver.call('clipboard_write', { text: previous }).catch(() => {});
    }
  }
}

const listed = app => !!(app.bundle_id || app.launch_path);
const appRecord = app => ({ id: app.bundle_id ?? app.launch_path ?? app.name, displayName: app.name, ...(app.last_used ? { lastUsedDate: app.last_used } : {}), isRunning: !!app.running });
const listDriverApps = async () => (await driver.call('list_apps', {})).structuredContent?.apps ?? [];
const findApp = (apps, identifier) => {
  const wanted = String(identifier).trim();
  const lower = wanted.toLowerCase();
  const base = path.basename(wanted).replace(/\.app$/i, '').toLowerCase();
  const byRunning = list => list.sort((a, b) => (b.running ? 1 : 0) - (a.running ? 1 : 0));
  const exact = byRunning(apps.filter(app => app.bundle_id === wanted || app.launch_path === wanted || app.name?.toLowerCase() === lower || (wanted.includes('/') && app.name?.toLowerCase() === base)));
  if (exact.length) return exact[0];
  const partial = byRunning(apps.filter(app => app.name?.toLowerCase().includes(lower) || app.bundle_id?.toLowerCase() === lower));
  const names = new Set(partial.map(app => app.bundle_id ?? app.name));
  if (names.size === 1) return partial[0];
  if (names.size > 1) throw new Error(`"${wanted}" matches several apps: ${[...names].slice(0, 8).join(', ')}; pass a bundle identifier`);
  return null;
};
const launch = async (app, identifier) => {
  const args = app?.bundle_id && MAC ? { bundle_id: app.bundle_id } : app?.launch_path ? { launch_path: app.launch_path } : { name: app?.name ?? identifier };
  const launched = await driver.call('launch_app', args);
  const deadline = Date.now() + LAUNCH_TIMEOUT_MS;
  let pid = launched.structuredContent?.pid;
  while (Date.now() < deadline) {
    if (!pid) pid = findApp((await listDriverApps()).filter(entry => entry.running), app?.bundle_id ?? app?.name ?? identifier)?.pid;
    if (pid) {
      const windows = (await driver.call('list_windows', { pid })).structuredContent?.windows ?? [];
      if (windows.length) return pid;
    }
    await sleep(250);
  }
  throw new Error(`${app?.name ?? identifier} did not open a window within ${LAUNCH_TIMEOUT_MS} ms`);
};

class Relay {
  #path;
  #server = null;
  #listening = null;
  #socket = null;
  #buffer = '';
  #pending = new Map();
  #next = 0;
  #waiters = new Set();

  constructor(socketPath) { this.#path = socketPath; }
  get available() { return !!this.#path; }

  listen() {
    if (!this.#path) return Promise.reject(new Error('No browser is configured: set CUA_REPL_BROWSER_SOCKET (nanocode cua install)'));
    if (!this.#listening) this.#listening = this.#bind().catch(error => { this.#listening = null; throw error; });
    return this.#listening;
  }

  async #bind() {
    fs.mkdirSync(path.dirname(this.#path), { recursive: true, mode: 0o700 });
    if (fs.existsSync(this.#path)) {
      const live = await new Promise(resolve => {
        const probe = net.connect(this.#path);
        probe.once('connect', () => { probe.destroy(); resolve(true); });
        probe.once('error', () => resolve(false));
      });
      if (live) throw new Error(`Another cua_repl session is serving the browser on ${this.#path}; only one session can drive Chrome at a time`);
      fs.unlinkSync(this.#path);
    }
    const server = net.createServer(socket => this.#attach(socket));
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(this.#path, resolve); });
    fs.chmodSync(this.#path, 0o600);
    this.#server = server;
  }

  #attach(socket) {
    if (this.#socket) { socket.destroy(); return; }
    this.#socket = socket;
    this.#buffer = '';
    socket.setEncoding('utf8');
    socket.write(JSON.stringify({ type: 'hello' }) + '\n');
    socket.on('data', data => {
      this.#buffer += data;
      let newline;
      while ((newline = this.#buffer.indexOf('\n')) >= 0) {
        const line = this.#buffer.slice(0, newline);
        this.#buffer = this.#buffer.slice(newline + 1);
        let message;
        try { message = JSON.parse(line); } catch { continue; }
        const pending = this.#pending.get(message.id);
        if (message.type !== 'result' || !pending) continue;
        this.#pending.delete(message.id);
        if (message.ok) pending.resolve(message.result);
        else pending.reject(new Error(message.error ?? 'browser step failed'));
      }
    });
    const closed = () => {
      if (this.#socket !== socket) return;
      this.#socket = null;
      for (const pending of this.#pending.values()) pending.reject(new Error('The browser disconnected during this step; whether it took effect is unknown'));
      this.#pending.clear();
    };
    socket.on('close', closed);
    socket.on('error', closed);
    for (const waiter of this.#waiters) waiter();
    this.#waiters.clear();
  }

  async connected() {
    await this.listen();
    if (this.#socket) return;
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.#waiters.delete(done); reject(new Error(`Chrome is not connected: open Chrome with the nanocode extension loaded (nanocode cua install sets it up, then Load unpacked in chrome://extensions); it connects through ${this.#path}`)); }, BROWSER_WAIT_MS);
      const done = () => { clearTimeout(timer); resolve(); };
      this.#waiters.add(done);
    });
  }

  async step(input) {
    await this.connected();
    const { session, turn } = callIdentity();
    const id = ++this.#next;
    const socket = this.#socket;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      socket.write(JSON.stringify({ type: 'call', id, session, turn, input }) + '\n');
    });
  }

  turnEnded(session, turn) {
    this.#socket?.write(JSON.stringify({ type: 'turn_ended', session, turn }) + '\n');
  }

  async close() {
    const socket = this.#socket;
    if (socket) await new Promise(resolve => { socket.end(resolve); setTimeout(resolve, 1000); });
    socket?.destroy();
    this.#server?.close();
    if (this.#server && this.#path) { try { fs.unlinkSync(this.#path); } catch {} }
  }
}

const relay = new Relay(env.CUA_REPL_BROWSER_SOCKET || '');
const BROWSERS = {
  chrome: { id: 'chrome', name: 'Chrome', family: 'chromium', type: 'extension' },
};
const BROWSER_ALIASES = { chrome: 'chrome', edge: 'chrome', brave: 'chrome', chromium: 'chrome', arc: 'chrome', vivaldi: 'chrome' };
const tabs = new Map();
const closeTabs = async selected => {
  for (const [id, tab] of [...tabs]) {
    if (!selected(tab)) continue;
    tabs.delete(id);
    if (!tab.kept) await tab.close().catch(() => {});
  }
};

const READ_ONLY_CDP = new Set(['Runtime.evaluate', 'Accessibility.getFullAXTree', 'Page.getLayoutMetrics', 'Page.captureScreenshot', 'DOM.getContentQuads', 'Page.getNavigationHistory']);
const SETTLE_PROBE = "(()=>{const w=window;if(!w.__cuaReplMutations){const counter={n:0};Object.defineProperty(w,'__cuaReplMutations',{value:counter});new MutationObserver(()=>{counter.n++}).observe(document,{subtree:true,childList:true,attributes:true,characterData:true})}return document.readyState+':'+location.href+':'+w.__cuaReplMutations.n})()";
const TRANSPARENT = new Set(['none', 'generic', 'GenericContainer', 'InlineTextBox', 'LineBreak', 'RootWebArea', 'WebArea', 'Section', 'presentation', 'Pre', 'Div', 'paragraph', 'LabelText']);
const ACTIONABLE = new Set(['button', 'link', 'textbox', 'searchbox', 'combobox', 'checkbox', 'radio', 'switch', 'slider', 'spinbutton', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'option', 'tab', 'treeitem', 'listbox', 'textField', 'TextField']);
const LEAVES = new Set(['heading', 'img', 'image', 'cell', 'columnheader', 'rowheader', 'label']);
const FLAGS = ['checked', 'disabled', 'expanded', 'selected', 'pressed', 'required', 'focused'];
const axText = value => typeof value?.value === 'string' ? value.value.split(/\s+/).filter(Boolean).join(' ') : typeof value?.value === 'number' ? String(value.value) : '';
const clip = (text, limit) => [...text].slice(0, limit).join('');

const KEY_CODES = {
  return: ['Enter', 'Enter', 13, '\r'], tab: ['Tab', 'Tab', 9, ''], escape: ['Escape', 'Escape', 27, ''], backspace: ['Backspace', 'Backspace', 8, ''],
  delete: ['Delete', 'Delete', 46, ''], insert: ['Insert', 'Insert', 45, ''], home: ['Home', 'Home', 36, ''], end: ['End', 'End', 35, ''],
  pageup: ['PageUp', 'PageUp', 33, ''], pagedown: ['PageDown', 'PageDown', 34, ''], up: ['ArrowUp', 'ArrowUp', 38, ''], down: ['ArrowDown', 'ArrowDown', 40, ''],
  left: ['ArrowLeft', 'ArrowLeft', 37, ''], right: ['ArrowRight', 'ArrowRight', 39, ''], space: [' ', 'Space', 32, ' '],
};
const CDP_MODIFIERS = { alt: 1, ctrl: 2, cmd: 4, shift: 8 };
const MAC_EDITING = { a: 'selectAll', c: 'copy', v: 'paste', x: 'cut', z: 'undo' };
const keyEvents = spec => {
  const { key, modifiers } = parseChord(spec);
  const lower = key.toLowerCase();
  const name = KEY_ALIASES[lower] ?? lower;
  let entry = KEY_CODES[name];
  if (!entry && /^kp_\d$/.test(lower)) entry = [lower.slice(3), `Numpad${lower.slice(3)}`, 96 + Number(lower.slice(3)), lower.slice(3)];
  if (!entry && /^f([1-9]|1[0-2])$/.test(lower)) entry = [key.toUpperCase(), key.toUpperCase(), 111 + Number(lower.slice(1)), ''];
  if (!entry && [...(KEY_ALIASES[lower] ?? key)].length === 1) {
    const char = KEY_ALIASES[lower] ?? key;
    const upper = char.toUpperCase();
    const letter = /^[A-Z]$/.test(upper);
    const digit = /^\d$/.test(char);
    entry = [char, letter ? `Key${upper}` : digit ? `Digit${char}` : '', letter || digit ? upper.charCodeAt(0) : 0, char];
  }
  if (!entry) throw new TypeError(`Unknown key ${key}`);
  const mask = modifiers.reduce((sum, modifier) => sum | CDP_MODIFIERS[modifier], 0);
  const [keyName, code, keyCode, typed] = entry;
  const text = mask & ~8 ? '' : typed;
  const down = { type: text ? 'keyDown' : 'rawKeyDown', key: keyName, code, windowsVirtualKeyCode: keyCode, modifiers: mask, text, unmodifiedText: text };
  if (MAC && mask & 4) {
    const command = MAC_EDITING[keyName.toLowerCase()];
    if (command) down.commands = [keyName.toLowerCase() === 'z' && mask & 8 ? 'redo' : command];
  }
  return [['Input.dispatchKeyEvent', down], ['Input.dispatchKeyEvent', { type: 'keyUp', key: keyName, code, windowsVirtualKeyCode: keyCode, modifiers: mask }]];
};

class BrowserTab {
  #browser;
  #tab;
  #refs = new Map();
  #stable = new Map();
  #nextIndex = 1;
  #lines = null;
  #turn = null;
  #session = null;
  #keep = false;
  #dirty = true;

  constructor(browserId, tabId, info) {
    this.#browser = browserId;
    this.#tab = tabId;
    Object.defineProperty(this, 'id', { value: String(tabId), enumerable: true });
    Object.defineProperty(this, 'providerTabId', { value: String(tabId), enumerable: true });
    Object.defineProperty(this, 'browserId', { value: browserId, enumerable: true });
    this.url = info?.url;
    this.title = info?.title;
    this.#turn = callIdentity().turn;
    this.#session = callIdentity().session;
  }

  get turn() { return this.#turn; }
  get session() { return this.#session; }
  get kept() { return this.#keep; }
  info() { return { id: this.id, providerTabId: this.providerTabId, browserId: this.#browser, ...(this.title ? { title: this.title } : {}), ...(this.url ? { url: this.url } : {}) }; }

  async #cdp(...commands) {
    this.#turn = callIdentity().turn;
    if (commands.some(([method]) => !READ_ONLY_CDP.has(method))) this.#dirty = true;
    const result = await relay.step({ action: 'cdp', tab: this.#tab, commands: commands.map(([method, params]) => ({ method, params: params ?? {} })) });
    return result.results.map((entry, position) => {
      if (entry.error !== undefined) {
        const error = new Error(`${commands[position][0]}: ${entry.error}`);
        error.cdp = true;
        throw error;
      }
      return entry.result;
    });
  }

  async #evaluate(expression) {
    const [result] = await this.#cdp(['Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }]);
    if (result.exceptionDetails) throw new Error(`Page script failed: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
    return result.result?.value;
  }

  async #settle() {
    if (!this.#dirty) return;
    this.#dirty = false;
    const deadline = Date.now() + PAGE_SETTLE_LIMIT_MS;
    let previous;
    while (Date.now() < deadline) {
      let probe;
      try { probe = await this.#evaluate(SETTLE_PROBE); } catch { probe = undefined; }
      if (probe !== undefined && probe === previous && probe.startsWith('complete:')) return;
      previous = probe;
      await sleep(PAGE_SETTLE_POLL_MS);
    }
  }

  async #history(step) {
    const [history] = await this.#cdp(['Page.getNavigationHistory', {}]);
    const entry = history.entries[history.currentIndex + step];
    if (!entry) throw new Error(`There is no page to go ${step < 0 ? 'back' : 'forward'} to`);
    await this.#cdp(['Page.navigateToHistoryEntry', { entryId: entry.id }]);
    await sleep(PAGE_SETTLE_POLL_MS);
    await this.#settle();
  }

  #node(value) {
    const node = this.#refs.get(index(value));
    if (node === undefined) throw new Error(`Element ${value} is not in this tab's latest state; call getAXState() and use its indexes`);
    return node;
  }

  async #center(value) {
    const backendNodeId = this.#node(value);
    const [, quads] = await this.#cdp(['DOM.scrollIntoViewIfNeeded', { backendNodeId }], ['DOM.getContentQuads', { backendNodeId }]).catch(async error => {
      if (!error.cdp) throw error;
      return [null, (await this.#cdp(['DOM.getContentQuads', { backendNodeId }]))[0]];
    });
    const quad = quads?.quads?.[0];
    if (!quad || quad.length !== 8) throw new Error(`Element ${value} is not visible; scroll it into view or use coordinates`);
    return [(quad[0] + quad[2] + quad[4] + quad[6]) / 4, (quad[1] + quad[3] + quad[5] + quad[7]) / 4];
  }

  async #call(value, functionDeclaration, args = []) {
    const [, result] = await this.#cdp(
      ['DOM.resolveNode', { backendNodeId: this.#node(value) }],
      ['Runtime.callFunctionOn', { objectId: { $from: { command: 0, pointer: '/object/objectId' } }, functionDeclaration, arguments: args.map(arg => ({ value: arg })), returnByValue: true, awaitPromise: true }],
    ).catch(error => {
      if (error.cdp) throw new Error(`Element ${value} is gone; call getAXState() (${error.message})`);
      throw error;
    });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result?.value;
  }

  async #snapshot() {
    await this.#settle();
    const [location, tree] = await this.#cdp(
      ['Runtime.evaluate', { expression: 'JSON.stringify([location.href, document.title])', returnByValue: true }],
      ['Accessibility.getFullAXTree', {}],
    );
    try { [this.url, this.title] = JSON.parse(location.result.value); } catch {}
    const nodes = tree.nodes ?? [];
    const byId = new Map(nodes.map(node => [node.nodeId, node]));
    const children = new Set(nodes.flatMap(node => node.childIds ?? []));
    const lines = [];
    const refs = new Map();
    const seen = new Set();
    const visit = (node, depth) => {
      if (seen.has(node.nodeId)) return;
      seen.add(node.nodeId);
      const role = axText(node.role);
      const name = axText(node.name);
      const ignored = node.ignored || !role || TRANSPARENT.has(role);
      if (!ignored) {
        const indent = '  '.repeat(Math.min(depth, 30));
        let line;
        if (role === 'StaticText' || role === 'text') line = name ? `${indent}- text: ${JSON.stringify(clip(name, 200))}` : '';
        else {
          const properties = node.properties ?? [];
          const property = key => properties.find(entry => entry.name === key)?.value?.value;
          const focusable = property('focusable') === true;
          let prefix = '';
          if ((ACTIONABLE.has(role) || focusable) && Number.isInteger(node.backendDOMNodeId)) {
            let stable = this.#stable.get(node.backendDOMNodeId);
            if (stable === undefined) { stable = this.#nextIndex++; this.#stable.set(node.backendDOMNodeId, stable); }
            refs.set(stable, node.backendDOMNodeId);
            prefix = `[${stable}] `;
          }
          line = `${indent}- ${prefix}${role}${name ? ' ' + JSON.stringify(clip(name, 200)) : ''}`;
          const level = property('level');
          if (typeof level === 'number') line += ` [level=${level}]`;
          for (const flag of FLAGS) {
            const value = property(flag);
            if (value === undefined || value === null || value === false || value === 'false') continue;
            line += value === true || value === 'true' ? ` [${flag}]` : ` [${flag}=${value}]`;
          }
          const url = axText(properties.find(entry => entry.name === 'url')?.value);
          if (url) line += ` [url=${clip(url, 500)}]`;
          const value = axText(node.value);
          if (value && value !== name && value !== url) line += `: ${JSON.stringify(clip(value, 200))}`;
        }
        if (line) lines.push(line);
      }
      if (!ignored && name && (ACTIONABLE.has(role) || LEAVES.has(role) || role === 'text' || role === 'StaticText')) return;
      for (const child of node.childIds ?? []) {
        const next = byId.get(child);
        if (next) visit(next, depth + (ignored ? 0 : 1));
      }
    };
    for (const node of nodes) if (!children.has(node.nodeId)) visit(node, 0);
    this.#refs = refs;
    return lines;
  }

  async #state(diff) {
    const lines = await this.#snapshot();
    const header = `Tab ${this.id} — ${JSON.stringify(this.title ?? '')} ${this.url ?? ''}`.trim();
    const previous = this.#lines;
    this.#lines = lines;
    if (!diff || !previous) return [header, ...lines].join('\n');
    const counts = new Map();
    for (const line of previous) counts.set(line, (counts.get(line) ?? 0) + 1);
    const added = [];
    for (const line of lines) {
      const count = counts.get(line) ?? 0;
      if (count) counts.set(line, count - 1); else added.push(line);
    }
    const removed = [...counts].flatMap(([line, count]) => Array(count).fill(line));
    if (!added.length && !removed.length) return `${header}\nNo accessibility-tree change since the previous state.`;
    if (added.length + removed.length > lines.length) return [header, ...lines].join('\n');
    const row = line => line.trim().replace(/^- /, '');
    return [header, `Since the previous state: ${added.length} added or changed, ${removed.length} removed; other elements keep their index.`, ...added.map(line => '+ ' + row(line)), ...removed.map(line => '- ' + row(line))].join('\n');
  }

  async #capture() {
    await this.#settle();
    const [metrics] = await this.#cdp(['Page.getLayoutMetrics', {}]);
    const viewport = metrics.cssVisualViewport ?? metrics.visualViewport;
    const [shot] = await this.#cdp(['Page.captureScreenshot', { format: 'png', captureBeyondViewport: false, clip: { x: viewport.pageX, y: viewport.pageY, width: viewport.clientWidth, height: viewport.clientHeight, scale: 1 } }]);
    return Buffer.from(shot.data, 'base64');
  }

  async getAXState(options) {
    const text = await this.#state(options?.disableDiffing !== true);
    emitState(text, options);
    return text;
  }

  async getScreenshot(options) {
    const screenshot = await this.#capture();
    await emitScreenshot(screenshot, options);
    return screenshot;
  }

  async getAXStateAndScreenshot(options) {
    const state = await this.#state(options?.disableDiffing !== true);
    const screenshot = await this.#capture();
    emitState(state, options);
    await emitScreenshot(screenshot, options);
    return { state, screenshot };
  }

  async #mouse(x, y, which, count) {
    await this.#cdp(
      ['Input.dispatchMouseEvent', { type: 'mouseMoved', x, y }],
      ['Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: which, clickCount: count }],
      ['Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: which, clickCount: count }],
    );
  }

  async click(target, options) {
    const which = button(options?.mouseButton);
    const count = options?.clickCount ?? 1;
    const [x, y] = isPoint(target) ? target : await this.#center(target);
    for (let click = 1; click <= count; click++) await this.#mouse(x, y, which, click);
    await this.#settle();
  }

  async drag(from, to) {
    if (!isPoint(from) || !isPoint(to)) throw new TypeError('drag expects two [x, y] points');
    const moves = Array.from({ length: DRAG_STEPS }, (_, step) => {
      const t = (step + 1) / DRAG_STEPS;
      return ['Input.dispatchMouseEvent', { type: 'mouseMoved', x: from[0] + (to[0] - from[0]) * t, y: from[1] + (to[1] - from[1]) * t, button: 'left', buttons: 1 }];
    });
    await this.#cdp(
      ['Input.dispatchMouseEvent', { type: 'mouseMoved', x: from[0], y: from[1] }],
      ['Input.dispatchMouseEvent', { type: 'mousePressed', x: from[0], y: from[1], button: 'left', clickCount: 1, buttons: 1 }],
      ...moves,
      ['Input.dispatchMouseEvent', { type: 'mouseReleased', x: to[0], y: to[1], button: 'left', clickCount: 1 }],
    );
    await this.#settle();
  }

  async pressKey(key) {
    await this.#cdp(...keyEvents(key));
    await this.#settle();
  }

  async scroll(target, dir, pages = 1) {
    const resolved = direction(dir);
    const [metrics] = await this.#cdp(['Page.getLayoutMetrics', {}]);
    const viewport = metrics.cssVisualViewport ?? metrics.visualViewport;
    const [x, y] = isPoint(target) ? target : target === undefined || target === null ? [viewport.clientWidth / 2, viewport.clientHeight / 2] : await this.#center(target);
    const vertical = resolved === 'up' || resolved === 'down';
    const sign = resolved === 'up' || resolved === 'left' ? -1 : 1;
    const distance = sign * pages * CHROME_PAGE_STEP_FRACTION * (vertical ? viewport.clientHeight : viewport.clientWidth);
    await this.#cdp(['Input.dispatchMouseEvent', { type: 'mouseWheel', x, y, deltaX: vertical ? 0 : distance, deltaY: vertical ? distance : 0 }]);
    await this.#settle();
  }

  async selectText(elementIndex, text, options) {
    const found = await this.#call(elementIndex, `function(text, prefix, suffix, mode) {
      const wanted = (prefix || '') + text + (suffix || '');
      const offsetIn = value => { const at = value.indexOf(wanted); return at < 0 ? -1 : at + (prefix || '').length; };
      const pick = (start) => mode === 'cursor_before' ? [start, start] : mode === 'cursor_after' ? [start + text.length, start + text.length] : [start, start + text.length];
      if (typeof this.setSelectionRange === 'function' && typeof this.value === 'string') {
        const at = offsetIn(this.value);
        if (at < 0) return false;
        this.focus();
        const [a, b] = pick(at);
        this.setSelectionRange(a, b);
        return true;
      }
      const walker = document.createTreeWalker(this, NodeFilter.SHOW_TEXT);
      const nodes = [];
      let all = '';
      while (walker.nextNode()) { nodes.push([walker.currentNode, all.length]); all += walker.currentNode.data; }
      const at = offsetIn(all);
      if (at < 0) return false;
      const [a, b] = pick(at);
      const locate = offset => { for (let i = nodes.length - 1; i >= 0; i--) if (nodes[i][1] <= offset) return [nodes[i][0], offset - nodes[i][1]]; return [this, 0]; };
      const range = document.createRange();
      range.setStart(...locate(a));
      range.setEnd(...locate(b));
      if (this.isContentEditable) this.focus();
      const selection = getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      return true;
    }`, [String(text), options?.prefix ?? '', options?.suffix ?? '', options?.selectionType ?? 'text']);
    if (!found) throw new Error(`"${text}" was not found in element ${elementIndex}`);
  }

  async setValue(elementIndex, value) {
    const handled = await this.#call(elementIndex, `function(value) {
      if (this instanceof HTMLOptionElement && this.parentElement) {
        const select = this.closest('select');
        if (!select) throw new Error('option is not inside a select');
        select.value = this.value;
        select.dispatchEvent(new Event('input', { bubbles: true }));
        select.dispatchEvent(new Event('change', { bubbles: true }));
        return true;
      }
      if (this instanceof HTMLSelectElement) {
        const option = [...this.options].find(option => option.value === value || option.label === value || option.text === value);
        if (!option) throw new Error('no option ' + value);
        this.value = option.value;
      } else if (this instanceof HTMLInputElement && (this.type === 'checkbox' || this.type === 'radio')) {
        this.checked = value === 'true' || value === 'on' || value === '1';
      } else if (this instanceof HTMLInputElement && ['range', 'color', 'date', 'time', 'datetime-local', 'month', 'week'].includes(this.type)) {
        this.value = value;
      } else {
        const editable = this.isContentEditable || this instanceof HTMLTextAreaElement || (this instanceof HTMLInputElement && !['button', 'submit', 'reset', 'image', 'file', 'hidden'].includes(this.type));
        if (!editable) throw new Error('element is not editable; click it or use typeText');
        this.focus();
        if (typeof this.select === 'function') this.select();
        else { const range = document.createRange(); range.selectNodeContents(this); const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range); }
        return false;
      }
      this.dispatchEvent(new Event('input', { bubbles: true }));
      this.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    }`, [String(value)]);
    if (!handled) {
      if (String(value)) await this.#cdp(['Input.insertText', { text: String(value) }]);
      else await this.#cdp(...keyEvents('BackSpace'));
    }
    await this.#settle();
  }

  async typeText(text) {
    const parts = String(text).split('\n');
    const commands = [];
    parts.forEach((part, position) => {
      if (part) commands.push(['Input.insertText', { text: part }]);
      if (position < parts.length - 1) commands.push(...keyEvents('Return'));
    });
    if (commands.length) await this.#cdp(...commands);
    await this.#settle();
  }

  async paste(text, options) {
    const formatName = options?.format ?? 'text';
    if (formatName === 'html') await this.#evaluate(`document.execCommand('insertHTML', false, ${JSON.stringify(String(text))})`);
    else if (formatName === 'text' || formatName === 'md') await this.#cdp(['Input.insertText', { text: String(text) }]);
    else throw new TypeError(`Unknown paste format ${formatName}`);
    await this.#settle();
  }

  async performSecondaryAction(elementIndex, action) {
    const name = String(action).toLowerCase().replace(/[^a-z]/g, '');
    if (name === 'axshowmenu' || name === 'showmenu' || name === 'contextmenu') return this.click(elementIndex, { mouseButton: 'right' });
    if (name === 'axpress' || name === 'press' || name === 'click') return this.click(elementIndex);
    if (name === 'focus' || name === 'axfocus') { await this.#cdp(['DOM.focus', { backendNodeId: this.#node(elementIndex) }]); return; }
    if (name === 'scrollintoview' || name === 'axscrolltovisible') { await this.#cdp(['DOM.scrollIntoViewIfNeeded', { backendNodeId: this.#node(elementIndex) }]); return; }
    throw new Error(`Browser elements support the secondary actions AXShowMenu, AXPress, focus and scrollIntoView, not ${action}`);
  }

  async goto(url) {
    const [navigated] = await this.#cdp(['Page.navigate', { url: normalizeUrl(url) }]);
    if (navigated?.errorText) throw new Error(`Navigation failed: ${navigated.errorText}`);
    await sleep(PAGE_SETTLE_POLL_MS);
    await this.#settle();
  }

  async back() { await this.#history(-1); }
  async forward() { await this.#history(1); }
  async reload() { await this.#cdp(['Page.reload', {}]); await sleep(PAGE_SETTLE_POLL_MS); await this.#settle(); }

  async close() {
    await relay.step({ action: 'close', tab: this.#tab });
    tabs.delete(this.id);
  }

  async markDeliverable() { await relay.step({ action: 'keep', tab: this.#tab }); this.#keep = true; }
  async markHandoff() { await relay.step({ action: 'keep', tab: this.#tab }); this.#keep = true; }
}

const normalizeUrl = value => {
  const text = String(value).trim();
  return /^[a-z][a-z0-9+.-]*:/i.test(text) ? text : 'https://' + text;
};
const browserId = value => {
  const id = BROWSER_ALIASES[String(value ?? 'chrome').toLowerCase()];
  if (!id) throw new Error(`Browser ${value} is not available; use "chrome", the browser the nanocode extension runs in (this runtime has no in-app browser)`);
  if (!relay.available) throw new Error('No browser is configured for this runtime; set CUA_REPL_BROWSER_SOCKET (nanocode cua install)');
  return id;
};

const createCua = () => {
  let coreSent = false;
  let browserSent = false;
  const docs = browser => {
    if (!coreSent) { writeText(DOCS.core); coreSent = true; }
    if (browser && !browserSent) { writeText(DOCS.browser); browserSent = true; }
  };
  const getTabObject = id => {
    const tab = tabs.get(String(id));
    if (!tab) throw new Error(`Tab not found: ${id}; only tabs this session opened with cua.createBrowserTab() can be used`);
    return tab;
  };
  const openTab = async (id, url, sessionName) => {
    const input = { action: 'open' };
    if (sessionName !== undefined) input.title = String(sessionName);
    const opened = await relay.step(input);
    const tab = new BrowserTab(id, opened.tab, opened);
    tabs.set(tab.id, tab);
    if (url !== undefined) await tab.goto(url);
    return tab;
  };
  const browserObject = id => Object.freeze({
    browserId: id,
    ...BROWSERS[id],
    async documentation() { return DOCS.browser; },
    tabs: Object.freeze({
      async list() { return [...tabs.values()].map(tab => tab.info()); },
      async get(tabId) { return getTabObject(tabId); },
      async new(url) { return openTab(id, url); },
    }),
  });
  const show = async tab => {
    const state = await tab.getAXState({ disableDiffing: true, emit: false });
    docs(true);
    writeText(state);
    return tab;
  };
  return Object.freeze({
    async getState(options) {
      docs(false);
      const apps = (await listDriverApps()).filter(listed).map(appRecord);
      const browsers = relay.available ? [{ ...BROWSERS.chrome, tabs: [...tabs.values()].map(tab => tab.info()) }] : [];
      const state = { apps, browsers };
      emitState(JSON.stringify(state), options);
      return state;
    },
    async getApp(identifier) {
      docs(false);
      if (typeof identifier !== 'string' || !identifier.trim()) throw new TypeError('getApp expects an app name, bundle identifier or path');
      const apps = await listDriverApps();
      let app = findApp(apps, identifier);
      if (!app && !identifier.includes('/') && !/^[\w-]+(\.[\w-]+)+$/.test(identifier)) throw new Error(`No app matches "${identifier}"; call cua.listApps() and retry with its bundle identifier`);
      if (!app?.running) {
        const pid = await launch(app, identifier);
        app = { ...(app ?? { name: identifier }), pid, running: true };
      }
      const bound = new NativeApp(app);
      const { text } = await bound.read({ diff: false });
      writeText(text);
      return bound;
    },
    async listApps(options) {
      docs(false);
      const apps = (await listDriverApps()).filter(listed).map(appRecord).sort((a, b) => Number(b.isRunning) - Number(a.isRunning));
      emitState(JSON.stringify(apps), options);
      return apps;
    },
    async getBrowser(options) {
      const id = browserId(options?.id ?? 'chrome');
      docs(true);
      return browserObject(id);
    },
    async createBrowserTab(id, url, options) {
      if (typeof id !== 'string' || !id.trim()) throw new Error('createBrowserTab requires a browser ID such as "chrome"');
      const resolved = browserId(id);
      if (options?.visible !== undefined) throw new Error(`Browser ${resolved} does not support visible: its tabs always open in the background`);
      return show(await openTab(resolved, url, options?.sessionName));
    },
    async getTab(id, options) {
      if (!id) throw new Error('getTab requires a tab id');
      if (options?.browser !== undefined) browserId(options.browser);
      return show(getTabObject(id));
    },
    async listBrowsers(options) {
      docs(false);
      const list = relay.available ? [BROWSERS.chrome] : [];
      emitState(JSON.stringify(list), options);
      return list;
    },
    async listTabs(options) {
      docs(false);
      if (options?.browser !== undefined) browserId(options.browser);
      const list = [...tabs.values()].map(tab => tab.info());
      emitState(JSON.stringify(list), options);
      return list;
    },
    async rewriteDocumentation() {
      writeText(DOCS.core);
      coreSent = true;
      if (browserSent) writeText(DOCS.browser);
    },
    driver: Object.freeze({
      call: (tool, args) => driver.call(String(tool), args ?? {}),
      tools: () => driver.tools(),
    }),
  });
};

const baseline = new Map(Reflect.ownKeys(globalThis).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
const installGlobals = () => {
  globalThis.cua = createCua();
  globalThis.nodeRepl = nodeRepl;
  globalThis.console = captureConsole;
  globalThis.require = moduleRequire;
  Object.defineProperty(globalThis, '__cua_import', { value: moduleImport, configurable: true, writable: false, enumerable: false });
};
const resetKernel = () => {
  for (const key of Reflect.ownKeys(globalThis)) {
    if (!baseline.has(key)) { try { delete globalThis[key]; } catch {} }
  }
  for (const [key, descriptor] of baseline) {
    try { Object.defineProperty(globalThis, key, descriptor); } catch {}
  }
  installGlobals();
};
installGlobals();

const describeError = error => {
  if (error instanceof Error) {
    const stack = (error.stack ?? '').split('\n').slice(1).filter(line => line.includes('cua_repl.js')).map(line => line.trim());
    return [`${error.name}: ${error.message}`, ...stack].join('\n');
  }
  return `Uncaught ${format(error)}`;
};
const compose = (items, error) => {
  const text = items.filter(item => item.type === 'text').map(item => item.text).join('\n');
  const images = items.filter(item => item.type === 'image');
  if (error) return { content: [{ type: 'text', text: describeError(error) + (text ? '\n' + text : '') }], isError: true };
  const content = [];
  if (text) content.push({ type: 'text', text });
  content.push(...images);
  return { content, isError: false };
};

const runCell = async (code, timeoutMs) => {
  const cell = { items: [] };
  let script;
  try {
    script = new vm.Script(transform(code), { filename: 'cua_repl.js' });
  } catch (error) {
    return compose([], error);
  }
  let timer;
  try {
    const running = cells.run(cell, () => Promise.resolve().then(() => script.runInThisContext()));
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`js execution timed out after ${timeoutMs} ms; work it started may still be running`)), timeoutMs); });
    await Promise.race([running, timeout]);
    return compose(cell.items, null);
  } catch (error) {
    return compose(cell.items, error);
  } finally {
    clearTimeout(timer);
    cell.done = true;
  }
};

const toolError = text => ({ content: [{ type: 'text', text }], isError: true });
const callTool = async (name, args = {}, meta = {}) => {
  currentMeta = meta ?? {};
  switch (name) {
    case 'js': {
      if (typeof args.code !== 'string' || !args.code.trim()) return toolError('js expects non-empty JavaScript source');
      const timeout = args.timeout_ms ?? DEFAULT_TIMEOUT_MS;
      if (!Number.isInteger(timeout) || timeout < 1) return toolError('timeout_ms must be a positive integer');
      return runCell(args.code, timeout);
    }
    case 'js_reset':
      resetKernel();
      return { content: [{ type: 'text', text: 'js kernel reset' }], isError: false };
    case 'js_add_node_module_dir': {
      const dir = args.path;
      if (typeof dir !== 'string' || !path.isAbsolute(dir)) return toolError('path must be an absolute node_modules directory');
      if (!fs.existsSync(dir) || !fs.statSync(dir).isDirectory()) return toolError(`${dir} is not a directory`);
      const resolved = path.basename(dir) === 'node_modules' ? path.dirname(dir) : dir;
      if (!moduleDirs.includes(resolved)) moduleDirs.push(resolved);
      return { content: [{ type: 'text', text: `Added ${dir} to module resolution` }], isError: false };
    }
    case 'turn_ended': {
      const session = String(args.session_id ?? '');
      const turn = String(args.turn_id ?? '');
      if (!session || !turn) return toolError('turn_ended requires session_id and turn_id');
      await closeTabs(tab => tab.turn === turn);
      return { content: [], isError: false };
    }
    default:
      return toolError(`Unknown tool ${name}`);
  }
};

const handle = async message => {
  const { id, method, params } = message;
  if (id === undefined || id === null) return;
  switch (method) {
    case 'initialize':
      send({ jsonrpc: '2.0', id, result: { protocolVersion: params?.protocolVersion ?? '2025-06-18', capabilities: { tools: { listChanged: false } }, serverInfo: { name: 'cua_repl', version: VERSION } } });
      return;
    case 'ping':
      send({ jsonrpc: '2.0', id, result: {} });
      return;
    case 'tools/list':
      send({ jsonrpc: '2.0', id, result: { tools: CATALOG } });
      return;
    case 'tools/call': {
      let result;
      try { result = await callTool(params?.name, params?.arguments ?? {}, params?._meta); } catch (error) { result = toolError(describeError(error)); }
      send({ jsonrpc: '2.0', id, result });
      return;
    }
    default:
      send({ jsonrpc: '2.0', id, error: { code: -32601, message: `Unknown method: ${method}` } });
  }
};

let queue = Promise.resolve();
const shutdown = async () => {
  driver.close();
  await closeTabs(() => true);
  await relay.close();
  process.exit(0);
};
readline.createInterface({ input: process.stdin }).on('line', line => {
  let message;
  try { message = JSON.parse(line); } catch { return; }
  queue = queue.then(() => handle(message)).catch(error => process.stderr.write(`cua_repl: ${error.stack ?? error}\n`));
}).on('close', () => { queue.finally(shutdown); });
process.on('SIGTERM', shutdown);
process.on('uncaughtException', error => process.stderr.write(`cua_repl uncaught: ${error.stack ?? error}\n`));
process.on('unhandledRejection', error => process.stderr.write(`cua_repl unhandled: ${error?.stack ?? error}\n`));
if (relay.available) relay.listen().catch(error => process.stderr.write(`cua_repl browser: ${error.message}\n`));
