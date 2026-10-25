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
const NAVIGATION_TIMEOUT_MS = Number(env.CUA_REPL_NAVIGATION_TIMEOUT_MS ?? 15000);
const ACTION_TIMEOUT_MS = Number(env.CUA_REPL_ACTION_TIMEOUT_MS ?? 5000);
const NATIVE_FIRST_READ_MS = Number(env.CUA_REPL_NATIVE_READ_MS ?? 60);
const PASTE_SETTLE_MS = 150;
const DOM_SNAPSHOT_LIMIT = 200000;
const SCREENSHOT_RETRY_MS = [700, 700, 700, 700, 700, 700];
const SCREENSHOT_TIMEOUT_MS = 8000;
const VIEWPORT = '(() => { const v = visualViewport; const d = document.documentElement; const b = document.body; return [v ? v.pageLeft : scrollX, v ? v.pageTop : scrollY, v ? v.width : innerWidth, v ? v.height : innerHeight, Math.max(d.scrollWidth, b ? b.scrollWidth : 0, innerWidth), Math.max(d.scrollHeight, b ? b.scrollHeight : 0, innerHeight)]; })()';
const DEFAULT_TIMEOUT_MS = 30000;
const CHROME_PAGE_STEP_FRACTION = 0.875;
const DRAG_STEPS = 4;
const MAC = process.platform === 'darwin';

const stdoutWrite = process.stdout.write.bind(process.stdout);
process.stdout.write = (...args) => process.stderr.write(...args);
const send = message => stdoutWrite(JSON.stringify(message) + '\n');
const internal = createRequire(path.join(process.cwd(), '[cua_repl]'));
const acorn = internal('internal/deps/acorn/acorn/dist/acorn');
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

const DOCS = {
  core: [
    '## Computer use',
    '',
    '\`cua_repl\` runs JavaScript in a persistent Node.js session with a \`cua\` object that reads and operates native apps and browser tabs on the user\'s computer. Prefer purpose-built connectors, APIs or CLIs when they can do the job.',
    '',
    '- Native apps go through cua-driver in the background: the app is not raised or focused and the user\'s pointer and keyboard focus stay where they are.',
    '- Browser tabs go through the nanocode extension in the user\'s own Chromium browsers (Chrome, Brave, Edge, ...), in a tab group of this session\'s own, opened in the background. The user\'s own tabs are reached only when you claim one (\`browser.user.claimTab\`).',
    '- \`cua_repl\` state is persistent across calls: \`let app = await cua.getApp("Calculator")\` in one call, \`await app.click(12)\` in the next.',
    '- \`cua.getApp\`, \`cua.getTab\` and \`cua.createBrowserTab\` print the target\'s initial UI state.',
    '',
    '## API',
    '',
    '\`\`\`typescript',
    'type Vec2 = [x: number, y: number];',
    'type ObservationOptions = { emit?: boolean };',
    'type StateOptions = ObservationOptions & { disableDiffing?: boolean };',
    'type StateAndScreenshot = { state: string; screenshot?: Uint8Array };',
    'type PasteOptions = { format?: "text" | "md" | "html" };',
    'type ClickOptions = { mouseButton?: MouseButton; clickCount?: number; key?: string; durationMs?: number };',
    'type PressKeyOptions = { durationMs?: number };',
    'type SelectTextOptions = { prefix?: string; suffix?: string; selectionType?: SelectionType };',
    'type Direction = "up" | "down" | "left" | "right" | "u" | "d" | "l" | "r";',
    'type SelectionType = "text" | "cursor_before" | "cursor_after";',
    'type MouseButton = "left" | "right" | "middle" | "l" | "r" | "m";',
    '',
    'interface Target {',
    '  getAXState(options?: StateOptions): Promise<string>;',
    '  getScreenshot(options?: ObservationOptions): Promise<Uint8Array>;',
    '  getAXStateAndScreenshot(options?: StateOptions): Promise<StateAndScreenshot>;',
    '  click(target: number | Vec2, options?: ClickOptions): Promise<void>;',
    '  drag(from: Vec2, to: Vec2): Promise<void>;',
    '  scroll(target: number | Vec2, direction: Direction, pages?: number | { pixels: number }): Promise<void>;',
    '  selectText(elementIndex: number, text: string, options?: SelectTextOptions): Promise<void>;',
    '  setValue(elementIndex: number, value: string): Promise<void>;',
    '  performSecondaryAction(elementIndex: number, action: string): Promise<void>;',
    '}',
    '',
    'type AppInfo = { id: string; displayName?: string; lastUsedDate?: string; isRunning?: boolean };',
    'type WindowInfo = { id: number; app: string; title?: string };',
    'interface App extends Target {',
    '  paste(text: string, options?: PasteOptions): Promise<void>;',
    '  pressKey(key: string, options?: PressKeyOptions): Promise<void>;',
    '  typeText(text: string): Promise<void>;',
    '}',
    '',
    'type BrowserInfo = { id: string; name?: string; family?: string; type?: "extension" };',
    'type TabInfo = { id: string; providerTabId?: string; browserId: string; title?: string; url?: string };',
    'type State = { apps: AppInfo[]; browsers: (BrowserInfo & { tabs: TabInfo[] })[]; errors?: string[] };',
    'type CreateBrowserTabOptions = { visible?: false; sessionName?: string };',
    '',
    '/** Browser tabs: element-index input plus Playwright locators, see the browser documentation. */',
    'interface Tab extends Target {',
    '  readonly id: string;',
    '  paste(elementIndex: number | null, text: string, options?: PasteOptions): Promise<void>;',
    '  pressKey(elementIndex: number | null, key: string, options?: PressKeyOptions): Promise<void>;',
    '  typeText(elementIndex: number | null, text: string): Promise<void>;',
    '  goto(url: string): Promise<void>;',
    '  back(): Promise<void>;',
    '  forward(): Promise<void>;',
    '  reload(): Promise<void>;',
    '  close(): Promise<void>;',
    '  markDeliverable(): Promise<void>;',
    '  markHandoff(): Promise<void>;',
    '}',
    '',
    'declare const cua: {',
    '  getState(options?: ObservationOptions): Promise<State>;',
    '  getApp(target: string | { windowId: number }): Promise<App>;',
    '  listApps(options?: ObservationOptions): Promise<AppInfo[]>;',
    '  listWindows(options?: ObservationOptions): Promise<WindowInfo[]>;',
    '  /** Select a browser without opening a tab; use the returned browserId with createBrowserTab. */',
    '  getBrowser(options?: { id?: string; url?: string }): Promise<Browser>;',
    '  createBrowserTab(browserId: string, url?: string, options?: CreateBrowserTabOptions): Promise<Tab>;',
    '  /** Bind a tab of this session, or claim a user tab, by id or { url }. */',
    '  getTab(reference: string | { url: string }, options?: { browser?: string }): Promise<Tab>;',
    '  listBrowsers(options?: ObservationOptions): Promise<BrowserInfo[]>;',
    '  listTabs(options?: { browser?: string } & ObservationOptions): Promise<TabInfo[]>;',
    '  rewriteDocumentation(): Promise<void>;',
    '  computer: {',
    '    target: "mac" | "linux" | "windows";',
    '    list_apps(): Promise<AppInfo[]>;',
    '    launch_app(input: { app: string }): Promise<void>;',
    '    get_app_state(input: { app: string; disableDiff?: boolean }): Promise<{ app: string; text: string; screenshot?: string }>;',
    '    click(input: { app: string; element_index?: number; x?: number; y?: number; mouse_button?: MouseButton; click_count?: number }): Promise<void>;',
    '    drag(input: { app: string; from_x: number; from_y: number; to_x: number; to_y: number }): Promise<void>;',
    '    perform_secondary_action(input: { app: string; element_index: number; action: string }): Promise<void>;',
    '    press_key(input: { app: string; key: string }): Promise<void>;',
    '    scroll(input: { app: string; element_index: number; direction: Direction; pages?: number }): Promise<void>;',
    '    select_text(input: { app: string; element_index: number; text: string; prefix?: string; suffix?: string; selection_type?: SelectionType }): Promise<void>;',
    '    set_value(input: { app: string; element_index: number; value: string }): Promise<void>;',
    '    type_text(input: { app: string; text: string }): Promise<void>;',
    '  };',
    '  driver: { call(tool: string, args?: object): Promise<object>; tools(): Promise<string[]> };',
    '};',
    '\`\`\`',
    '',
    '## Workflow',
    '',
    'After one or more actions, call \`getAXState()\` before deciding what to do next, and use the element indexes of that latest state. By default it returns only what changed since the previous state of that target (added, changed and removed rows); pass \`{ disableDiffing: true }\` for the whole tree. Batch deterministic actions and the \`getAXState()\` that checks them into one call:',
    '',
    '\`\`\`typescript',
    'await app.click(42);',
    'await app.setValue(17, "openai.com");',
    'await app.pressKey("Return");',
    'await tab.typeText(42, "hello");',
    'await app.scroll(42, "down", 1);',
    'await app.performSecondaryAction(42, "AXShowMenu");',
    'await app.getAXState();',
    '\`\`\`',
    '',
    'Observation methods wait for the UI to settle before reading it, so do not sleep before them. If a standalone \`getAXState()\` reports no change, do not repeat it without an intervening action. Once the requested result is visible in the state, stop and answer.',
    '',
    '## Output',
    '',
    '- \`nodeRepl.write(value)\` adds text (non-strings are inspected); \`await nodeRepl.emitImage(image)\` adds an image (PNG, JPEG or WebP bytes, a data: or file: URL, or \`{ bytes, mimeType }\`).',
    '- \`getAXState()\`, \`getScreenshot()\`, \`getAXStateAndScreenshot()\`, \`cua.getState()\`, \`cua.getApp()\`, \`cua.getTab()\`, \`cua.createBrowserTab()\`, \`cua.listApps()\`, \`cua.listWindows()\`, \`cua.listBrowsers()\` and \`cua.listTabs()\` print their result themselves; pass \`{ emit: false }\` to observation and listing methods to keep it out. \`cua.getBrowser()\` prints its documentation the first time. A cell\'s final expression is not printed.',
    '',
    '## Notes',
    '',
    '- Prefer element indexes over coordinates. Coordinates are pixels of the target\'s latest screenshot. Use screenshots when the accessibility tree misses what you need.',
    '- \`cua.getApp()\` takes a display name, bundle identifier or app path (or \`{ windowId }\` from \`cua.listWindows()\`) and launches a missing app in the background. A name several apps share is an error naming their bundle identifiers.',
    '- \`performSecondaryAction()\` invokes an action the element lists (such as AXShowMenu, AXPick, AXConfirm, AXCancel, AXOpen); do not guess names.',
    '- \`pressKey()\` takes xdotool-style keys and chords: \`"Return"\`, \`"Tab"\`, \`"Up"\`, \`"Page_Down"\`, \`"super+c"\` (super is Command on macOS), \`"ctrl+shift+z"\`, \`"KP_0"\`.',
    '- Native \`paste\` puts the text on the system clipboard, pastes it and restores the previous clipboard text.',
    '- Native \`selectText()\` focuses the field and moves the caret with arrow keys, so it needs the field\'s text in the latest state.',
    '- \`cua.driver.call(tool, args)\` calls a cua-driver MCP tool directly, for what the API above does not cover.',
    '',
    'Persist until the request is done end to end and the latest state visibly shows the result, or explain the concrete blocker.',
  ].join('\n'),
};
DOCS.browser = [
  '## Browser tabs',
  '',
  'Browsers are the user\'s own Chromium browsers running the nanocode extension. \`cua.getState()\` lists them with ids ("1", "2", ...) and names ("Chrome", "Brave", "Edge"); \`createBrowserTab\` and \`getBrowser\` take either, and "chrome" means the connected Chrome (or the only browser). There is no in-app browser: "iab" and "mcpapps" are refused. Tabs open in the background in a tab group named after \`sessionName\` (pass a short, emoji-prefixed name when starting a task) and never take focus.',
  '',
  '- A tab\'s \`getAXState()\` is its accessibility tree with \`[N]\` element indexes that stay with the same element across later states; coordinates are CSS pixels of the viewport screenshot.',
  '- \`tab.playwright\` offers Playwright locators that act through trusted input at the element\'s center after scrolling it into view, waiting until it is visible, enabled and receives pointer events (default timeout 5000 ms, \`timeoutMs\` to change). Single-element actions on a locator that matches several elements fail with a strict-mode error listing them: narrow it with \`exact: true\`, \`.filter()\`, \`.first()\` or \`.nth()\`.',
  '- \`tab.playwright.domSnapshot()\` is a compact YAML outline of the page (roles, accessible names, states, link URLs, text, iframe content), the ground truth for locators.',
  '- The tabs a turn opened close when the turn ends. Call \`tab.markDeliverable()\` (a result the user should see) or \`tab.markHandoff()\` (the user or a later turn continues there) to leave them open. Claimed user tabs are never closed for you.',
  '- If a tab already shows the URL, do not \`goto\` it again (that reloads it); call \`tab.reload()\` when you mean to.',
  '',
  '\`\`\`typescript',
  'declare const agent: {',
  '  browsers: { list(): Promise<BrowserInfo[]>; get(id: string): Promise<Browser> };',
  '  documentation: { get(name: "browser-troubleshooting" | "local-web-development" | "file-uploads" | "screenshots"): Promise<string> };',
  '};',
  '',
  'interface Browser {',
  '  browserId: string;',
  '  capabilities: { list(): Promise<{ id: string; description: string }[]>; get(id: "viewport"): Promise<Viewport> };',
  '  tabs: { get(id: string): Promise<Tab>; list(): Promise<TabInfo[]>; new(url?: string): Promise<Tab>; selected(): Promise<Tab | undefined> };',
  '  user: { openTabs(): Promise<UserTabInfo[]>; claimTab(tab: string | UserTabInfo): Promise<Tab> };',
  '  documentation(): Promise<string>;',
  '  history(options: { from?: string | Date; to?: string | Date; queries?: string[]; limit?: number }): Promise<{ dateVisited: string; title?: string; url: string }[]>;',
  '  nameSession(name: string): Promise<void>;',
  '}',
  'type UserTabInfo = { id: string; lastOpened?: string; providerTabId?: string; tabGroup?: string; title?: string; url?: string };',
  'interface Viewport { documentation(): Promise<string>; get(): Promise<{ width: number; height: number } | null>; set(size: { width: number; height: number; deviceScaleFactor?: number; mobile?: boolean }): Promise<void>; reset(): Promise<void> }',
  '',
  'interface Tab extends Target {',
  '  id: string;',
  '  playwright: PlaywrightAPI;',
  '  dev: { logs(options?: { filter?: string; levels?: ("debug" | "info" | "log" | "warn" | "error")[]; limit?: number }): Promise<{ level: string; message: string; timestamp: string; url?: string }[]> };',
  '  clipboard: { readText(): Promise<string>; writeText(text: string): Promise<void>; read(): Promise<ClipboardItem[]>; write(items: ClipboardItem[]): Promise<void> };',
  '  content: { exportGsuite(type: "pdf" | "md" | "docx" | "txt" | "xlsx" | "csv" | "pptx"): Promise<string> };',
  '  capabilities: { list(): Promise<{ id: string; description: string }[]>; get(id: "pageAssets"): Promise<PageAssets> };',
  '  title(): Promise<string | undefined>;',
  '  url(): Promise<string | undefined>;',
  '  goto(url: string): Promise<void>;',
  '  back(): Promise<void>;',
  '  forward(): Promise<void>;',
  '  reload(): Promise<void>;',
  '  close(): Promise<void>;',
  '  screenshot(options?: { fullPage?: boolean; clip?: { x: number; y: number; width: number; height: number } }): Promise<Uint8Array>;',
  '  getJsDialog(): Promise<undefined | { type: "alert" | "confirm" | "prompt" | "beforeunload"; message: string; accept?(text?: string): Promise<void>; dismiss(): Promise<void> }>;',
  '  markDeliverable(): Promise<void>;',
  '  markHandoff(): Promise<void>;',
  '  typeText(elementIndex: number | null, text: string): Promise<void>;',
  '  pressKey(elementIndex: number | null, key: string): Promise<void>;',
  '  paste(elementIndex: number | null, text: string, options?: PasteOptions): Promise<void>;',
  '}',
  '',
  'type TextMatcher = string | RegExp;',
  'interface PlaywrightAPI {',
  '  domSnapshot(): Promise<string>;',
  '  evaluate<R, A>(pageFunction: string | ((arg: A) => R | Promise<R>), arg?: A): Promise<R>;',
  '  expectNavigation<T>(action: () => Promise<T>, options?: { url?: string | RegExp; waitUntil?: "load" | "domcontentloaded" | "networkidle" | "commit"; timeoutMs?: number }): Promise<T>;',
  '  frameLocator(selector: string): FrameLocator;',
  '  getByRole(role: string, options?: { name?: TextMatcher; exact?: boolean; checked?: boolean; disabled?: boolean; expanded?: boolean; level?: number; pressed?: boolean; selected?: boolean; includeHidden?: boolean }): Locator;',
  '  getByText(text: TextMatcher, options?: { exact?: boolean }): Locator;',
  '  getByLabel(text: TextMatcher, options?: { exact?: boolean }): Locator;',
  '  getByPlaceholder(text: TextMatcher, options?: { exact?: boolean }): Locator;',
  '  getByAltText(text: TextMatcher, options?: { exact?: boolean }): Locator;',
  '  getByTitle(text: TextMatcher, options?: { exact?: boolean }): Locator;',
  '  getByTestId(testId: TextMatcher): Locator;',
  '  locator(selector: string, options?: FilterOptions): Locator; // CSS (with :has-text(), :text(), :visible), text=, xpath=, role=, >> chains, nth=',
  '  waitForEvent(event: "download" | "filechooser" | "dialog", options?: { timeoutMs?: number }): Promise<Download | FileChooser | Dialog>;',
  '  waitForLoadState(options?: { state?: "load" | "domcontentloaded" | "networkidle"; timeoutMs?: number }): Promise<void>;',
  '  waitForTimeout(timeoutMs: number): Promise<void>;',
  '  waitForURL(url: string | RegExp, options?: { timeoutMs?: number; waitUntil?: "load" | "domcontentloaded" | "networkidle" | "commit" }): Promise<void>;',
  '  keyboard: { press(key: string): Promise<void>; type(text: string): Promise<void>; insertText(text: string): Promise<void> };',
  '  mouse: { click(x: number, y: number): Promise<void>; move(x: number, y: number): Promise<void>; wheel(dx: number, dy: number): Promise<void> };',
  '}',
  'type FilterOptions = { has?: Locator; hasNot?: Locator; hasText?: TextMatcher; hasNotText?: TextMatcher; visible?: boolean };',
  'type ActionOptions = { timeoutMs?: number; force?: boolean };',
  'interface Locator {',
  '  locator(selector: string | Locator, options?: FilterOptions): Locator;',
  '  getByRole(...): Locator; getByText(...): Locator; getByLabel(...): Locator; getByPlaceholder(...): Locator; getByTestId(...): Locator; frameLocator(selector: string): FrameLocator;',
  '  filter(options: FilterOptions): Locator; first(): Locator; last(): Locator; nth(index: number): Locator; and(other: Locator): Locator; or(other: Locator): Locator;',
  '  all(): Promise<Locator[]>; count(): Promise<number>;',
  '  click(options?: ActionOptions & { button?: "left" | "right" | "middle"; modifiers?: ("Alt" | "Control" | "ControlOrMeta" | "Meta" | "Shift")[]; position?: { x: number; y: number }; clickCount?: number }): Promise<void>;',
  '  dblclick(options?): Promise<void>; hover(options?): Promise<void>; focus(): Promise<void>;',
  '  fill(value: string, options?: ActionOptions): Promise<void>; clear(): Promise<void>;',
  '  press(key: string, options?): Promise<void>; // Playwright keys: "Enter", "Meta+k", "Control+A", "ArrowDown"',
  '  pressSequentially(text: string, options?): Promise<void>; type(text: string, options?): Promise<void>;',
  '  check(options?): Promise<void>; uncheck(options?): Promise<void>; setChecked(checked: boolean, options?): Promise<void>;',
  '  selectOption(value: string | { value?: string; label?: string; index?: number } | (string | object)[], options?): Promise<string[]>;',
  '  setInputFiles(files: string | string[]): Promise<void>;',
  '  textContent(): Promise<string | null>; innerText(): Promise<string>; inputValue(): Promise<string>; getAttribute(name: string): Promise<string | null>;',
  '  allTextContents(): Promise<string[]>; allInnerTexts(): Promise<string[]>; boundingBox(): Promise<{ x: number; y: number; width: number; height: number } | null>; ariaSnapshot(): Promise<string>;',
  '  isVisible(): Promise<boolean>; isHidden(): Promise<boolean>; isEnabled(): Promise<boolean>; isChecked(): Promise<boolean>; isEditable(): Promise<boolean>;',
  '  waitFor(options?: { state?: "attached" | "detached" | "visible" | "hidden"; timeoutMs?: number }): Promise<void>;',
  '  evaluate<R, A>(fn: string | ((element: Element, arg: A) => R), arg?: A): Promise<R>;',
  '  evaluateAll<R, A>(fn: string | ((elements: Element[], arg: A) => R), arg?: A): Promise<R>;',
  '  screenshot(): Promise<Uint8Array>; downloadMedia(options?: { timeoutMs?: number }): Promise<string>;',
  '}',
  '\`\`\`',
  '',
  'Example:',
  '',
  '\`\`\`typescript',
  'const browser = await cua.getBrowser();',
  'await browser.nameSession("🔎 Wikipedia lookup");',
  'const tab = await browser.tabs.new("https://en.wikipedia.org");',
  'await tab.playwright.getByRole("searchbox", { name: "Search Wikipedia" }).fill("Alan Turing");',
  'await tab.playwright.getByRole("searchbox", { name: "Search Wikipedia" }).press("Enter");',
  'await tab.playwright.waitForURL(/Alan_Turing/);',
  'nodeRepl.write(await tab.playwright.domSnapshot());',
  '\`\`\`',
].join('\n');

const DOCUMENTATION = {
  'browser-troubleshooting': [
    '# Browser Interaction Troubleshooting',
    '- Read the page with \`tab.playwright.domSnapshot()\` or \`tab.getAXState()\` before retrying an action, and target what is really there.',
    '- A strict-mode violation means the locator matched several elements: add \`exact: true\`, \`.filter({ hasText })\`, scope it with a parent locator, or pick one with \`.first()\`/\`.nth()\`.',
    '- A timeout that names an element intercepting pointer events means something covers the target (a dialog, cookie banner or overlay): close it first.',
    '- A JavaScript dialog blocks the page: read it with \`tab.getJsDialog()\` and accept or dismiss it.',
    '- \`tab.dev.logs({ levels: ["error", "warn"] })\` shows console errors since the tab was opened or claimed.',
    '- If a tab is gone (the user closed it), open a new one with \`browser.tabs.new(url)\`; do not reselect the browser.',
    '- Background tabs run timers slowly; locator actions and observations already wait for the page to settle, so avoid fixed sleeps.',
  ].join('\n'),
  'local-web-development': [
    '# Local Web Development',
    'When testing a local app (localhost, 127.0.0.1 or another development URL), reload after code or build changes if the framework does not hot-reload: call \`tab.reload()\`, then take a fresh \`domSnapshot()\`. Check \`tab.dev.logs({ levels: ["error"] })\` for runtime errors after each change, and use the \`viewport\` browser capability only when the task asks for specific sizes.',
  ].join('\n'),
  'file-uploads': [
    '# File Uploads',
    'Prefer \`locator.setInputFiles(paths)\` on the \`<input type=file>\` (it may be hidden). When the page opens the chooser from a button, start waiting first and then click:',
    '\`\`\`typescript',
    'const chooser = tab.playwright.waitForEvent("filechooser");',
    'await tab.playwright.getByRole("button", { name: "Upload" }).click();',
    'await (await chooser).setFiles(["/absolute/path/file.pdf"]);',
    '\`\`\`',
    'Paths are absolute paths on the user\'s computer.',
  ].join('\n'),
  'chrome-file-upload-troubleshooting': 'If setFiles or setInputFiles has no effect, check that the input is the one the page reads (several inputs may exist; locate it with locator(\'input[type=file]\')), and that the files exist and are readable. Drag-and-drop-only upload areas need a hidden input; look for one in evaluate() output.',
  screenshots: [
    '# Screenshots',
    '\`tab.screenshot()\` returns PNG bytes of the viewport, \`{ fullPage: true }\` the whole page and \`{ clip: { x, y, width, height } }\` a region in CSS pixels; \`locator.screenshot()\` captures one element. Show one with \`await nodeRepl.emitImage(bytes)\`, or save it with \`fs.writeFileSync\` when the user asked for a file.',
  ].join('\n'),
  viewport: [
    '# Browser Capability: viewport',
    'Overrides the viewport size of this session\'s tabs (current and new ones). Do not set it during normal browsing; use it only when the task asks for specific dimensions or a responsive breakpoint, and reset it before finishing unless the user asked to keep it.',
    '- \`set({ width, height, deviceScaleFactor?, mobile? })\` applies the override.',
    '- \`get()\` returns the current override or null.',
    '- \`reset()\` returns to the browser window\'s own size.',
  ].join('\n'),
  pageAssets: [
    '# Tab Capability: pageAssets',
    '- \`list({ type? })\` returns \`{ url, type, bytes? }\` for resources the page loaded (images, scripts, CSS, media, fetches), from the Performance API and media elements.',
    '- \`bundle(urls)\` downloads the given assets with the browser (the user\'s cookies apply) into a new temporary directory and returns \`{ path, files }\`.',
  ].join('\n'),
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
    const child = spawn(this.#executable, (env.CUA_DRIVER_ARGS || 'mcp').split(' ').filter(Boolean), { stdio: ['pipe', 'pipe', 'pipe'], env: process.env });
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
  up: 'up', down: 'down', left: 'left', right: 'right', arrowup: 'up', arrowdown: 'down', arrowleft: 'left', arrowright: 'right', space: 'space', ' ': 'space', minus: '-', plus: '+', equal: '=', comma: ',', period: '.', slash: '/',
};
const MODIFIERS = { super: 'cmd', cmd: 'cmd', command: 'cmd', meta: 'cmd', win: 'cmd', ctrl: 'ctrl', control: 'ctrl', controlormeta: MAC ? 'cmd' : 'ctrl', alt: 'alt', option: 'alt', opt: 'alt', shift: 'shift' };
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
  #rows = new Map();

  constructor(info, window) {
    this.#info = info;
    if (window) this.#window = window;
    Object.defineProperty(this, 'id', { value: info.bundle_id ?? info.launch_path ?? info.name, enumerable: true });
    Object.defineProperty(this, 'displayName', { value: info.name, enumerable: true });
  }

  get pid() { return this.#info.pid; }
  [util.inspect.custom]() { return 'App ' + this.#info.name + ' (pid ' + this.#info.pid + ')'; }

  async #resolveWindow(refresh = false) {
    if (this.#window && !refresh) return this.#window;
    const result = await driver.call('list_windows', { pid: this.#info.pid });
    const windows = result.structuredContent?.windows ?? [];
    if (!windows.length) throw new Error(this.#info.name + ' has no window to read; open one first or check that it is running');
    const rank = window => [window.is_on_screen ? 1 : 0, window.title ? 1 : 0, Number.isInteger(window.z_index) ? window.z_index : -1];
    windows.sort((a, b) => {
      const [x, y] = [rank(a), rank(b)];
      return y[0] - x[0] || y[1] - x[1] || y[2] - x[2];
    });
    const chosen = windows.find(window => window.window_id === this.#window?.window_id && window.is_on_screen) ?? windows[0];
    this.#window = chosen;
    return chosen;
  }

  async #windowState(args) {
    const window = await this.#resolveWindow();
    try {
      return { window, result: await driver.call('get_window_state', { ...args, pid: this.#info.pid, window_id: window.window_id }) };
    } catch (error) {
      const fresh = await this.#resolveWindow(true).catch(() => null);
      if (!fresh) throw error;
      const retry = { ...args };
      delete retry.since;
      return { window: fresh, result: await driver.call('get_window_state', { ...retry, pid: this.#info.pid, window_id: fresh.window_id }) };
    }
  }

  #remember(markdown) {
    for (const line of String(markdown ?? '').split('\n')) {
      const match = /\[(\d+)\]/.exec(line);
      if (match) this.#rows.set(Number(match[1]), line);
    }
  }

  async read({ tree = true, screenshot = false, diff = true } = {}) {
    const acted = this.#lastAction;
    this.#lastAction = 0;
    if (acted) {
      const wait = acted + NATIVE_FIRST_READ_MS - Date.now();
      if (wait > 0) await sleep(wait);
    }
    const args = { include_screenshot: screenshot, include_accessibility_tree: tree };
    if (!tree) args.display_only = true;
    else if (diff && this.#snapshot) args.since = 'latest';
    let { window, result } = await this.#windowState(args);
    let state = result.structuredContent ?? {};
    if (tree && acted && state.since_status === 'no_change' && Date.now() < acted + SETTLE_MS) {
      await sleep(acted + SETTLE_MS - Date.now());
      ({ window, result } = await this.#windowState(args));
      state = result.structuredContent ?? {};
    }
    let text;
    if (tree) {
      if (state.snapshot_id) this.#snapshot = state.snapshot_id;
      if (state.since_status !== 'diff' && state.since_status !== 'no_change') this.#rows.clear();
      this.#remember(state.tree_markdown ?? state.tree_diff);
      const title = state.window_title ?? window.title ?? '';
      const header = this.#info.name + ' (pid ' + this.#info.pid + ')' + (title ? ' — window "' + title + '"' : '');
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
      if (state.truncated || (state.elements_complete === false && state.returned_element_count < state.total_element_count)) notes.push('(tree cut at ' + (state.returned_element_count ?? '?') + ' of ' + (state.total_element_count ?? '?') + ' elements)');
      if (state.degraded_reason) notes.push('(' + state.degraded_reason + ')');
      text = [header, body, ...notes].join('\n');
    }
    return { text, screenshot: imageOf(result) };
  }

  #token(value) {
    if (!this.#snapshot) throw new Error('Read the app first with getAXState(); element indexes come from its latest state');
    return this.#snapshot + ':' + index(value);
  }

  async #act(name, args) {
    const window = args.element_token ? null : await this.#resolveWindow();
    try {
      return await driver.call(name, { ...args, ...(window ? { pid: this.#info.pid, window_id: window.window_id } : {}) });
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
    if (!screenshot) throw new Error('Screenshot unavailable for ' + this.#info.name + '; grant Screen Recording to cua-driver');
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
    const modifiers = options?.key ? nativeKey(options.key + '+a').modifiers : undefined;
    if (isPoint(target)) {
      await this.#act('click', { x: target[0], y: target[1], button: which, count, ...(modifiers?.length ? { modifiers } : {}) });
      return;
    }
    const element_token = this.#token(target);
    if (count === 2 && which === 'left') await this.#act('double_click', { element_token });
    else if (which === 'right') await this.#act('right_click', { element_token });
    else await this.#act('click', { element_token, button: which === 'left' ? undefined : which, count: count === 1 ? undefined : count });
  }

  async drag(start, end) {
    if (!isPoint(start) || !isPoint(end)) throw new TypeError('drag expects two [x, y] points');
    await this.#act('drag', { from_x: start[0], from_y: start[1], to_x: end[0], to_y: end[1] });
  }

  async pressKey(key, options) {
    const chord = nativeKey(key);
    await this.#act('press_key', { key: chord.key, modifiers: chord.modifiers.length ? chord.modifiers : undefined });
    if (options?.durationMs) await sleep(options.durationMs);
  }

  async scroll(target, dir, distance = 1) {
    const where = isPoint(target) ? { x: target[0], y: target[1] } : { element_token: this.#token(target) };
    const pixels = distance && typeof distance === 'object' && Number.isFinite(distance.pixels);
    await this.#act('scroll', { ...where, direction: direction(dir), ...(pixels ? { by: 'pixel', amount: Math.max(1, Math.round(distance.pixels)) } : { by: 'page', amount: Math.max(1, Math.round(distance)) }) });
  }

  async selectText(elementIndex, text, options) {
    const row = this.#rows.get(index(elementIndex));
    const value = row && (/ value="((?:[^"\\]|\\.)*)"/.exec(row)?.[1] ?? /^\s*- \[\d+\] [^"]*"((?:[^"\\]|\\.)*)"/.exec(row)?.[1]);
    if (value === undefined || value === null) throw new Error('Element ' + elementIndex + ' shows no text value in the latest state; click into the field and use pressKey to move the caret instead');
    const decoded = value.replace(/\\(["\\])/g, '$1').replace(/\\n/g, '\n');
    const wanted = (options?.prefix ?? '') + String(text) + (options?.suffix ?? '');
    const found = decoded.indexOf(wanted);
    if (found < 0) throw new Error('"' + text + '" was not found in element ' + elementIndex);
    if (decoded.indexOf(wanted, found + 1) >= 0) throw new Error('"' + wanted + '" occurs more than once in element ' + elementIndex + '; pass prefix or suffix to pick one');
    const start = found + (options?.prefix ?? '').length;
    const mode = options?.selectionType ?? 'text';
    const caret = mode === 'cursor_after' ? start + String(text).length : start;
    const length = mode === 'text' ? String(text).length : 0;
    const steps = [
      { tool: 'click', args: { element_token: this.#token(elementIndex) } },
      { tool: 'press_key', args: { key: 'a', modifiers: [MAC ? 'cmd' : 'ctrl'] } },
      { tool: 'press_key', args: { key: 'left' } },
      ...Array.from({ length: caret }, () => ({ tool: 'press_key', args: { key: 'right' } })),
      ...Array.from({ length }, () => ({ tool: 'press_key', args: { key: 'right', modifiers: ['shift'] } })),
    ];
    if (steps.length > 600) throw new Error('The text is too far into element ' + elementIndex + ' to select with keys; click near it instead');
    const window = await this.#resolveWindow();
    try {
      for (let i = 0; i < steps.length; i += 64) await driver.call('run_steps', { pid: this.#info.pid, window_id: window.window_id, steps: steps.slice(i, i + 64) });
    } finally {
      this.#lastAction = Date.now();
    }
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
    if (!['text', 'md', 'html'].includes(formatName)) throw new TypeError('Unknown paste format ' + formatName);
    let previous;
    try { previous = (await driver.call('clipboard_read', { include_text: true })).structuredContent?.text; } catch {}
    await driver.call('clipboard_write', { text: String(text) });
    try {
      await this.#act('press_key', { key: 'v', modifiers: [MAC ? 'cmd' : 'ctrl'] });
      await sleep(PASTE_SETTLE_MS);
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
  const key = app => app.bundle_id ?? app.launch_path ?? app.name;
  const pick = (list, label) => {
    const ids = [...new Set(list.map(key))];
    if (ids.length <= 1) return list.sort((a, b) => (b.running ? 1 : 0) - (a.running ? 1 : 0))[0] ?? null;
    const running = [...new Set(list.filter(app => app.running).map(key))];
    if (running.length === 1) return list.find(app => app.running);
    throw new Error('App "' + wanted + '" is ambiguous: ' + label + ' ' + ids.slice(0, 8).map(id => '"' + id + '"').join(', ') + '; pass a bundle identifier or app path');
  };
  const byId = apps.filter(app => app.bundle_id === wanted || app.launch_path === wanted || (app.bundle_id && app.bundle_id.toLowerCase() === lower));
  if (byId.length) return pick(byId, 'it matches');
  const byName = apps.filter(app => app.name?.toLowerCase() === lower || (wanted.includes('/') && (app.name?.toLowerCase() === base || app.launch_path?.toLowerCase() === lower)));
  if (byName.length) return pick(byName, 'several apps are named that:');
  const partial = apps.filter(app => app.name?.toLowerCase().includes(lower));
  if (partial.length) return pick(partial, 'it matches');
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

function nanocodeEngine() {
  const KEY = Symbol.for('nanocode.engine');
  if (globalThis[KEY]) return globalThis[KEY];
  const SKIP = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'TEMPLATE', 'HEAD', 'TITLE', 'META', 'LINK', 'BASE']);
  const ROLES = new Set('alert alertdialog application article banner blockquote button caption cell checkbox code columnheader combobox complementary contentinfo definition deletion dialog directory document emphasis feed figure form generic grid gridcell group heading img image insertion link list listbox listitem log main mark marquee math menu menubar menuitem menuitemcheckbox menuitemradio meter navigation none note option paragraph presentation progressbar radio radiogroup region row rowgroup rowheader scrollbar search searchbox separator slider spinbutton status strong subscript superscript switch tab table tablist tabpanel term textbox time timer toolbar tooltip tree treegrid treeitem'.split(' '));
  const FROM_CONTENT = new Set(['button', 'cell', 'checkbox', 'columnheader', 'gridcell', 'heading', 'link', 'menuitem', 'menuitemcheckbox', 'menuitemradio', 'option', 'radio', 'row', 'rowheader', 'switch', 'tab', 'tooltip', 'treeitem', 'caption', 'code', 'deletion', 'emphasis', 'insertion', 'mark', 'strong', 'subscript', 'superscript', 'term', 'time']);
  const SCOPES = 'article, aside, main, nav, section, [role=article], [role=complementary], [role=main], [role=navigation], [role=region]';
  const TEXT_INPUTS = new Set(['', 'text', 'search', 'email', 'tel', 'url', 'password', 'number']);
  const VALUE_INPUTS = new Set(['date', 'time', 'datetime-local', 'month', 'week', 'color', 'range']);
  const norm = value => String(value ?? '').replace(/[\s\u00a0\u200b]+/g, ' ').trim();
  const matcher = (pattern, exact) => {
    if (pattern && typeof pattern === 'object' && typeof pattern.source === 'string') {
      const re = new RegExp(pattern.source, String(pattern.flags ?? '').replace('g', ''));
      return text => re.test(norm(text));
    }
    const wanted = norm(pattern);
    if (exact) return text => norm(text) === wanted;
    const lower = wanted.toLowerCase();
    return text => norm(text).toLowerCase().includes(lower);
  };
  const isElement = node => node && node.nodeType === 1;
  const css = el => (el.ownerDocument.defaultView || window).getComputedStyle(el);
  const parentOf = node => node.parentElement || (node.parentNode && node.parentNode.host) || null;
  const childNodes = el => {
    const nodes = el.shadowRoot ? [...el.shadowRoot.childNodes] : [...el.childNodes];
    return nodes.flatMap(node => node.localName === 'slot' && typeof node.assignedNodes === 'function' ? (node.assignedNodes({ flatten: true }).length ? node.assignedNodes({ flatten: true }) : [...node.childNodes]) : [node]);
  };
  const descendants = root => {
    const out = [];
    const visit = node => {
      for (let child = node.firstElementChild; child; child = child.nextElementSibling) {
        out.push(child);
        if (child.shadowRoot) visit(child.shadowRoot);
        visit(child);
      }
    };
    if (isElement(root) && root.shadowRoot) visit(root.shadowRoot);
    visit(root);
    return out;
  };
  const uniq = list => [...new Set(list)];
  let cache = null;
  const fresh = () => { cache = { text: new Map(), name: new Map(), hidden: new Map(), role: new Map() }; };
  fresh();

  const nameFrom = el => !!(norm(el.getAttribute('aria-label')) || el.getAttribute('aria-labelledby') || norm(el.getAttribute('title')));
  const implicitRole = el => {
    const tag = el.localName;
    switch (tag) {
      case 'a': case 'area': return el.hasAttribute('href') ? 'link' : 'generic';
      case 'article': return 'article';
      case 'aside': return 'complementary';
      case 'blockquote': return 'blockquote';
      case 'button': return 'button';
      case 'caption': return 'caption';
      case 'code': return 'code';
      case 'datalist': return 'listbox';
      case 'dd': return 'definition';
      case 'del': case 's': return 'deletion';
      case 'details': return 'group';
      case 'dfn': case 'dt': return 'term';
      case 'dialog': return 'dialog';
      case 'em': return 'emphasis';
      case 'fieldset': return 'group';
      case 'figure': return 'figure';
      case 'footer': return el.parentElement && el.parentElement.closest(SCOPES) ? 'generic' : 'contentinfo';
      case 'form': return 'form';
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return 'heading';
      case 'header': return el.parentElement && el.parentElement.closest(SCOPES) ? 'generic' : 'banner';
      case 'hr': return 'separator';
      case 'html': return 'document';
      case 'iframe': return 'iframe';
      case 'img': return el.getAttribute('alt') === '' && !el.getAttribute('title') ? 'presentation' : 'img';
      case 'input': {
        const type = (el.getAttribute('type') || '').toLowerCase();
        if (['button', 'image', 'reset', 'submit', 'file'].includes(type)) return 'button';
        if (type === 'checkbox') return 'checkbox';
        if (type === 'radio') return 'radio';
        if (type === 'range') return 'slider';
        if (type === 'number') return 'spinbutton';
        if (type === 'hidden') return 'none';
        if (el.hasAttribute('list')) return 'combobox';
        if (type === 'search') return 'searchbox';
        return 'textbox';
      }
      case 'ins': return 'insertion';
      case 'li': return 'listitem';
      case 'main': return 'main';
      case 'mark': return 'mark';
      case 'math': return 'math';
      case 'menu': case 'ol': case 'ul': return 'list';
      case 'meter': return 'meter';
      case 'nav': return 'navigation';
      case 'optgroup': return 'group';
      case 'option': return 'option';
      case 'output': return 'status';
      case 'p': return 'paragraph';
      case 'progress': return 'progressbar';
      case 'search': return 'search';
      case 'section': return nameFrom(el) ? 'region' : 'generic';
      case 'select': return el.multiple || el.size > 1 ? 'listbox' : 'combobox';
      case 'strong': return 'strong';
      case 'sub': return 'subscript';
      case 'sup': return 'superscript';
      case 'table': return 'table';
      case 'tbody': case 'thead': case 'tfoot': return 'rowgroup';
      case 'td': { const table = el.closest('table'); const role = table && table.getAttribute('role'); return role === 'grid' || role === 'treegrid' ? 'gridcell' : 'cell'; }
      case 'textarea': return 'textbox';
      case 'th': { const scope = el.getAttribute('scope'); return scope === 'row' || scope === 'rowgroup' ? 'rowheader' : 'columnheader'; }
      case 'time': return 'time';
      case 'tr': return 'row';
      case 'div': case 'span': case 'b': case 'i': case 'u': case 'small': case 'label': case 'bdi': case 'bdo': case 'data': case 'hgroup': case 'address': case 'samp': case 'q': case 'pre': case 'body': return 'generic';
      default: return null;
    }
  };
  const roleOf = el => {
    if (cache.role.has(el)) return cache.role.get(el);
    let role = null;
    for (const token of (el.getAttribute('role') || '').trim().toLowerCase().split(/\s+/)) {
      if (ROLES.has(token)) { role = token === 'image' ? 'img' : token; break; }
    }
    if ((role === 'none' || role === 'presentation') && (el.tabIndex >= 0 && el.hasAttribute('tabindex'))) role = null;
    if (!role) role = implicitRole(el);
    cache.role.set(el, role);
    return role;
  };
  const styleHidden = el => {
    if (cache.hidden.has(el)) return cache.hidden.get(el);
    let hidden;
    if (el.localName === 'option' || el.localName === 'optgroup') {
      const select = el.closest('select, datalist');
      hidden = select ? (select.localName === 'datalist' ? false : styleHidden(select)) : false;
    } else {
      const style = css(el);
      if (style.display === 'contents') hidden = ![...el.childNodes].some(node => (node.nodeType === 3 && node.data.trim()) || (isElement(node) && !styleHidden(node)));
      else if (typeof el.checkVisibility === 'function' ? !el.checkVisibility() : style.display === 'none') hidden = true;
      else hidden = style.visibility !== 'visible';
    }
    cache.hidden.set(el, hidden);
    return hidden;
  };
  const ariaHidden = el => {
    for (let node = el; node; node = parentOf(node)) if (node.getAttribute && node.getAttribute('aria-hidden') === 'true') return true;
    return styleHidden(el);
  };
  const byIds = (el, attribute) => (el.getAttribute(attribute) || '').split(/\s+/).filter(Boolean).map(id => {
    const root = el.getRootNode();
    return (root.getElementById ? root.getElementById(id) : null) || el.ownerDocument.getElementById(id);
  }).filter(Boolean);
  const inputType = el => (el.getAttribute('type') || '').toLowerCase();
  const alternative = (el, context) => {
    if (context.visited.has(el)) return '';
    context.visited.add(el);
    const embedded = !context.root;
    if (embedded && !context.referenced && ariaHidden(el)) return '';
    if (!context.labelledBy) {
      const refs = byIds(el, 'aria-labelledby');
      if (refs.length) return refs.map(ref => alternative(ref, { visited: context.visited, labelledBy: true, referenced: true })).join(' ');
    }
    const tag = el.localName;
    const type = inputType(el);
    if (embedded && (tag === 'input' || tag === 'textarea' || tag === 'select')) {
      if (tag === 'select') return [...el.selectedOptions].map(option => option.textContent).join(' ');
      if (type === 'checkbox' || type === 'radio') return '';
      return el.value || '';
    }
    const label = norm(el.getAttribute('aria-label'));
    if (label) return label;
    if (tag === 'input' || tag === 'textarea' || tag === 'select') {
      if (['button', 'submit', 'reset'].includes(type)) return el.value || el.getAttribute('title') || (type === 'submit' ? 'Submit' : type === 'reset' ? 'Reset' : '');
      if (type === 'image') return el.getAttribute('alt') || el.getAttribute('value') || el.getAttribute('title') || 'Submit';
      const labels = el.labels ? [...el.labels] : [];
      if (labels.length) return labels.map(element => alternative(element, { visited: context.visited, referenced: true })).join(' ');
      return el.getAttribute('title') || el.getAttribute('placeholder') || '';
    }
    if (tag === 'img' || tag === 'area') {
      const alt = el.getAttribute('alt');
      if (alt !== null && alt.trim()) return alt;
    }
    if (tag === 'fieldset') { const legend = [...el.children].find(child => child.localName === 'legend'); if (legend) return alternative(legend, { visited: context.visited, referenced: true }); }
    if (tag === 'table') { const caption = [...el.children].find(child => child.localName === 'caption'); if (caption) return alternative(caption, { visited: context.visited, referenced: true }); }
    if (tag === 'figure') { const caption = [...el.children].find(child => child.localName === 'figcaption'); if (caption) return alternative(caption, { visited: context.visited, referenced: true }); }
    if (tag === 'svg') { const title = [...el.children].find(child => child.localName === 'title'); if (title) return title.textContent; }
    if (embedded || context.labelledBy || context.referenced || FROM_CONTENT.has(roleOf(el))) {
      let out = '';
      for (const node of childNodes(el)) {
        if (node.nodeType === 3) out += node.data;
        else if (isElement(node) && !SKIP.has(node.tagName)) {
          const part = alternative(node, { visited: context.visited });
          const display = css(node).display;
          out += display.startsWith('inline') || display === 'contents' ? part : ' ' + part + ' ';
        }
      }
      if (norm(out)) return out;
    }
    return el.getAttribute('title') || '';
  };
  const accName = el => {
    if (!cache.name.has(el)) cache.name.set(el, norm(alternative(el, { visited: new Set(), root: true })));
    return cache.name.get(el);
  };
  const fullText = el => {
    if (cache.text.has(el)) return cache.text.get(el);
    let text = '';
    if (el.localName === 'input' && ['button', 'submit', 'reset'].includes(inputType(el))) text = el.value;
    else for (const node of childNodes(el)) {
      if (node.nodeType === 3) text += node.data;
      else if (isElement(node) && !SKIP.has(node.tagName)) text += fullText(node);
    }
    cache.text.set(el, text);
    return text;
  };
  const checkedState = el => {
    if (el.localName === 'input' && (el.type === 'checkbox' || el.type === 'radio')) return el.indeterminate ? 'mixed' : el.checked;
    const value = el.getAttribute('aria-checked');
    if (value === 'mixed') return 'mixed';
    if (value === 'true') return true;
    if (value === 'false') return false;
    return undefined;
  };
  const disabled = el => {
    if (['button', 'input', 'select', 'textarea', 'option', 'optgroup', 'fieldset'].includes(el.localName) && el.matches(':disabled')) return true;
    for (let node = el; node; node = parentOf(node)) if (node.getAttribute && node.getAttribute('aria-disabled') === 'true') return true;
    return false;
  };
  const level = el => {
    const match = /^h([1-6])$/.exec(el.localName);
    const aria = Number(el.getAttribute('aria-level'));
    return aria || (match ? Number(match[1]) : undefined);
  };
  const pressed = el => { const value = el.getAttribute('aria-pressed'); return value === 'mixed' ? 'mixed' : value === 'true'; };
  const expanded = el => { const value = el.getAttribute('aria-expanded'); if (el.localName === 'details') return el.open; if (el.localName === 'summary' && el.parentElement && el.parentElement.localName === 'details') return el.parentElement.open; return value === null ? undefined : value === 'true'; };
  const selected = el => el.localName === 'option' ? el.selected : el.getAttribute('aria-selected') === 'true';

  const visible = el => {
    if (el.localName === 'option' || el.localName === 'optgroup') { const select = el.closest('select'); return select ? visible(select) : false; }
    const rect = el.getBoundingClientRect();
    if (!(rect.width > 0 && rect.height > 0)) {
      if (css(el).display === 'contents') return [...el.children].some(visible);
      return false;
    }
    return !styleHidden(el);
  };
  const editable = el => {
    if (disabled(el)) return false;
    if (el.isContentEditable) return true;
    if (el.localName === 'textarea') return !el.readOnly;
    if (el.localName === 'input') return !el.readOnly && (TEXT_INPUTS.has(inputType(el)) || VALUE_INPUTS.has(inputType(el)));
    return ['textbox', 'searchbox', 'combobox', 'spinbutton'].includes(roleOf(el)) && el.getAttribute('aria-readonly') !== 'true';
  };

  const selectRole = (root, step) => {
    const wanted = String(step.role).toLowerCase();
    const name = step.name === undefined ? null : matcher(step.name, step.exact);
    return descendants(root).filter(el => {
      if (roleOf(el) !== wanted) return false;
      if (step.checked !== undefined && (checkedState(el) ?? false) !== step.checked) return false;
      if (step.disabled !== undefined && disabled(el) !== step.disabled) return false;
      if (step.expanded !== undefined && (expanded(el) ?? false) !== step.expanded) return false;
      if (step.level !== undefined && level(el) !== step.level) return false;
      if (step.pressed !== undefined && pressed(el) !== step.pressed) return false;
      if (step.selected !== undefined && selected(el) !== step.selected) return false;
      if (name && !name(accName(el))) return false;
      if (!step.includeHidden && ariaHidden(el)) return false;
      return true;
    });
  };
  const selectText = (root, pattern, exact) => {
    const test = matcher(pattern, exact);
    const prune = typeof pattern === 'string' && !exact;
    const out = [];
    const visit = node => {
      for (let child = node.firstElementChild; child; child = child.nextElementSibling) {
        if (SKIP.has(child.tagName)) continue;
        const matches = test(fullText(child));
        if (prune && !matches) continue;
        const before = out.length;
        if (child.shadowRoot) visit(child.shadowRoot);
        visit(child);
        if (matches && out.length === before) out.push(child);
      }
    };
    visit(isElement(root) && root.shadowRoot ? root.shadowRoot : root);
    if (isElement(root) && root.shadowRoot) visit(root);
    return out;
  };
  const labelTexts = el => {
    const texts = [];
    const aria = el.getAttribute('aria-label');
    if (aria) texts.push(aria);
    for (const ref of byIds(el, 'aria-labelledby')) texts.push(alternative(ref, { visited: new Set([el]), referenced: true, labelledBy: true }));
    if (el.labels) for (const label of el.labels) texts.push(alternative(label, { visited: new Set([el]), referenced: true }));
    return texts;
  };
  const selectLabel = (root, pattern, exact) => {
    const test = matcher(pattern, exact);
    return descendants(root).filter(el => (el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby') || (el.labels && el.labels.length)) && labelTexts(el).some(test));
  };
  const selectAttribute = (root, attribute, pattern, exact) => {
    const test = attribute === 'data-testid' && typeof pattern === 'string' ? text => text === pattern : matcher(pattern, exact);
    return descendants(root).filter(el => el.hasAttribute(attribute) && test(el.getAttribute(attribute)));
  };
  const scopesOf = root => [root, ...descendants(root).filter(el => el.shadowRoot).map(el => el.shadowRoot)];
  const nativeQuery = (root, selector) => {
    const out = [];
    for (const scope of scopesOf(root)) for (const el of scope.querySelectorAll(selector)) out.push(el);
    return uniq(out);
  };
  const splitTop = (text, separators) => {
    const parts = [];
    let depth = 0;
    let quote = null;
    let start = 0;
    for (let i = 0; i < text.length; i++) {
      const char = text[i];
      if (quote) { if (char === '\\') i++; else if (char === quote) quote = null; continue; }
      if (char === '"' || char === "'") { quote = char; continue; }
      if (char === '(' || char === '[') depth++;
      else if (char === ')' || char === ']') depth--;
      else if (depth === 0) {
        const separator = separators.find(candidate => text.startsWith(candidate, i));
        if (separator) { parts.push(text.slice(start, i)); start = i + separator.length; i += separator.length - 1; }
      }
    }
    parts.push(text.slice(start));
    return parts;
  };
  const PSEUDO = /:(has-text|text-is|text-matches|text|visible)(?=\(|$|[\s>+~:.\[#,])/;
  const unquote = text => {
    const trimmed = text.trim();
    if ((trimmed.startsWith('"') && trimmed.endsWith('"')) || (trimmed.startsWith("'") && trimmed.endsWith("'"))) return trimmed.slice(1, -1).replace(/\\(.)/g, '$1');
    return trimmed;
  };
  const pseudoArgument = (text, at) => {
    if (text[at] !== '(') return { arg: null, end: at };
    let depth = 0;
    let quote = null;
    for (let i = at; i < text.length; i++) {
      const char = text[i];
      if (quote) { if (char === '\\') i++; else if (char === quote) quote = null; continue; }
      if (char === '"' || char === "'") quote = char;
      else if (char === '(') depth++;
      else if (char === ')' && --depth === 0) return { arg: text.slice(at + 1, i), end: i + 1 };
    }
    return { arg: text.slice(at + 1), end: text.length };
  };
  const deepest = (elements, test) => elements.filter(el => test(fullText(el)) && ![...el.children].some(child => test(fullText(child))));
  const applyPseudo = (elements, name, arg) => {
    if (name === 'visible') return elements.filter(visible);
    if (name === 'has-text') return elements.filter(el => matcher(unquote(arg ?? ''), false)(fullText(el)));
    if (name === 'text') return deepest(elements, matcher(unquote(arg ?? ''), false));
    if (name === 'text-is') return deepest(elements, matcher(unquote(arg ?? ''), true));
    const [source, flags] = splitTop(arg ?? '', [',']).map(unquote);
    return deepest(elements, matcher({ source, flags: flags ?? '' }));
  };
  const COMBINATOR = /^\s*([>+~])\s*|^\s+/;
  const cssQuery = (root, selector) => {
    const parts = splitTop(selector, [',']);
    if (parts.length > 1) return uniq(parts.flatMap(part => cssQuery(root, part.trim())));
    if (!PSEUDO.test(selector)) return nativeQuery(root, selector);
    let current = [root];
    let rest = selector.trim();
    let combinator = ' ';
    let first = true;
    while (rest) {
      let compound = '';
      const pseudos = [];
      while (rest && !(compound && COMBINATOR.test(rest))) {
        const match = PSEUDO.exec(rest);
        const boundary = rest.search(/\s|[>+~](?![^\[]*\])/);
        if (match && (boundary < 0 || match.index < boundary)) {
          compound += rest.slice(0, match.index);
          const { arg, end } = pseudoArgument(rest, match.index + match[0].length);
          pseudos.push([match[1], arg]);
          rest = rest.slice(end);
        } else {
          const cut = boundary < 0 ? rest.length : boundary;
          compound += rest.slice(0, cut);
          rest = rest.slice(cut);
          break;
        }
      }
      const native = compound.trim() || '*';
      let next;
      if (first && !isElement(root)) next = nativeQuery(root, native);
      else if (combinator === ' ') next = uniq(current.flatMap(el => nativeQuery(el, native)));
      else if (combinator === '>') next = uniq(current.flatMap(el => [...(el.shadowRoot && !isElement(el) ? el.children : el.children)].filter(child => child.matches(native))));
      else if (combinator === '+') next = uniq(current.map(el => el.nextElementSibling).filter(sibling => sibling && sibling.matches(native)));
      else {
        next = [];
        for (const el of current) for (let sibling = el.nextElementSibling; sibling; sibling = sibling.nextElementSibling) if (sibling.matches(native)) next.push(sibling);
        next = uniq(next);
      }
      for (const [name, arg] of pseudos) next = applyPseudo(next, name, arg);
      current = next;
      first = false;
      const found = COMBINATOR.exec(rest);
      if (found) { combinator = found[1] || ' '; rest = rest.slice(found[0].length); }
    }
    return current;
  };
  const xpathQuery = (root, expression) => {
    const doc = root.nodeType === 9 ? root : root.ownerDocument;
    const result = doc.evaluate(expression, root, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
    const out = [];
    for (let i = 0; i < result.snapshotLength; i++) if (isElement(result.snapshotItem(i))) out.push(result.snapshotItem(i));
    return out;
  };
  const select = (root, step) => {
    switch (step.kind) {
      case 'css': return cssQuery(root, step.selector);
      case 'xpath': return xpathQuery(root, step.selector);
      case 'role': return selectRole(root, step);
      case 'text': return selectText(root, step.text, step.exact);
      case 'label': return selectLabel(root, step.text, step.exact);
      case 'placeholder': return selectAttribute(root, 'placeholder', step.text, step.exact);
      case 'alt': return selectAttribute(root, 'alt', step.text, step.exact);
      case 'title': return selectAttribute(root, 'title', step.text, step.exact);
      case 'testid': return selectAttribute(root, 'data-testid', step.text, true);
      default: throw new Error('unknown selector kind ' + step.kind);
    }
  };
  const describe = el => {
    const attributes = [];
    for (const name of ['id', 'class', 'type', 'role', 'name', 'aria-label', 'placeholder', 'data-testid', 'href', 'title']) {
      const value = el.getAttribute(name);
      if (value !== null) attributes.push(name + '="' + (value.length > 40 ? value.slice(0, 40) + '…' : value) + '"');
    }
    const text = norm(fullText(el));
    const role = roleOf(el);
    const name = role && role !== 'generic' && role !== 'none' ? accName(el) : '';
    const aka = name ? " aka getByRole('" + role + "', { name: " + JSON.stringify(name.length > 50 ? name.slice(0, 50) : name) + ' })' : text ? ' aka getByText(' + JSON.stringify(text.slice(0, 50)) + ')' : '';
    return '<' + el.localName + (attributes.length ? ' ' + attributes.join(' ') : '') + '>' + (text.length > 50 ? text.slice(0, 50) + '…' : text) + '</' + el.localName + '>' + aka;
  };
  class Failure extends Error {}
  const strict = (elements, description) => {
    if (elements.length > 1) throw new Failure('strict mode violation: ' + description + ' resolved to ' + elements.length + ' elements:\n' + elements.slice(0, 10).map((el, i) => '    ' + (i + 1) + ') ' + describe(el)).join('\n') + (elements.length > 10 ? '\n    …' : '') + '\nUse .first(), .nth(i), .filter() or a more specific locator.');
  };
  const query = (chain, roots) => {
    let current = roots ?? [document];
    for (const step of chain) {
      switch (step.kind) {
        case 'nth': {
          const index = step.n < 0 ? current.length + step.n : step.n;
          current = current[index] ? [current[index]] : [];
          break;
        }
        case 'filter':
          current = current.filter(el => {
            if (step.hasText !== undefined && !matcher(step.hasText, false)(fullText(el))) return false;
            if (step.hasNotText !== undefined && matcher(step.hasNotText, false)(fullText(el))) return false;
            if (step.has && !query(step.has, [el]).length) return false;
            if (step.hasNot && query(step.hasNot, [el]).length) return false;
            if (step.visible !== undefined && visible(el) !== step.visible) return false;
            return true;
          });
          break;
        case 'and': {
          const other = new Set(query(step.chain));
          current = current.filter(el => other.has(el));
          break;
        }
        case 'or': {
          const mine = new Set(current);
          const other = query(step.chain);
          const order = new Map(descendants(document).map((el, i) => [el, i]));
          current = uniq([...current, ...other.filter(el => !mine.has(el))]).sort((a, b) => (order.get(a) ?? 0) - (order.get(b) ?? 0));
          break;
        }
        case 'frame': {
          const frames = uniq(current.flatMap(root => select(root, step.selector))).filter(el => el.localName === 'iframe' || el.localName === 'frame');
          strict(frames, step.description);
          if (!frames.length) { current = []; break; }
          let doc = null;
          try { doc = frames[0].contentDocument; } catch {}
          if (!doc) throw new Failure('frame ' + step.description + ' is a cross-origin frame; its content can be read with domSnapshot() but not located from the page');
          current = [doc];
          break;
        }
        default:
          current = uniq(current.flatMap(root => select(root, step)));
      }
    }
    return current.filter(isElement);
  };
  const viewportOf = el => {
    const view = el.ownerDocument.defaultView;
    return { width: view.innerWidth, height: view.innerHeight };
  };
  const toTop = (el, x, y) => {
    let view = el.ownerDocument.defaultView;
    while (view && view !== window && view.frameElement) {
      const frame = view.frameElement;
      const rect = frame.getBoundingClientRect();
      const style = css(frame);
      x += rect.left + frame.clientLeft + parseFloat(style.paddingLeft || '0');
      y += rect.top + frame.clientTop + parseFloat(style.paddingTop || '0');
      view = frame.ownerDocument.defaultView;
    }
    return [x, y];
  };
  const point = (el, position) => {
    const rect = el.getBoundingClientRect();
    if (position) return [rect.left + position.x, rect.top + position.y];
    const { width, height } = viewportOf(el);
    const rects = [...el.getClientRects()].filter(r => r.width > 0 && r.height > 0);
    const pick = rects.find(r => r.right > 0 && r.bottom > 0 && r.left < width && r.top < height) || rects[0] || rect;
    const left = Math.max(pick.left, 0);
    const right = Math.min(pick.right, width);
    const top = Math.max(pick.top, 0);
    const bottom = Math.min(pick.bottom, height);
    return right > left && bottom > top ? [(left + right) / 2, (top + bottom) / 2] : [pick.left + pick.width / 2, pick.top + pick.height / 2];
  };
  const inView = el => {
    const rect = el.getBoundingClientRect();
    const { width, height } = viewportOf(el);
    return rect.top >= 0 && rect.left >= 0 && rect.bottom <= height && rect.right <= width;
  };
  const hits = (el, x, y) => {
    const hit = el.ownerDocument.elementFromPoint(x, y);
    if (!hit) return 'no element is at the click point';
    let node = hit;
    for (let deep = hit; deep && deep.shadowRoot;) {
      const inner = deep.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === deep) break;
      deep = inner;
      node = inner;
    }
    for (let walk = node; walk; walk = parentOf(walk)) if (walk === el) return null;
    if (el.localName === 'label' || (el.labels && [...el.labels].some(label => label.contains(node)))) return null;
    if (node.localName === 'label' && node.control === el) return null;
    return describe(node) + ' intercepts pointer events';
  };
  const waitUntil = (attempt, timeoutMs) => new Promise(resolve => {
    let done = false;
    let last = null;
    let observer;
    let poll;
    let timer;
    const finish = value => {
      if (done) return;
      done = true;
      observer && observer.disconnect();
      clearInterval(poll);
      clearTimeout(timer);
      resolve(value);
    };
    const run = () => {
      if (done) return;
      fresh();
      try {
        const outcome = attempt();
        if (outcome.done) finish({ value: outcome.value });
        else last = outcome.reason;
      } catch (error) {
        finish({ error: error instanceof Failure ? error.message : String(error && error.message || error) });
      }
    };
    run();
    if (done || timeoutMs <= 0) { finish({ timeout: true, reason: last }); return; }
    observer = new MutationObserver(run);
    observer.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    poll = setInterval(run, 100);
    timer = setTimeout(() => finish({ timeout: true, reason: last }), timeoutMs);
  });
  const one = (chain, description) => {
    const found = query(chain);
    strict(found, description);
    return found[0];
  };
  const actionable = (req) => () => {
    const found = query(req.chain);
    strict(found, req.description);
    const el = found[0];
    if (!el) return { reason: 'waiting for ' + req.description };
    if (req.precheck !== undefined && checkedState(el) === req.precheck) return { done: true, value: { skip: true } };
    if (req.force && req.fill === undefined) {
      if (req.scroll !== false && !inView(el)) el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
      if (req.focus) el.focus({ preventScroll: true });
      const [x, y] = toTop(el, ...point(el, req.position));
      return { done: true, value: { x, y } };
    }
    if (!req.force && !visible(el)) return { reason: describe(el) + ' is not visible' };
    if (req.enabled && disabled(el)) return { reason: describe(el) + ' is not enabled' };
    if (req.editable && !editable(el)) return { reason: describe(el) + ' is not editable' };
    if (req.scroll !== false && !inView(el)) el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
    const [lx, ly] = point(el, req.position);
    if (req.hit && !req.force) {
      const blocked = hits(el, lx, ly);
      if (blocked) return { reason: blocked };
    }
    const [x, y] = toTop(el, lx, ly);
    const value = { x, y };
    if (req.focus) el.focus({ preventScroll: true });
    if (req.fill !== undefined) {
      const type = inputType(el);
      if (el.localName === 'input' && VALUE_INPUTS.has(type)) {
        el.focus({ preventScroll: true });
        const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
        setter.call(el, req.fill);
        el.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
        el.dispatchEvent(new Event('change', { bubbles: true }));
        value.done = true;
      } else {
        el.focus({ preventScroll: true });
        if (typeof el.select === 'function' && (el.localName === 'input' || el.localName === 'textarea')) el.select();
        else {
          const range = el.ownerDocument.createRange();
          range.selectNodeContents(el);
          const selection = el.ownerDocument.defaultView.getSelection();
          selection.removeAllRanges();
          selection.addRange(range);
        }
        if (req.fill) value.text = req.fill;
        else value.clear = true;
      }
    }
    return { done: true, value };
  };
  const optionMatches = (option, wanted, index) => {
    if (wanted && typeof wanted === 'object') {
      if (wanted.index !== undefined && wanted.index !== index) return false;
      if (wanted.value !== undefined && option.value !== wanted.value) return false;
      if (wanted.label !== undefined && option.label !== wanted.label && norm(option.textContent) !== wanted.label) return false;
      return true;
    }
    return option.value === wanted || option.label === wanted || norm(option.textContent) === wanted;
  };
  const read = (el, req) => {
    switch (req.what) {
      case 'textContent': return el.textContent;
      case 'innerText': return el.innerText;
      case 'innerHTML': return el.innerHTML;
      case 'inputValue': {
        if (!['input', 'textarea', 'select'].includes(el.localName)) throw new Failure('Node is not an <input>, <textarea> or <select> element');
        return el.value;
      }
      case 'attribute': return el.getAttribute(req.name);
      case 'box': {
        const rect = el.getBoundingClientRect();
        const [x, y] = toTop(el, rect.left, rect.top);
        return rect.width || rect.height ? { x, y, width: rect.width, height: rect.height } : null;
      }
      case 'media': {
        const source = el.currentSrc || el.src || el.href || (el.querySelector && el.querySelector('source[src], img[src], video[src], a[href]')?.src) || el.getAttribute('data-src');
        if (!source) throw new Failure(describe(el) + ' has no media URL (src or href)');
        return new URL(source, el.baseURI).href;
      }
      case 'aria': return render(build(el, true), 0);
      default: throw new Failure('unknown read ' + req.what);
    }
  };
  const stateOf = (el, what) => {
    switch (what) {
      case 'visible': return !!el && visible(el);
      case 'hidden': return !el || !visible(el);
      case 'enabled': return !!el && !disabled(el);
      case 'disabled': return !!el && disabled(el);
      case 'editable': return !!el && editable(el);
      case 'checked': {
        if (!el) return false;
        const state = checkedState(el);
        if (state === undefined) throw new Failure('Not a checkbox or radio button: ' + describe(el));
        return state === true;
      }
      default: throw new Failure('unknown state ' + what);
    }
  };

  const ATTRIBUTE_ROLES = new Set(['checkbox', 'radio', 'switch', 'menuitemcheckbox', 'menuitemradio', 'button', 'tab', 'option', 'treeitem', 'row', 'gridcell']);
  const NAMELESS_TEXT = new Set(['paragraph', 'listitem', 'cell', 'gridcell', 'columnheader', 'rowheader', 'heading', 'link', 'button', 'option', 'tab', 'menuitem', 'generic', 'strong', 'emphasis', 'code', 'term', 'definition', 'blockquote', 'caption', 'status', 'alert', 'time', 'mark', 'deletion', 'insertion', 'subscript', 'superscript']);
  const build = (el, root) => {
    if (SKIP.has(el.tagName)) return [];
    if (el.getAttribute('aria-hidden') === 'true') return [];
    const style = css(el);
    if (style.display === 'none' || (typeof el.checkVisibility === 'function' && style.display !== 'contents' && !el.checkVisibility() && el.localName !== 'option' && el.localName !== 'optgroup')) return [];
    const selfHidden = style.visibility !== 'visible' && el.localName !== 'option';
    const block = !(style.display.startsWith('inline') || style.display === 'contents');
    let children = [];
    if (el.localName === 'iframe' || el.localName === 'frame') {
      let doc = null;
      try { doc = el.contentDocument; } catch {}
      const node = { role: 'iframe', name: norm(el.getAttribute('title') || el.getAttribute('name') || ''), attributes: [], children: [] };
      if (doc && doc.body) node.children = build(doc.body, false);
      else { node.frame = frames.length; frames.push(el.src || el.getAttribute('src') || ''); }
      return [node];
    }
    for (const node of childNodes(el)) {
      if (node.nodeType === 3) { if (node.data.trim()) children.push({ text: node.data }); }
      else if (isElement(node)) children.push(...build(node, false));
    }
    let role = roleOf(el);
    if (el.localName === 'svg' && !el.getAttribute('role')) role = null;
    const named = role === 'generic' || role === null || role === 'none' || role === 'presentation' ? norm(el.getAttribute('aria-label')) : '';
    if (selfHidden || ((role === null || role === 'generic' || role === 'none' || role === 'presentation') && !named) || role === 'document') {
      return block && !root ? [{ gap: true }, ...children, { gap: true }] : children;
    }
    const node = { role, name: accName(el), attributes: [], children };
    const checked = ATTRIBUTE_ROLES.has(role) ? checkedState(el) : undefined;
    if (checked === 'mixed') node.attributes.push('checked=mixed');
    else if (checked === true) node.attributes.push('checked');
    if (disabled(el) && ['button', 'textbox', 'searchbox', 'combobox', 'checkbox', 'radio', 'option', 'link', 'menuitem', 'tab', 'slider', 'spinbutton', 'switch', 'listbox'].includes(role)) node.attributes.push('disabled');
    const open = expanded(el);
    if (open === true) node.attributes.push('expanded');
    if (role === 'heading') node.attributes.push('level=' + level(el));
    if (el.getAttribute('aria-pressed') !== null && pressed(el)) node.attributes.push(pressed(el) === 'mixed' ? 'pressed=mixed' : 'pressed');
    if ((role === 'option' || role === 'tab' || role === 'treeitem' || role === 'row') && selected(el)) node.attributes.push('selected');
    if (role === 'link' && el.getAttribute('href') !== null) node.url = el.getAttribute('href');
    if (['textbox', 'searchbox', 'combobox', 'spinbutton', 'slider'].includes(role)) {
      const value = el.localName === 'select' ? [...el.selectedOptions].map(option => option.textContent).join(', ') : el.value !== undefined && el.localName !== 'li' ? el.value : el.getAttribute('aria-valuenow') || (el.isContentEditable ? el.innerText : '');
      if (el.localName === 'input' || el.localName === 'textarea' || el.isContentEditable) node.children = [];
      if (norm(value)) node.value = norm(value);
    }
    if (role === 'img') node.children = [];
    return [node];
  };
  const merge = items => {
    const out = [];
    let text = null;
    for (const item of items) {
      if (item.gap) { if (text !== null) { out.push({ text }); text = null; } continue; }
      if (item.text !== undefined) { text = text === null ? item.text : text + item.text; continue; }
      if (text !== null) { out.push({ text }); text = null; }
      out.push(item);
    }
    if (text !== null) out.push({ text });
    return out.map(item => item.text !== undefined ? { text: norm(item.text) } : item).filter(item => item.text === undefined || item.text);
  };
  let frames = [];
  const clip = (text, limit) => text.length > limit ? text.slice(0, limit) + '…' : text;
  const render = (items, depth) => {
    const lines = [];
    const indent = '  '.repeat(depth);
    for (const item of merge(items)) {
      if (item.text !== undefined) { lines.push(indent + '- text: ' + clip(item.text, 2000)); continue; }
      let children = merge(item.children);
      if (children.length && children.every(child => child.text !== undefined) && norm(children.map(child => child.text).join(' ')) === item.name) children = [];
      let line = indent + '- ' + item.role + (item.name ? ' ' + JSON.stringify(clip(item.name, 300)) : '') + (item.attributes ?? []).map(attribute => ' [' + attribute + ']').join('');
      if (item.frame !== undefined) { lines.push(line + ' [frame=' + item.frame + ']:'); lines.push(indent + '  ' + '\u0000frame:' + item.frame); continue; }
      const inline = item.value ?? (children.length === 1 && children[0].text !== undefined && !item.url && NAMELESS_TEXT.has(item.role) ? children[0].text : null);
      if (inline !== null && !item.url) { lines.push(line + ': ' + clip(inline, 2000)); continue; }
      if (!children.length && !item.url && inline === null) { lines.push(line); continue; }
      lines.push(line + ':');
      if (item.url !== undefined) lines.push(indent + '  - /url: ' + clip(item.url, 500));
      if (inline !== null) lines.push(indent + '  - text: ' + clip(inline, 2000));
      else lines.push(...render(children, depth + 1).split('\n').filter(Boolean));
    }
    return lines.join('\n');
  };
  const snapshot = () => {
    fresh();
    frames = [];
    const body = document.body || document.documentElement;
    const text = body ? render(build(body, true), 0) : '';
    return { text, frames, url: location.href, title: document.title };
  };

  const run = async req => {
    fresh();
    try {
      switch (req.op) {
        case 'count': return { value: query(req.chain).length };
        case 'texts': return { value: query(req.chain).map(el => req.inner ? el.innerText : el.textContent) };
        case 'snapshot': return { value: snapshot() };
        case 'state': {
          const found = query(req.chain);
          strict(found, req.description);
          return { value: stateOf(found[0], req.what) };
        }
        case 'act': {
          const outcome = await waitUntil(actionable(req), req.timeoutMs);
          if (outcome.timeout) return { error: 'Timeout ' + req.timeoutMs + 'ms exceeded waiting for ' + req.description + ' to be ' + req.waiting + (outcome.reason ? '\n  - ' + outcome.reason : '') };
          return outcome;
        }
        case 'read': {
          const outcome = await waitUntil(() => {
            const found = query(req.chain);
            strict(found, req.description);
            return found[0] ? { done: true, value: read(found[0], req) } : { reason: 'waiting for ' + req.description };
          }, req.timeoutMs);
          if (outcome.timeout) return { error: 'Timeout ' + req.timeoutMs + 'ms exceeded waiting for ' + req.description };
          return outcome;
        }
        case 'wait': {
          const outcome = await waitUntil(() => {
            const found = query(req.chain);
            if (req.state !== 'detached') strict(found, req.description);
            const el = found[0];
            const done = req.state === 'attached' ? !!el : req.state === 'detached' ? !el : req.state === 'hidden' ? !el || !visible(el) : !!el && visible(el);
            return done ? { done: true, value: true } : { reason: el ? describe(el) + ' is ' + (visible(el) ? 'visible' : 'not visible') : 'no element matches' };
          }, req.timeoutMs);
          if (outcome.timeout) return { error: 'Timeout ' + req.timeoutMs + 'ms exceeded waiting for ' + req.description + ' to be ' + req.state + (outcome.reason ? '\n  - ' + outcome.reason : '') };
          return outcome;
        }
        case 'select': {
          const outcome = await waitUntil(() => {
            const found = query(req.chain);
            strict(found, req.description);
            const el = found[0];
            if (!el) return { reason: 'waiting for ' + req.description };
            if (el.localName !== 'select') throw new Failure('Element is not a <select> element: ' + describe(el));
            if (disabled(el)) return { reason: describe(el) + ' is not enabled' };
            const options = [...el.options];
            const chosen = [];
            for (const wanted of req.values) {
              const option = options.find((candidate, i) => optionMatches(candidate, wanted, i));
              if (!option) return { reason: 'did not find some options' };
              chosen.push(option);
              if (!el.multiple) break;
            }
            for (const option of options) option.selected = chosen.includes(option);
            el.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
            el.dispatchEvent(new Event('change', { bubbles: true }));
            return { done: true, value: chosen.map(option => option.value) };
          }, req.timeoutMs);
          if (outcome.timeout) return { error: 'Timeout ' + req.timeoutMs + 'ms exceeded selecting options in ' + req.description + (outcome.reason ? '\n  - ' + outcome.reason : '') };
          return outcome;
        }
        case 'setChecked': {
          const found = query(req.chain);
          strict(found, req.description);
          return { value: found[0] ? checkedState(found[0]) : null };
        }
        default: return { error: 'unknown op ' + req.op };
      }
    } catch (error) {
      return { error: error instanceof Failure ? error.message : String(error && error.stack || error) };
    }
  };
  const resolveOne = async (chain, description, timeoutMs) => {
    const outcome = await waitUntil(() => {
      const found = query(chain);
      strict(found, description);
      return found[0] ? { done: true, value: found[0] } : { reason: 'waiting for ' + description };
    }, timeoutMs);
    if (outcome.error) throw new Error(outcome.error);
    if (outcome.timeout) throw new Error('Timeout ' + timeoutMs + 'ms exceeded waiting for ' + description);
    return outcome.value;
  };
  const resolveAll = chain => { fresh(); return query(chain); };
  const api = { run, resolveOne, resolveAll, snapshot, version: 1 };
  Object.defineProperty(globalThis, KEY, { value: api, configurable: true });
  return api;
}
const PROTOCOL = 2;
const HELLO_TIMEOUT_MS = 1500;
const READ_SETTLE = { quietMs: 100, timeoutMs: PAGE_SETTLE_LIMIT_MS };
const ACTION_SETTLE = { minMs: 40, quietMs: 100, timeoutMs: PAGE_SETTLE_LIMIT_MS };
const NAV_SETTLE = { quietMs: 150, load: true, timeoutMs: NAVIGATION_TIMEOUT_MS };
const ENGINE = '(' + nanocodeEngine.toString() + ')()';
const ENGINE_RUN = req => "(()=>{const e=globalThis[Symbol.for('nanocode.engine')];return e?e.run(" + JSON.stringify(req) + "):{missing:true}})()";
const TRANSIENT = /Execution context was destroyed|Cannot find context|Inspected target navigated|Promise was collected|context with specified id|Target closed/;
const exceptionText = details => {
  const text = details.exception?.description ?? details.exception?.value ?? details.text ?? 'Page script failed';
  return String(text).split('\n    at ')[0];
};
class Relay {
  #path;
  #server = null;
  #listening = null;
  #connections = new Set();
  #waiters = new Set();
  #next = 0;
  #ids = new Map();
  #count = 0;

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
      if (live) throw new Error('Another cua_repl session is serving the browser on ' + this.#path + '; only one session can drive the browser at a time');
      fs.unlinkSync(this.#path);
    }
    const server = net.createServer(socket => this.#attach(socket));
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(this.#path, resolve); });
    fs.chmodSync(this.#path, 0o600);
    this.#server = server;
  }

  #attach(socket) {
    const connection = { socket, buffer: '', pending: new Map(), info: null, id: null, name: null };
    this.#connections.add(connection);
    socket.setEncoding('utf8');
    const identify = info => {
      if (connection.id || !this.#connections.has(connection)) return;
      connection.info = info;
      const base = info?.protocol === PROTOCOL ? info.browser?.name || 'Chrome' : 'outdated extension';
      let name = base;
      for (let n = 2; [...this.#connections].some(other => other !== connection && other.name === name); n++) name = base + ' ' + n;
      connection.name = name;
      if (!this.#ids.has(name)) this.#ids.set(name, String(++this.#count));
      connection.id = this.#ids.get(name);
      for (const waiter of this.#waiters) waiter();
      this.#waiters.clear();
    };
    const timer = setTimeout(() => identify(null), HELLO_TIMEOUT_MS);
    socket.write(JSON.stringify({ type: 'hello' }) + '\n');
    socket.on('data', data => {
      connection.buffer += data;
      let newline;
      while ((newline = connection.buffer.indexOf('\n')) >= 0) {
        const line = connection.buffer.slice(0, newline);
        connection.buffer = connection.buffer.slice(newline + 1);
        let message;
        try { message = JSON.parse(line); } catch { continue; }
        if (message.type === 'hello') { clearTimeout(timer); identify(message); continue; }
        const pending = connection.pending.get(message.id);
        if (message.type !== 'result' || !pending) continue;
        connection.pending.delete(message.id);
        if (message.ok) pending.resolve(message.result);
        else pending.reject(new Error(message.error ?? 'browser step failed'));
      }
    });
    const closed = () => {
      clearTimeout(timer);
      if (!this.#connections.delete(connection)) return;
      for (const pending of connection.pending.values()) pending.reject(new Error('The browser disconnected during this step; whether it took effect is unknown'));
      connection.pending.clear();
    };
    socket.on('close', closed);
    socket.on('error', closed);
  }

  list() { return [...this.#connections].filter(connection => connection.id && connection.info?.protocol === PROTOCOL).sort((a, b) => Number(a.id) - Number(b.id)); }
  outdated() { return [...this.#connections].filter(connection => connection.id && connection.info?.protocol !== PROTOCOL); }

  async connections(wait = BROWSER_WAIT_MS) {
    await this.listen();
    if (this.list().length) return this.list();
    await new Promise(resolve => {
      const done = () => { clearTimeout(timer); this.#waiters.delete(done); resolve(); };
      const timer = setTimeout(done, wait);
      this.#waiters.add(done);
    });
    return this.list();
  }

  async step(connection, input) {
    if (!this.#connections.has(connection)) throw new Error(connection.name + ' disconnected; get the browser again with cua.getBrowser() or agent.browsers.get()');
    if (connection.info?.protocol !== PROTOCOL) throw new Error('The nanocode extension in ' + connection.name + ' is outdated: run "nanocode cua install", then reload the extension in chrome://extensions');
    const { session, turn } = callIdentity();
    const id = ++this.#next;
    return new Promise((resolve, reject) => {
      connection.pending.set(id, { resolve, reject });
      connection.socket.write(JSON.stringify({ type: 'call', id, session, turn, input }) + '\n');
    });
  }

  broadcast(message) {
    for (const connection of this.#connections) connection.socket.write(JSON.stringify(message) + '\n');
  }

  async close() {
    for (const connection of [...this.#connections]) {
      const socket = connection.socket;
      await new Promise(resolve => { socket.end(resolve); setTimeout(resolve, 1000); });
      socket.destroy();
    }
    this.#server?.close();
    if (this.#server && this.#path) { try { fs.unlinkSync(this.#path); } catch {} }
  }
}

const relay = new Relay(env.CUA_REPL_BROWSER_SOCKET || '');
const UNSUPPORTED_BROWSERS = { iab: 'the in-app browser', mcpapps: 'MCP Apps' };
const GENERIC_BROWSERS = new Set(['', 'chrome', 'chromium', 'default', 'browser', 'extension', 'user', 'google chrome']);
const browserInfo = connection => ({ id: connection.id, name: connection.name, family: 'chromium', type: 'extension', ...(connection.info?.extension ? { metadata: { extensionVersion: connection.info.extension } } : {}) });
const describeBrowsers = list => list.map(connection => '"' + connection.id + '" (' + connection.name + ')').join(', ');
const resolveBrowser = async value => {
  const wanted = String(value ?? '').trim();
  const lower = wanted.toLowerCase();
  if (!relay.available) throw new Error('No browser is configured for this runtime; set CUA_REPL_BROWSER_SOCKET (nanocode cua install)');
  if (UNSUPPORTED_BROWSERS[lower]) {
    const list = relay.list();
    throw new Error('Browser "' + wanted + '" (' + UNSUPPORTED_BROWSERS[lower] + ') is not available in this runtime; use the user\'s own browser' + (list.length ? ': ' + describeBrowsers(list) : ' through the nanocode extension'));
  }
  const list = await relay.connections();
  if (!list.length && relay.outdated().length) throw new Error('The nanocode extension in the connected browser is outdated: run "nanocode cua install", then reload the extension in chrome://extensions');
  if (!list.length) throw new Error('No browser is connected: open Chrome (or another Chromium browser) with the nanocode extension loaded (nanocode cua install sets it up, then Load unpacked in chrome://extensions)');
  const found = list.find(connection => connection.id === wanted)
    ?? list.find(connection => connection.name.toLowerCase() === lower)
    ?? list.find(connection => lower.includes(connection.name.toLowerCase()) || (lower && connection.name.toLowerCase().startsWith(lower)))
    ?? (GENERIC_BROWSERS.has(lower) ? list.find(connection => connection.name === 'Chrome') ?? list[0] : null);
  if (!found) throw new Error('Browser ' + wanted + ' is not available; connected browsers: ' + describeBrowsers(list));
  return found;
};

const tabs = new Map();
const sessionNames = new Map();
const closeTabs = async selected => {
  for (const [id, tab] of [...tabs]) {
    if (!selected(tab)) continue;
    if (tab.kept || tab.claimed) continue;
    tabs.delete(id);
    await tab.close().catch(() => {});
  }
};

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
  if (!entry && /^kp_\d$/.test(lower)) entry = [lower.slice(3), 'Numpad' + lower.slice(3), 96 + Number(lower.slice(3)), lower.slice(3)];
  if (!entry && /^f([1-9]|1[0-2])$/.test(lower)) entry = [key.toUpperCase(), key.toUpperCase(), 111 + Number(lower.slice(1)), ''];
  if (!entry && [...(KEY_ALIASES[lower] ?? key)].length === 1) {
    const char = KEY_ALIASES[lower] ?? key;
    const upper = char.toUpperCase();
    const letter = /^[A-Z]$/.test(upper);
    const digit = /^\d$/.test(char);
    entry = [char, letter ? 'Key' + upper : digit ? 'Digit' + char : '', letter || digit ? upper.charCodeAt(0) : 0, char];
  }
  if (!entry) throw new TypeError('Unknown key ' + key);
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
const typeEvents = text => {
  const commands = [];
  for (const char of String(text)) {
    if (char === '\n' || char === '\r') commands.push(...keyEvents('Return'));
    else if (char === '\t') commands.push(...keyEvents('Tab'));
    else if (/^[\x20-\x7e]$/.test(char)) commands.push(...keyEvents(char === ' ' ? 'space' : char === '+' ? 'plus' : char));
    else commands.push(['Input.insertText', { text: char }]);
  }
  return commands;
};
const PW_MODIFIERS = { Alt: 1, Control: 2, Meta: 4, Shift: 8, ControlOrMeta: MAC ? 4 : 2 };
const modifierMask = list => (list ?? []).reduce((mask, name) => {
  const bit = PW_MODIFIERS[name];
  if (bit === undefined) throw new TypeError('Unknown modifier ' + name + '; use Alt, Control, ControlOrMeta, Meta or Shift');
  return mask | bit;
}, 0);
const from = key => ({ $from: { command: 0, pointer: '/result/value/value/' + key } });
const mouseClick = (button, count, modifiers) => {
  const commands = [];
  for (let click = 1; click <= count; click++) {
    commands.push(['Input.dispatchMouseEvent', { type: 'mousePressed', x: from('x'), y: from('y'), button, clickCount: click, modifiers, buttons: 1 }]);
    commands.push(['Input.dispatchMouseEvent', { type: 'mouseReleased', x: from('x'), y: from('y'), button, clickCount: click, modifiers }]);
  }
  return commands;
};
const quote = value => value instanceof RegExp ? String(value) : "'" + String(value).replace(/\\/g, '\\\\').replace(/'/g, "\\'") + "'";
const serializeText = value => value instanceof RegExp ? { source: value.source, flags: value.flags } : String(value);
const optionText = options => {
  const entries = Object.entries(options ?? {}).filter(([, value]) => value !== undefined && !(value instanceof Locator));
  return entries.length ? ', { ' + entries.map(([key, value]) => key + ': ' + (typeof value === 'string' || value instanceof RegExp ? quote(value) : String(value))).join(', ') + ' }' : '';
};
const splitSelector = (text, separator) => {
  const parts = [];
  let quoteChar = null;
  let depth = 0;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (quoteChar) { if (char === '\\') i++; else if (char === quoteChar) quoteChar = null; continue; }
    if (char === '"' || char === "'") quoteChar = char;
    else if (char === '(' || char === '[') depth++;
    else if (char === ')' || char === ']') depth--;
    else if (depth === 0 && text.startsWith(separator, i)) { parts.push(text.slice(start, i)); start = i + separator.length; i += separator.length - 1; }
  }
  parts.push(text.slice(start));
  return parts;
};
const unescapeQuoted = text => text.replace(/\\(.)/g, '$1');
const textArgument = (value, quotedExact) => {
  const trimmed = value.trim();
  const quoted = /^(["'])((?:\\.|(?!\1).)*)\1([is]?)$/.exec(trimmed);
  if (quoted) return { text: unescapeQuoted(quoted[2]), exact: quoted[3] === 's' || (quoted[3] === '' && quotedExact) };
  const regex = /^\/(.*)\/([dgimsuy]*)$/.exec(trimmed);
  if (regex) return { text: { source: regex[1], flags: regex[2] }, exact: false };
  return { text: trimmed, exact: false };
};
const attributeList = text => {
  const out = [];
  const pattern = /\[\s*([\w-]+)\s*(?:=\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|\/(?:[^\/\\]|\\.)*\/[a-z]*|[^\]\s]*)\s*([is])?)?\s*\]/g;
  let match;
  while ((match = pattern.exec(text))) out.push({ name: match[1], raw: match[2], suffix: match[3] ?? '' });
  return out;
};
const parseRole = text => {
  const role = text.split('[')[0].trim();
  const step = { kind: 'role', role };
  for (const attribute of attributeList(text.slice(role.length))) {
    const value = attribute.raw === undefined ? true : attribute.raw;
    if (attribute.name === 'name') {
      const parsed = textArgument(String(value) + attribute.suffix, false);
      step.name = parsed.text;
      step.exact = parsed.exact;
    } else if (attribute.name === 'include-hidden') step.includeHidden = value === true || value === 'true';
    else if (attribute.name === 'level') step.level = Number(value);
    else if (['checked', 'disabled', 'expanded', 'pressed', 'selected'].includes(attribute.name)) step[attribute.name] = value === true || value === 'true' ? true : value === 'mixed' ? 'mixed' : false;
  }
  return step;
};
const parseSelector = selector => {
  const steps = [];
  for (const raw of splitSelector(String(selector), '>>')) {
    const part = raw.trim();
    if (!part) continue;
    let match;
    if ((match = /^nth=(-?\d+)$/.exec(part))) steps.push({ kind: 'nth', n: Number(match[1]) });
    else if ((match = /^visible=(true|false)$/.exec(part))) steps.push({ kind: 'filter', visible: match[1] === 'true' });
    else if (part.startsWith('text=')) steps.push({ kind: 'text', ...textArgument(part.slice(5), true) });
    else if (part.startsWith('internal:text=')) steps.push({ kind: 'text', ...textArgument(part.slice(14), false) });
    else if (part.startsWith('internal:label=')) steps.push({ kind: 'label', ...textArgument(part.slice(15), false) });
    else if (part.startsWith('internal:has-text=')) steps.push({ kind: 'filter', hasText: textArgument(part.slice(18), false).text });
    else if (part.startsWith('internal:has-not-text=')) steps.push({ kind: 'filter', hasNotText: textArgument(part.slice(22), false).text });
    else if (part.startsWith('internal:role=')) steps.push(parseRole(part.slice(14)));
    else if (part.startsWith('role=')) steps.push(parseRole(part.slice(5)));
    else if (part.startsWith('internal:testid=') || part.startsWith('internal:attr=')) {
      const [attribute] = attributeList(part.slice(part.indexOf('=') + 1));
      const parsed = textArgument(String(attribute?.raw ?? '') + (attribute?.suffix ?? ''), false);
      const kind = { 'data-testid': 'testid', placeholder: 'placeholder', alt: 'alt', title: 'title' }[attribute?.name] ?? 'css';
      steps.push(kind === 'css' ? { kind, selector: '[' + attribute?.name + ']' } : { kind, text: parsed.text, exact: parsed.exact });
    } else if (part.startsWith('css=')) steps.push({ kind: 'css', selector: part.slice(4) });
    else if (part.startsWith('xpath=')) steps.push({ kind: 'xpath', selector: part.slice(6) });
    else if (/^\(*\/\//.test(part) || part.startsWith('..')) steps.push({ kind: 'xpath', selector: part });
    else if (part.startsWith('id=')) steps.push({ kind: 'css', selector: '[id=' + JSON.stringify(part.slice(3)) + ']' });
    else if (part.startsWith('data-testid=') || part.startsWith('data-test-id=')) steps.push({ kind: 'testid', text: part.slice(part.indexOf('=') + 1).replace(/^["']|["']$/g, ''), exact: true });
    else if (/^(["']).*\1$/.test(part)) steps.push({ kind: 'text', ...textArgument(part, true) });
    else steps.push({ kind: 'css', selector: part });
  }
  return steps;
};
const locatorChain = value => {
  if (!(value instanceof Locator)) throw new TypeError('Expected a locator created with tab.playwright');
  return value.chain;
};
const filterStep = options => {
  if (!options) return null;
  const step = { kind: 'filter' };
  if (options.hasText !== undefined) step.hasText = serializeText(options.hasText);
  if (options.hasNotText !== undefined) step.hasNotText = serializeText(options.hasNotText);
  if (options.has !== undefined) step.has = locatorChain(options.has);
  if (options.hasNot !== undefined) step.hasNot = locatorChain(options.hasNot);
  if (options.visible !== undefined) step.visible = !!options.visible;
  return Object.keys(step).length > 1 ? step : null;
};
const filterText = options => {
  const parts = Object.entries(options ?? {}).filter(([, value]) => value !== undefined).map(([key, value]) => key + ': ' + (value instanceof Locator ? value.description : typeof value === 'string' || value instanceof RegExp ? quote(value) : String(value)));
  return '{ ' + parts.join(', ') + ' }';
};
const roleStep = (role, options = {}) => {
  const step = { kind: 'role', role: String(role) };
  if (options.name !== undefined) step.name = serializeText(options.name);
  for (const key of ['exact', 'checked', 'disabled', 'expanded', 'includeHidden', 'level', 'pressed', 'selected']) if (options[key] !== undefined) step[key] = options[key];
  return step;
};
const textStep = (kind, text, options) => ({ kind, text: serializeText(text), exact: !!options?.exact });
const timeoutOf = options => {
  const value = options?.timeoutMs ?? options?.timeout ?? ACTION_TIMEOUT_MS;
  if (!Number.isFinite(value) || value < 0) throw new TypeError('timeoutMs must be a non-negative number');
  return value;
};

class Locator {
  constructor(tab, chain, description) {
    this.tab = tab;
    this.chain = chain;
    this.description = description;
  }

  #with(steps, suffix) { return new Locator(this.tab, [...this.chain, ...steps], this.description + suffix); }
  #scoped(step, text) { return new Locator(this.tab, [...this.chain, step], this.description ? this.description + '.' + text : text); }
  toString() { return this.description; }
  [util.inspect.custom]() { return 'Locator@' + this.description; }

  locator(selector, options) {
    const base = selector instanceof Locator
      ? new Locator(this.tab, [...this.chain, ...selector.chain], (this.description ? this.description + '.' : '') + 'locator(' + selector.description + ')')
      : new Locator(this.tab, [...this.chain, ...parseSelector(selector)], (this.description ? this.description + '.' : '') + 'locator(' + quote(selector) + ')');
    return options ? base.filter(options) : base;
  }
  getByRole(role, options) { return this.#scoped(roleStep(role, options), 'getByRole(' + quote(role) + optionText(options) + ')'); }
  getByText(text, options) { return this.#scoped(textStep('text', text, options), 'getByText(' + quote(text) + optionText(options) + ')'); }
  getByLabel(text, options) { return this.#scoped(textStep('label', text, options), 'getByLabel(' + quote(text) + optionText(options) + ')'); }
  getByPlaceholder(text, options) { return this.#scoped(textStep('placeholder', text, options), 'getByPlaceholder(' + quote(text) + optionText(options) + ')'); }
  getByAltText(text, options) { return this.#scoped(textStep('alt', text, options), 'getByAltText(' + quote(text) + optionText(options) + ')'); }
  getByTitle(text, options) { return this.#scoped(textStep('title', text, options), 'getByTitle(' + quote(text) + optionText(options) + ')'); }
  getByTestId(testId) { return this.#scoped({ kind: 'testid', text: serializeText(testId), exact: true }, 'getByTestId(' + quote(testId) + ')'); }
  frameLocator(selector) {
    const text = 'frameLocator(' + quote(selector) + ')';
    const steps = parseSelector(selector);
    return new FrameLocator(this.tab, [...this.chain, ...steps.slice(0, -1), { kind: 'frame', selector: steps[steps.length - 1] ?? { kind: 'css', selector }, description: text }], (this.description ? this.description + '.' : '') + text);
  }
  contentFrame() { return new FrameLocator(this.tab, [...this.chain.slice(0, -1), { kind: 'frame', selector: this.chain[this.chain.length - 1], description: this.description }], this.description + '.contentFrame()'); }
  first() { return this.#with([{ kind: 'nth', n: 0 }], '.first()'); }
  last() { return this.#with([{ kind: 'nth', n: -1 }], '.last()'); }
  nth(position) {
    if (!Number.isInteger(position)) throw new TypeError('nth expects an integer index');
    return this.#with([{ kind: 'nth', n: position }], '.nth(' + position + ')');
  }
  filter(options) { const step = filterStep(options); return step ? this.#with([step], '.filter(' + filterText(options) + ')') : this; }
  and(other) { return this.#with([{ kind: 'and', chain: locatorChain(other) }], '.and(' + other.description + ')'); }
  or(other) { return this.#with([{ kind: 'or', chain: locatorChain(other) }], '.or(' + other.description + ')'); }

  #req(op, extra) { return { op, chain: this.chain, description: this.description, ...extra }; }
  async count() { return (await this.tab.engine(this.#req('count'))).value; }
  async all() {
    const count = await this.count();
    return Array.from({ length: count }, (_, i) => this.nth(i));
  }
  async allTextContents() { return (await this.tab.engine(this.#req('texts'))).value; }
  async allInnerTexts() { return (await this.tab.engine(this.#req('texts', { inner: true }))).value; }
  async #read(what, options, extra) { return (await this.tab.engine(this.#req('read', { what, timeoutMs: timeoutOf(options), ...extra }))).value; }
  textContent(options) { return this.#read('textContent', options); }
  innerText(options) { return this.#read('innerText', options); }
  innerHTML(options) { return this.#read('innerHTML', options); }
  inputValue(options) { return this.#read('inputValue', options); }
  getAttribute(name, options) { return this.#read('attribute', options, { name: String(name) }); }
  boundingBox(options) { return this.#read('box', options); }
  ariaSnapshot(options) { return this.#read('aria', options); }
  async #state(what) { return (await this.tab.engine(this.#req('state', { what }))).value; }
  isVisible() { return this.#state('visible'); }
  isHidden() { return this.#state('hidden'); }
  isEnabled() { return this.#state('enabled'); }
  isDisabled() { return this.#state('disabled'); }
  isEditable() { return this.#state('editable'); }
  isChecked() { return this.#state('checked'); }
  async waitFor(options) {
    const state = options?.state ?? 'visible';
    if (!['attached', 'detached', 'visible', 'hidden'].includes(state)) throw new TypeError('waitFor state must be attached, detached, visible or hidden');
    await this.tab.engine(this.#req('wait', { state, timeoutMs: timeoutOf(options) }));
  }
  async #pointer(options, count, extra = {}) {
    const which = BUTTONS[String(options?.button ?? 'left').toLowerCase()];
    if (!which) throw new TypeError('Unknown mouse button ' + options?.button);
    const modifiers = modifierMask(options?.modifiers);
    const req = this.#req('act', { timeoutMs: timeoutOf(options), force: !!options?.force, position: options?.position, enabled: true, hit: true, waiting: 'visible, enabled and receiving pointer events', ...extra });
    const commands = count ? mouseClick(which, count, modifiers) : [['Input.dispatchMouseEvent', { type: 'mouseMoved', x: from('x'), y: from('y'), modifiers }]];
    return (await this.tab.engine(req, commands, { after: ACTION_SETTLE })).value;
  }
  async click(options) { await this.#pointer(options, options?.clickCount ?? 1); }
  async dblclick(options) { await this.#pointer(options, 2); }
  async hover(options) { await this.#pointer(options, 0); }
  async tap(options) { await this.#pointer(options, 1); }
  async setChecked(checked, options) {
    const wanted = !!checked;
    const outcome = await this.#pointer(options, 1, { precheck: wanted });
    if (outcome?.skip) return;
    const state = (await this.tab.engine(this.#req('setChecked'))).value;
    if (state !== wanted) throw new Error('Clicking ' + this.description + ' did not ' + (wanted ? 'check' : 'uncheck') + ' it');
  }
  check(options) { return this.setChecked(true, options); }
  uncheck(options) { return this.setChecked(false, options); }
  async focus(options) { await this.tab.engine(this.#req('act', { timeoutMs: timeoutOf(options), force: true, focus: true, scroll: false, waiting: 'attached' })); }
  async blur() { await this.evaluate(element => element.blur()); }
  async scrollIntoViewIfNeeded(options) { await this.tab.engine(this.#req('act', { timeoutMs: timeoutOf(options), waiting: 'visible' })); }
  async fill(value, options) {
    const text = String(value);
    const req = this.#req('act', { timeoutMs: timeoutOf(options), force: !!options?.force, editable: true, enabled: true, fill: text, waiting: 'visible, enabled and editable' });
    const outcome = await this.tab.engine(req, text ? [['Input.insertText', { text: from('text') }]] : [], text ? { after: ACTION_SETTLE } : {});
    if (outcome.value?.clear) await this.tab.cdp(keyEvents('BackSpace'), { after: ACTION_SETTLE });
  }
  async clear(options) { await this.fill('', options); }
  async #keys(commands, options) {
    const req = this.#req('act', { timeoutMs: timeoutOf(options), force: true, focus: true, scroll: false, waiting: 'attached' });
    await this.tab.engine(req, commands, { after: ACTION_SETTLE });
  }
  async press(key, options) { await this.#keys(keyEvents(key), options); }
  async pressSequentially(text, options) { await this.#keys(typeEvents(text), options); }
  async type(text, options) { await this.#keys(typeEvents(text), options); }
  async selectOption(values, options) {
    const list = (Array.isArray(values) ? values : [values]).map(value => {
      if (value instanceof Locator) throw new TypeError('selectOption takes option values, labels or { value, label, index }');
      return typeof value === 'object' && value !== null ? value : String(value);
    });
    return (await this.tab.engine(this.#req('select', { values: list, timeoutMs: timeoutOf(options) }), [], { after: ACTION_SETTLE })).value;
  }
  async evaluate(pageFunction, arg, options) {
    const body = 'const el=await e.resolveOne(' + JSON.stringify(this.chain) + ',' + JSON.stringify(this.description) + ',' + timeoutOf(options) + ');return {value:await (' + callable(pageFunction) + ')(el,' + argument(arg) + ')}';
    return this.tab.evaluateWithEngine(body);
  }
  async evaluateAll(pageFunction, arg) {
    const body = 'const els=e.resolveAll(' + JSON.stringify(this.chain) + ');return {value:await (' + callable(pageFunction) + ')(els,' + argument(arg) + ')}';
    return this.tab.evaluateWithEngine(body);
  }
  async setInputFiles(files, options) {
    const list = (Array.isArray(files) ? files : [files]).map(file => path.resolve(String(file)));
    for (const file of list) if (!fs.existsSync(file)) throw new Error('No such file: ' + file);
    await this.tab.setFiles(this.chain, this.description, list, timeoutOf(options));
  }
  async screenshot(options) {
    const box = await this.boundingBox(options);
    if (!box) throw new Error(this.description + ' has no visible box to capture');
    return this.tab.screenshot({ clip: box });
  }
  async downloadMedia(options) {
    const url = await this.#read('media', options);
    const result = await this.tab.step({ action: 'download', url, timeoutMs: options?.timeoutMs ?? 120000 });
    return result.path;
  }
  async highlight() {}
}

class FrameLocator {
  constructor(tab, chain, description) {
    this.tab = tab;
    this.chain = chain;
    this.description = description;
  }
  #root() { return new Locator(this.tab, this.chain, this.description); }
  locator(selector, options) { return this.#root().locator(selector, options); }
  getByRole(role, options) { return this.#root().getByRole(role, options); }
  getByText(text, options) { return this.#root().getByText(text, options); }
  getByLabel(text, options) { return this.#root().getByLabel(text, options); }
  getByPlaceholder(text, options) { return this.#root().getByPlaceholder(text, options); }
  getByAltText(text, options) { return this.#root().getByAltText(text, options); }
  getByTitle(text, options) { return this.#root().getByTitle(text, options); }
  getByTestId(testId) { return this.#root().getByTestId(testId); }
  frameLocator(selector) { return this.#root().frameLocator(selector); }
  owner() { const frame = this.chain[this.chain.length - 1]; return new Locator(this.tab, [...this.chain.slice(0, -1), frame.selector], this.description + '.owner()'); }
  first() { return this; }
  last() { return this; }
  nth() { return this; }
  [util.inspect.custom]() { return 'FrameLocator@' + this.description; }
}

const callable = pageFunction => {
  if (typeof pageFunction === 'function') {
    const source = pageFunction.toString();
    if (/^(async\s+)?(function\b|\(|[\w$]+\s*=>)/.test(source)) return source;
    return source.startsWith('async') ? 'async function ' + source.slice(5).trim() : 'function ' + source;
  }
  if (typeof pageFunction === 'string') return '(function(){const f=(' + pageFunction + ');return typeof f==="function"?f.apply(this,arguments):f;})';
  throw new TypeError('evaluate expects a function or a string expression');
};
const argument = value => {
  if (value === undefined) return 'undefined';
  if (value instanceof Locator) throw new TypeError('Locators cannot be passed to evaluate; use locator.evaluate()');
  return JSON.stringify(value);
};
const globToRegex = glob => {
  let out = '';
  for (let i = 0; i < glob.length; i++) {
    const char = glob[i];
    if (char === '*') { if (glob[i + 1] === '*') { out += '.*'; i++; } else out += '[^/]*'; }
    else if (char === '?') out += '.';
    else out += /[.+^$(){}|[\]\\]/.test(char) ? '\\' + char : char;
  }
  return new RegExp('^' + out + '$');
};
const urlMatcher = url => {
  if (url === undefined || url === null) return () => true;
  if (url instanceof RegExp) return value => url.test(value);
  if (typeof url === 'function') return value => { try { return !!url(new URL(value)); } catch { return false; } };
  const text = String(url);
  if (/[*?]/.test(text)) { const re = globToRegex(text); return value => re.test(value); }
  return value => value === text || value.replace(/\/$/, '') === text.replace(/\/$/, '');
};
const LOG_LEVELS = { warning: 'warn', warn: 'warn', error: 'error', info: 'info', log: 'log', debug: 'debug' };

function selectTextInPage(text, prefix, suffix, mode) {
  const wanted = (prefix || '') + text + (suffix || '');
  const offsetIn = value => { const at = value.indexOf(wanted); return at < 0 ? -1 : at + (prefix || '').length; };
  const pick = start => mode === 'cursor_before' ? [start, start] : mode === 'cursor_after' ? [start + text.length, start + text.length] : [start, start + text.length];
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
}

function setValueInPage(value) {
  if (this instanceof HTMLOptionElement && this.parentElement) {
    const select = this.closest('select');
    if (!select) throw new Error('option is not inside a select');
    select.value = this.value;
    select.dispatchEvent(new Event('input', { bubbles: true }));
    select.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
  }
  if (this instanceof HTMLSelectElement) {
    const option = [...this.options].find(candidate => candidate.value === value || candidate.label === value || candidate.text === value);
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
}

class BrowserTab {
  #connection;
  #tab;
  #refs = new Map();
  #stable = new Map();
  #nextIndex = 1;
  #lines = null;
  #turn = null;
  #session = null;
  #keep = false;
  #claimed = false;
  #url;
  #title;
  #navigations = -1;
  #engineAt = -2;
  #playwright = null;
  used = 0;

  constructor(connection, tabId, info, claimed = false) {
    this.#connection = connection;
    this.#tab = tabId;
    this.#claimed = claimed;
    Object.defineProperty(this, 'id', { value: String(tabId), enumerable: true });
    Object.defineProperty(this, 'providerTabId', { value: String(tabId), enumerable: true });
    Object.defineProperty(this, 'browserId', { value: connection.id, enumerable: true });
    this.#url = info?.url;
    this.#title = info?.title;
    this.#turn = callIdentity().turn;
    this.#session = callIdentity().session;
    this.used = Date.now();
  }

  get turn() { return this.#turn; }
  get session() { return this.#session; }
  get kept() { return this.#keep; }
  get claimed() { return this.#claimed; }
  get connection() { return this.#connection; }
  [util.inspect.custom]() { return 'Tab ' + this.id + ' ' + JSON.stringify(this.#title ?? '') + ' ' + (this.#url ?? ''); }
  info() { return { id: this.id, providerTabId: this.providerTabId, browserId: this.#connection.id, ...(this.#title ? { title: this.#title } : {}), ...(this.#url ? { url: this.#url } : {}) }; }

  async step(input) {
    this.#turn = callIdentity().turn;
    this.used = Date.now();
    const result = await relay.step(this.#connection, { tab: this.#tab, ...input });
    if (typeof result?.url === 'string') this.#url = result.url;
    return result;
  }

  async cdp(commands, options = {}) {
    const input = { action: 'cdp', commands: commands.map(([method, params, session]) => ({ method, params: params ?? {}, ...(session ? { session } : {}) })) };
    if (options.before) input.before = options.before;
    if (options.after) input.after = options.after;
    const result = await this.step(input);
    if (typeof result.navigations === 'number') this.#navigations = result.navigations;
    if (options.raw) return result;
    return result.results.map((entry, position) => {
      if (entry.error !== undefined) {
        const error = new Error(commands[position][0] + ': ' + entry.error);
        error.cdp = true;
        throw error;
      }
      return entry.result;
    });
  }

  async #ensureEngine() {
    if (this.#engineAt === this.#navigations && this.#navigations >= 0) return;
    const result = await this.cdp([['Runtime.evaluate', { expression: ENGINE + ',1', returnByValue: true }]], { raw: true });
    this.#engineAt = result.navigations;
  }

  async engine(req, extra = [], options = {}) {
    const commands = [['Runtime.evaluate', { expression: ENGINE_RUN(req), returnByValue: true, awaitPromise: true }], ...extra];
    const deadline = Date.now() + (req.timeoutMs ?? 0) + 10000;
    for (let attempt = 0; ; attempt++) {
      await this.#ensureEngine();
      const result = await this.cdp(commands, { ...options, raw: true });
      const first = result.results[0];
      let failure = first.error;
      const evaluated = first.result;
      if (failure === undefined && evaluated?.exceptionDetails) failure = exceptionText(evaluated.exceptionDetails);
      if (failure !== undefined) {
        if (!first.dialog && TRANSIENT.test(failure) && Date.now() < deadline) { await sleep(60); continue; }
        throw new Error(failure);
      }
      const value = evaluated.result?.value;
      if (value?.missing) {
        this.#engineAt = -2;
        if (attempt > 3) throw new Error('The page refused the locator engine');
        continue;
      }
      if (value?.error) throw new Error(value.error);
      for (let i = 1; i < result.results.length; i++) {
        const entry = result.results[i];
        if (entry.error !== undefined && !/is not in an earlier result/.test(entry.error)) throw new Error(commands[i][0] + ': ' + entry.error);
      }
      return { value: value?.value, result };
    }
  }

  async evaluateWithEngine(body) {
    const expression = "(async()=>{const e=globalThis[Symbol.for('nanocode.engine')];if(!e)return{missing:true};" + body + '})()';
    for (let attempt = 0; ; attempt++) {
      await this.#ensureEngine();
      const [result] = await this.cdp([['Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }]]);
      if (result.exceptionDetails) {
        const text = exceptionText(result.exceptionDetails);
        if (TRANSIENT.test(text) && attempt < 20) { await sleep(60); continue; }
        throw new Error(text);
      }
      const value = result.result?.value;
      if (value?.missing && attempt < 3) { this.#engineAt = -2; continue; }
      return value?.value;
    }
  }

  async evaluate(pageFunction, arg) {
    const expression = '(async()=>({value:await (' + callable(pageFunction) + ')(' + argument(arg) + ')}))()';
    for (let attempt = 0; ; attempt++) {
      const [result] = await this.cdp([['Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, userGesture: true }]]);
      if (result.exceptionDetails) {
        const text = exceptionText(result.exceptionDetails);
        if (TRANSIENT.test(text) && attempt < 20) { await sleep(60); continue; }
        throw new Error(text);
      }
      return result.result?.value?.value;
    }
  }

  async setFiles(chain, description, files, timeoutMs) {
    const expression = "(async()=>{const e=globalThis[Symbol.for('nanocode.engine')];return e.resolveOne(" + JSON.stringify(chain) + ',' + JSON.stringify(description) + ',' + timeoutMs + ')})()';
    await this.#ensureEngine();
    const [found] = await this.cdp([['Runtime.evaluate', { expression, awaitPromise: true }]]);
    if (found.exceptionDetails) throw new Error(exceptionText(found.exceptionDetails));
    await this.cdp([['DOM.setFileInputFiles', { files, objectId: found.result.objectId }]], { after: ACTION_SETTLE });
  }

  async #history(step) {
    const [history] = await this.cdp([['Page.getNavigationHistory', {}]]);
    const entry = history.entries[history.currentIndex + step];
    if (!entry) throw new Error('There is no page to go ' + (step < 0 ? 'back' : 'forward') + ' to');
    await this.cdp([['Page.navigateToHistoryEntry', { entryId: entry.id }]], { after: NAV_SETTLE });
  }

  #node(value) {
    const node = this.#refs.get(index(value));
    if (node === undefined) throw new Error('Element ' + value + " is not in this tab's latest state; call getAXState() and use its indexes");
    return node;
  }

  async #center(value) {
    const backendNodeId = this.#node(value);
    const [, quads] = await this.cdp([['DOM.scrollIntoViewIfNeeded', { backendNodeId }], ['DOM.getContentQuads', { backendNodeId }]]).catch(async error => {
      if (!error.cdp) throw error;
      return [null, (await this.cdp([['DOM.getContentQuads', { backendNodeId }]]))[0]];
    });
    const quad = quads?.quads?.[0];
    if (!quad || quad.length !== 8) throw new Error('Element ' + value + ' is not visible; scroll it into view or use coordinates');
    return [(quad[0] + quad[2] + quad[4] + quad[6]) / 4, (quad[1] + quad[3] + quad[5] + quad[7]) / 4];
  }

  async #call(value, functionDeclaration, args = []) {
    const [, result] = await this.cdp([
      ['DOM.resolveNode', { backendNodeId: this.#node(value) }],
      ['Runtime.callFunctionOn', { objectId: { $from: { command: 0, pointer: '/object/objectId' } }, functionDeclaration, arguments: args.map(arg => ({ value: arg })), returnByValue: true, awaitPromise: true }],
    ]).catch(error => {
      if (error.cdp) throw new Error('Element ' + value + ' is gone; call getAXState() (' + error.message + ')');
      throw error;
    });
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result?.value;
  }

  async #snapshot() {
    const [location, tree] = await this.cdp([
      ['Runtime.evaluate', { expression: 'JSON.stringify([location.href, document.title])', returnByValue: true }],
      ['Accessibility.getFullAXTree', {}],
    ], { before: READ_SETTLE });
    try { [this.#url, this.#title] = JSON.parse(location.result.value); } catch {}
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
        if (role === 'StaticText' || role === 'text') line = name ? indent + '- text: ' + JSON.stringify(clip(name, 200)) : '';
        else {
          const properties = node.properties ?? [];
          const property = key => properties.find(entry => entry.name === key)?.value?.value;
          const focusable = property('focusable') === true;
          let prefix = '';
          if ((ACTIONABLE.has(role) || focusable) && Number.isInteger(node.backendDOMNodeId)) {
            let stable = this.#stable.get(node.backendDOMNodeId);
            if (stable === undefined) { stable = this.#nextIndex++; this.#stable.set(node.backendDOMNodeId, stable); }
            refs.set(stable, node.backendDOMNodeId);
            prefix = '[' + stable + '] ';
          }
          line = indent + '- ' + prefix + role + (name ? ' ' + JSON.stringify(clip(name, 200)) : '');
          const level = property('level');
          if (typeof level === 'number') line += ' [level=' + level + ']';
          for (const flag of FLAGS) {
            const value = property(flag);
            if (value === undefined || value === null || value === false || value === 'false') continue;
            line += value === true || value === 'true' ? ' [' + flag + ']' : ' [' + flag + '=' + value + ']';
          }
          const url = axText(properties.find(entry => entry.name === 'url')?.value);
          if (url) line += ' [url=' + clip(url, 500) + ']';
          const value = axText(node.value);
          if (value && value !== name && value !== url) line += ': ' + JSON.stringify(clip(value, 200));
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

  #header() { return 'Browser tab: ' + this.id + ', Title: ' + JSON.stringify(this.#title ?? '') + ', URL: ' + JSON.stringify(this.#url ?? '') + '.'; }

  async #state(diff) {
    const lines = await this.#snapshot();
    const header = this.#header();
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
    if (!added.length && !removed.length) return header + '\nNo accessibility-tree change since the previous state.';
    if (added.length + removed.length > lines.length) return [header, ...lines].join('\n');
    const row = line => line.trim().replace(/^- /, '');
    return [header, 'Since the previous state: ' + added.length + ' added or changed, ' + removed.length + ' removed; other elements keep their index.', ...added.map(line => '+ ' + row(line)), ...removed.map(line => '- ' + row(line))].join('\n');
  }

  async screenshot(options = {}) {
    const viewport = await this.#viewport({ before: READ_SETTLE });
    let area;
    if (options.clip) area = { x: Number(options.clip.x) + viewport.pageX, y: Number(options.clip.y) + viewport.pageY, width: Number(options.clip.width), height: Number(options.clip.height), scale: 1 };
    else if (options.fullPage) area = { x: 0, y: 0, width: Math.ceil(viewport.contentWidth), height: Math.ceil(viewport.contentHeight), scale: 1 };
    else area = { x: viewport.pageX, y: viewport.pageY, width: viewport.clientWidth, height: viewport.clientHeight, scale: 1 };
    const type = options.type === 'jpeg' ? 'jpeg' : 'png';
    const shot = await this.#capture({ format: type, ...(type === 'jpeg' ? { quality: options.quality ?? 80 } : {}), captureBeyondViewport: true, clip: area });
    return Buffer.from(shot.data, 'base64');
  }

  async #capture(params) {
    const attempts = [];
    const launch = () => attempts.push(this.cdp([['Page.captureScreenshot', params]]).then(([shot]) => ({ shot }), error => ({ error })));
    launch();
    for (const delay of SCREENSHOT_RETRY_MS) {
      const winner = await Promise.race([...attempts, sleep(delay).then(() => null)]);
      if (winner?.shot) return winner.shot;
      if (winner?.error) throw winner.error;
      launch();
    }
    const winner = await Promise.race([...attempts, sleep(SCREENSHOT_TIMEOUT_MS).then(() => null)]);
    if (winner?.shot) return winner.shot;
    throw winner?.error ?? new Error('The browser did not render tab ' + this.id + ' for a screenshot within ' + SCREENSHOT_TIMEOUT_MS + ' ms');
  }

  async #viewport(options) {
    const [result] = await this.cdp([['Runtime.evaluate', { expression: VIEWPORT, returnByValue: true }]], options);
    const [pageX, pageY, clientWidth, clientHeight, contentWidth, contentHeight] = result.result?.value ?? [0, 0, 800, 600, 800, 600];
    return { pageX, pageY, clientWidth, clientHeight, contentWidth, contentHeight };
  }

  async getAXState(options) {
    const text = await this.#state(options?.disableDiffing !== true);
    emitState(text, options);
    return text;
  }

  async getScreenshot(options) {
    const screenshot = await this.screenshot();
    await emitScreenshot(screenshot, options);
    return screenshot;
  }

  async getAXStateAndScreenshot(options) {
    const state = await this.#state(options?.disableDiffing !== true);
    const screenshot = await this.screenshot();
    emitState(state, options);
    await emitScreenshot(screenshot, options);
    return { state, screenshot };
  }

  async click(target, options) {
    const which = button(options?.mouseButton);
    const count = options?.clickCount ?? 1;
    const [x, y] = isPoint(target) ? target : await this.#center(target);
    const modifiers = options?.key ? parseChord(options.key + '+a').modifiers.reduce((sum, modifier) => sum | CDP_MODIFIERS[modifier], 0) : 0;
    const commands = [];
    for (let click = 1; click <= count; click++) {
      commands.push(['Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: which, clickCount: click, modifiers, buttons: 1 }]);
      commands.push(['Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: which, clickCount: click, modifiers }]);
    }
    if (options?.durationMs) {
      for (const command of commands) {
        await this.cdp([command]);
        if (command[1].type === 'mousePressed') await sleep(options.durationMs);
      }
      await this.cdp([], { after: ACTION_SETTLE });
      return;
    }
    await this.cdp(commands, { after: ACTION_SETTLE });
  }

  async drag(start, end) {
    if (!isPoint(start) || !isPoint(end)) throw new TypeError('drag expects two [x, y] points');
    const moves = Array.from({ length: DRAG_STEPS }, (_, step) => {
      const t = (step + 1) / DRAG_STEPS;
      return ['Input.dispatchMouseEvent', { type: 'mouseMoved', x: start[0] + (end[0] - start[0]) * t, y: start[1] + (end[1] - start[1]) * t, button: 'left', buttons: 1 }];
    });
    await this.cdp([
      ['Input.dispatchMouseEvent', { type: 'mouseMoved', x: start[0], y: start[1] }],
      ['Input.dispatchMouseEvent', { type: 'mousePressed', x: start[0], y: start[1], button: 'left', clickCount: 1, buttons: 1 }],
      ...moves,
      ['Input.dispatchMouseEvent', { type: 'mouseReleased', x: end[0], y: end[1], button: 'left', clickCount: 1 }],
    ], { after: ACTION_SETTLE });
  }

  #focusFirst(elementIndex) { return elementIndex === null || elementIndex === undefined ? [] : [['DOM.focus', { backendNodeId: this.#node(elementIndex) }]]; }

  async pressKey(...args) {
    const indexed = args.length >= 2 && (args[0] === null || typeof args[0] === 'number') && typeof args[1] === 'string';
    const [elementIndex, key, options] = indexed ? args : [null, args[0], args[1]];
    const events = keyEvents(key);
    if (options?.durationMs) {
      await this.cdp([...this.#focusFirst(elementIndex), events[0]]);
      await sleep(options.durationMs);
      await this.cdp([events[1]], { after: ACTION_SETTLE });
      return;
    }
    await this.cdp([...this.#focusFirst(elementIndex), ...events], { after: ACTION_SETTLE });
  }

  async scroll(target, dir, pages = 1) {
    const resolved = direction(dir);
    const viewport = await this.#viewport();
    const [x, y] = isPoint(target) ? target : target === undefined || target === null ? [viewport.clientWidth / 2, viewport.clientHeight / 2] : await this.#center(target);
    const vertical = resolved === 'up' || resolved === 'down';
    const sign = resolved === 'up' || resolved === 'left' ? -1 : 1;
    const amount = pages && typeof pages === 'object' && Number.isFinite(pages.pixels) ? pages.pixels : pages * CHROME_PAGE_STEP_FRACTION * (vertical ? viewport.clientHeight : viewport.clientWidth);
    const distance = sign * amount;
    await this.cdp([['Input.dispatchMouseEvent', { type: 'mouseWheel', x, y, deltaX: vertical ? 0 : distance, deltaY: vertical ? distance : 0 }]], { after: ACTION_SETTLE });
  }

  async selectText(elementIndex, text, options) {
    const found = await this.#call(elementIndex, selectTextInPage.toString(), [String(text), options?.prefix ?? '', options?.suffix ?? '', options?.selectionType ?? 'text']);
    if (!found) throw new Error('"' + text + '" was not found in element ' + elementIndex);
  }

  async setValue(elementIndex, value) {
    const handled = await this.#call(elementIndex, setValueInPage.toString(), [String(value)]);
    if (!handled) {
      if (String(value)) await this.cdp([['Input.insertText', { text: String(value) }]], { after: ACTION_SETTLE });
      else await this.cdp(keyEvents('BackSpace'), { after: ACTION_SETTLE });
    } else await this.cdp([], { after: ACTION_SETTLE });
  }

  async typeText(...args) {
    const indexed = args.length >= 2 && (args[0] === null || typeof args[0] === 'number');
    const [elementIndex, text] = indexed ? args : [null, args[0]];
    const parts = String(text).split('\n');
    const commands = [...this.#focusFirst(elementIndex)];
    parts.forEach((part, position) => {
      if (part) commands.push(['Input.insertText', { text: part }]);
      if (position < parts.length - 1) commands.push(...keyEvents('Return'));
    });
    await this.cdp(commands, { after: ACTION_SETTLE });
  }

  async paste(...args) {
    const indexed = args.length >= 2 && (args[0] === null || typeof args[0] === 'number') && typeof args[1] === 'string';
    const [elementIndex, text, options] = indexed ? args : [null, args[0], args[1]];
    const formatName = options?.format ?? 'text';
    if (!['text', 'md', 'html'].includes(formatName)) throw new TypeError('Unknown paste format ' + formatName);
    const focus = this.#focusFirst(elementIndex);
    if (formatName === 'html') await this.cdp([...focus, ['Runtime.evaluate', { expression: 'document.execCommand("insertHTML", false, ' + JSON.stringify(String(text)) + ')' }]], { after: ACTION_SETTLE });
    else await this.cdp([...focus, ['Input.insertText', { text: String(text) }]], { after: ACTION_SETTLE });
  }

  async performSecondaryAction(elementIndex, action) {
    const name = String(action).toLowerCase().replace(/[^a-z]/g, '');
    if (name === 'axshowmenu' || name === 'showmenu' || name === 'contextmenu') return this.click(elementIndex, { mouseButton: 'right' });
    if (name === 'axpress' || name === 'press' || name === 'click') return this.click(elementIndex);
    if (name === 'focus' || name === 'axfocus') { await this.cdp([['DOM.focus', { backendNodeId: this.#node(elementIndex) }]]); return; }
    if (name === 'scrollintoview' || name === 'axscrolltovisible') { await this.cdp([['DOM.scrollIntoViewIfNeeded', { backendNodeId: this.#node(elementIndex) }]]); return; }
    throw new Error('Browser elements support the secondary actions AXShowMenu, AXPress, focus and scrollIntoView, not ' + action);
  }

  async goto(url, options) {
    const target = normalizeUrl(url);
    const [navigated] = await this.cdp([['Page.navigate', { url: target }]], { after: { ...NAV_SETTLE, timeoutMs: options?.timeoutMs ?? NAV_SETTLE.timeoutMs } });
    if (navigated?.errorText) throw new Error('Browser Use cannot open ' + target + ' in tab ' + this.id + '. Browser reported: ' + navigated.errorText);
  }

  async back() { await this.#history(-1); }
  async forward() { await this.#history(1); }
  async reload() { await this.cdp([['Page.reload', {}]], { after: NAV_SETTLE }); }

  async title() {
    const state = await this.step({ action: 'state' });
    this.#title = state.title;
    return state.title;
  }

  async url() {
    const state = await this.step({ action: 'state' });
    return state.url;
  }

  async getJsDialog() {
    const state = await this.step({ action: 'state' });
    const dialog = state.dialog;
    if (!dialog) return undefined;
    const handle = (accept, promptText) => this.cdp([['Page.handleJavaScriptDialog', { accept, ...(promptText !== undefined ? { promptText: String(promptText) } : {}) }]], { after: ACTION_SETTLE }).then(() => undefined);
    const result = { type: dialog.type, message: dialog.message, ...(dialog.type === 'prompt' ? { defaultValue: dialog.defaultPrompt } : {}), dismiss: () => handle(false) };
    if (dialog.type === 'confirm' || dialog.type === 'beforeunload') result.accept = () => handle(true);
    if (dialog.type === 'prompt') result.accept = text => handle(true, text ?? dialog.defaultPrompt);
    return result;
  }

  async close() {
    await relay.step(this.#connection, { action: 'close', tab: this.#tab });
    tabs.delete(this.id);
  }

  async markDeliverable() { await this.step({ action: 'keep' }); this.#keep = true; }
  async markHandoff() { await this.step({ action: 'keep' }); this.#keep = true; }

  get playwright() {
    if (!this.#playwright) this.#playwright = playwrightApi(this);
    return this.#playwright;
  }

  get dev() {
    const tab = this;
    return Object.freeze({
      async logs(options = {}) {
        const { logs } = await tab.step({ action: 'logs' });
        const levels = options.levels ? new Set(options.levels.map(level => LOG_LEVELS[String(level).toLowerCase()] ?? String(level).toLowerCase())) : null;
        const filter = options.filter === undefined ? null : String(options.filter).toLowerCase();
        let entries = logs.filter(entry => (!levels || levels.has(entry.level)) && (!filter || entry.message.toLowerCase().includes(filter)));
        if (options.limit !== undefined) entries = entries.slice(-Math.max(0, options.limit));
        return entries;
      },
    });
  }

  get clipboard() { return CLIPBOARD; }

  get content() {
    const tab = this;
    return Object.freeze({
      async exportGsuite(type) {
        const current = new URL(await tab.url());
        const match = /^\/(document|spreadsheets|presentation)\/d\/([^/]+)/.exec(current.pathname);
        if (current.hostname !== 'docs.google.com' || !match) throw new Error('exportGsuite needs a Google Docs, Sheets or Slides tab');
        const allowed = { document: ['pdf', 'md', 'docx', 'txt'], spreadsheets: ['pdf', 'xlsx', 'csv'], presentation: ['pdf', 'pptx'] }[match[1]];
        if (!allowed.includes(type)) throw new Error(match[1] + ' exports as ' + allowed.join(', ') + ', not ' + type);
        const gid = current.hash.match(/gid=(\d+)/)?.[1];
        const result = await tab.step({ action: 'download', url: 'https://docs.google.com/' + match[1] + '/d/' + match[2] + '/export?format=' + type + (gid && match[1] === 'spreadsheets' ? '&gid=' + gid : ''), timeoutMs: 120000 });
        return result.path;
      },
      async exportYouTubeTranscript() { throw new Error('exportYouTubeTranscript is not supported by the nanocode extension; open the transcript panel and read it with domSnapshot()'); },
    });
  }

  get capabilities() {
    const tab = this;
    return Object.freeze({
      async list() { return [{ id: 'pageAssets', description: 'List assets the current page loaded and bundle selected ones into a temporary local directory.' }]; },
      async get(id) {
        if (id !== 'pageAssets') throw new Error('Unknown tab capability ' + id + '; available: pageAssets');
        return pageAssets(tab);
      },
    });
  }
}

const CLIPBOARD = Object.freeze({
  async readText() { return (await driver.call('clipboard_read', { include_text: true })).structuredContent?.text ?? ''; },
  async writeText(text) { await driver.call('clipboard_write', { text: String(text) }); },
  async read() { return [{ entries: [{ mimeType: 'text/plain', text: await CLIPBOARD.readText() }] }]; },
  async write(items) {
    const entries = (items ?? []).flatMap(item => item?.entries ?? []);
    const text = entries.find(entry => typeof entry.text === 'string');
    if (text) { await driver.call('clipboard_write', { text: text.text }); return; }
    const image = entries.find(entry => entry.base64 && /^image\//.test(entry.mimeType ?? ''));
    if (!image) throw new Error('clipboard.write needs a text entry or an image entry with base64 data');
    const file = path.join(os.tmpdir(), 'cua-clipboard-' + process.pid + '.' + (image.mimeType.split('/')[1] || 'png'));
    fs.writeFileSync(file, Buffer.from(image.base64, 'base64'));
    await driver.call('clipboard_write', { image_path: file });
  },
});

const pageAssets = tab => Object.freeze({
  async documentation() { return DOCUMENTATION.pageAssets; },
  async list(options = {}) {
    const assets = await tab.evaluate(() => {
      const out = new Map();
      for (const entry of performance.getEntriesByType('resource')) out.set(entry.name, { url: entry.name, type: entry.initiatorType, bytes: entry.encodedBodySize || entry.transferSize || undefined });
      for (const element of document.querySelectorAll('img, video, audio, source, link[rel~=icon], link[rel=stylesheet], script[src]')) {
        const url = element.currentSrc || element.src || element.href;
        if (url && !out.has(url)) out.set(url, { url, type: element.localName === 'link' ? (element.rel.includes('icon') ? 'icon' : 'css') : element.localName });
      }
      return [...out.values()].filter(asset => /^https?:/.test(asset.url));
    });
    const type = options.type;
    return assets.filter(asset => !type || asset.type === type || (type === 'image' && ['img', 'icon'].includes(asset.type)));
  },
  async bundle(urls) {
    const list = (Array.isArray(urls) ? urls : [urls]).map(entry => typeof entry === 'string' ? entry : entry?.url).filter(Boolean);
    if (!list.length) throw new Error('bundle expects asset URLs from list()');
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'page-assets-'));
    const files = [];
    for (const url of list) {
      const result = await tab.step({ action: 'download', url, timeoutMs: 120000 });
      const target = path.join(dir, path.basename(result.path));
      try { fs.renameSync(result.path, target); } catch { fs.copyFileSync(result.path, target); fs.rmSync(result.path, { force: true }); }
      files.push(target);
    }
    return { path: dir, files };
  },
});
const playwrightApi = tab => {
  const root = new Locator(tab, [], '');
  const api = {
    async domSnapshot() {
      const { value } = await tab.engine({ op: 'snapshot' }, [], { before: READ_SETTLE });
      let text = value.text;
      if (value.frames.length) text = await stitchFrames(tab, text, value.frames, 0);
      return text.length > DOM_SNAPSHOT_LIMIT ? text.slice(0, DOM_SNAPSHOT_LIMIT) + '\n… (snapshot cut at ' + DOM_SNAPSHOT_LIMIT + ' of ' + text.length + ' characters; read parts with locator(...).ariaSnapshot())' : text;
    },
    evaluate: (pageFunction, arg) => tab.evaluate(pageFunction, arg),
    async expectNavigation(action, options = {}) {
      const before = await tab.step({ action: 'state' });
      const result = await action();
      await waitForNavigation(tab, before.navigations, options);
      return result;
    },
    async waitForNavigation(options = {}) {
      const before = await tab.step({ action: 'state' });
      await waitForNavigation(tab, before.navigations, options);
    },
    locator: (selector, options) => root.locator(selector, options),
    getByRole: (role, options) => root.getByRole(role, options),
    getByText: (text, options) => root.getByText(text, options),
    getByLabel: (text, options) => root.getByLabel(text, options),
    getByPlaceholder: (text, options) => root.getByPlaceholder(text, options),
    getByAltText: (text, options) => root.getByAltText(text, options),
    getByTitle: (text, options) => root.getByTitle(text, options),
    getByTestId: testId => root.getByTestId(testId),
    frameLocator: selector => root.frameLocator(selector),
    async waitForEvent(event, options = {}) {
      const timeoutMs = options.timeoutMs ?? options.timeout ?? 30000;
      if (event === 'download') {
        const result = await tab.step({ action: 'waitDownload', timeoutMs });
        return Object.freeze({ path: async () => result.path, url: () => result.url, suggestedFilename: () => path.basename(result.path), saveAs: async target => { fs.copyFileSync(result.path, target); } });
      }
      if (event === 'filechooser') {
        const chooser = await tab.step({ action: 'fileChooser', timeoutMs });
        return Object.freeze({
          isMultiple: () => chooser.mode === 'selectMultiple',
          async setFiles(files) {
            const list = (Array.isArray(files) ? files : [files]).map(file => path.resolve(String(file)));
            await tab.cdp([['DOM.setFileInputFiles', { files: list, backendNodeId: chooser.backendNodeId }, chooser.session]], { after: ACTION_SETTLE });
          },
        });
      }
      if (event === 'dialog') {
        const deadline = Date.now() + timeoutMs;
        while (Date.now() < deadline) {
          const dialog = await tab.getJsDialog();
          if (dialog) return dialog;
          await sleep(100);
        }
        throw new Error('No dialog opened within ' + timeoutMs + ' ms');
      }
      throw new Error('waitForEvent supports "download", "filechooser" and "dialog", not ' + event);
    },
    async waitForLoadState(stateOrOptions, maybeOptions) {
      const options = typeof stateOrOptions === 'string' ? { ...maybeOptions, state: stateOrOptions } : stateOrOptions ?? {};
      const state = options.state ?? 'load';
      const timeoutMs = options.timeoutMs ?? options.timeout ?? NAVIGATION_TIMEOUT_MS;
      await tab.cdp([], { after: { quietMs: state === 'networkidle' ? 500 : 50, load: state !== 'domcontentloaded', networkIdle: state === 'networkidle', timeoutMs } });
    },
    async waitForTimeout(timeoutMs) { await sleep(Math.max(0, Number(timeoutMs) || 0)); },
    async waitForURL(url, options = {}) {
      const matches = urlMatcher(url);
      const timeoutMs = options.timeoutMs ?? options.timeout ?? NAVIGATION_TIMEOUT_MS;
      const deadline = Date.now() + timeoutMs;
      for (;;) {
        const state = await tab.step({ action: 'state' });
        if (matches(state.url)) break;
        if (Date.now() > deadline) throw new Error('Timeout ' + timeoutMs + 'ms exceeded waiting for URL ' + String(url) + '; the tab is at ' + state.url);
        await sleep(50);
      }
      if (options.waitUntil !== 'commit') await api.waitForLoadState({ state: options.waitUntil ?? 'load', timeoutMs: Math.max(1000, deadline - Date.now()) });
    },
    url: () => tab.url(),
    title: () => tab.title(),
    content: () => tab.evaluate(() => document.documentElement.outerHTML),
    goto: url => tab.goto(url),
    reload: () => tab.reload(),
    goBack: () => tab.back(),
    goForward: () => tab.forward(),
    screenshot: options => tab.screenshot(options),
    keyboard: Object.freeze({
      press: async key => { await tab.cdp(keyEvents(key), { after: ACTION_SETTLE }); },
      type: async text => { await tab.cdp(typeEvents(text), { after: ACTION_SETTLE }); },
      insertText: async text => { await tab.cdp([['Input.insertText', { text: String(text) }]], { after: ACTION_SETTLE }); },
    }),
    mouse: Object.freeze({
      click: async (x, y, options) => { await tab.click([x, y], { mouseButton: options?.button, clickCount: options?.clickCount }); },
      dblclick: async (x, y) => { await tab.click([x, y], { clickCount: 2 }); },
      move: async (x, y) => { await tab.cdp([['Input.dispatchMouseEvent', { type: 'mouseMoved', x, y }]]); },
      wheel: async (deltaX, deltaY) => { await tab.cdp([['Input.dispatchMouseEvent', { type: 'mouseWheel', x: 10, y: 10, deltaX, deltaY }]], { after: ACTION_SETTLE }); },
    }),
  };
  return Object.freeze(api);
};

const waitForNavigation = async (tab, before, options) => {
  const matches = urlMatcher(options.url);
  const timeoutMs = options.timeoutMs ?? options.timeout ?? NAVIGATION_TIMEOUT_MS;
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const state = await tab.step({ action: 'state' });
    if (state.navigations > before && matches(state.url)) break;
    if (Date.now() > deadline) throw new Error('Timeout ' + timeoutMs + 'ms exceeded waiting for a navigation' + (options.url ? ' to ' + String(options.url) : '') + '; the tab is at ' + state.url);
    await sleep(50);
  }
  if (options.waitUntil !== 'commit') await tab.cdp([], { after: { quietMs: 100, load: options.waitUntil !== 'domcontentloaded', networkIdle: options.waitUntil === 'networkidle', timeoutMs: Math.max(1000, deadline - Date.now()) } });
};

const stitchFrames = async (tab, text, placeholders, depth) => {
  let frames = [];
  try { frames = (await tab.step({ action: 'frames' })).frames; } catch {}
  const used = new Set();
  const pick = src => {
    const free = frames.filter(frame => !used.has(frame.frameId));
    let found = free.find(frame => frame.url === src);
    if (!found && src) found = free.find(frame => frame.url.startsWith(src) || src.startsWith(frame.url.split('#')[0]));
    if (!found && free.length === 1) found = free[0];
    if (found) used.add(found.frameId);
    return found;
  };
  const bodies = [];
  for (const src of placeholders) {
    const frame = pick(src);
    let body = '- text: (cross-origin frame ' + (src || '') + ' could not be read)';
    if (frame && depth < 3) {
      try {
        const [result] = await tab.cdp([['Runtime.evaluate', { expression: ENGINE + '.snapshot()', returnByValue: true }, frame.session]]);
        const value = result.result?.value;
        if (value) body = value.frames.length ? await stitchFrames(tab, value.text, value.frames, depth + 1) : value.text || '- text: (empty frame)';
      } catch {}
    }
    bodies.push(body);
  }
  return text.split('\n').map(line => {
    const match = /^(\s*)\u0000frame:(\d+)$/.exec(line);
    if (!match) return line;
    return bodies[Number(match[2])].split('\n').map(inner => match[1] + inner).join('\n');
  }).join('\n');
};

const normalizeUrl = value => {
  const text = String(value).trim();
  return /^[a-z][a-z0-9+.-]*:/i.test(text) ? text : 'https://' + text;
};

const browserObjects = new Map();
const appCache = new Map();
const createCua = () => {
  let coreSent = false;
  let browserSent = false;
  const docs = browser => {
    if (!coreSent) { writeText(DOCS.core); coreSent = true; }
    if (browser && !browserSent) { writeText(DOCS.browser); browserSent = true; }
  };
  const register = (connection, opened, claimed) => {
    const existing = tabs.get(String(opened.tab));
    if (existing && existing.connection === connection) return existing;
    const tab = new BrowserTab(connection, opened.tab, opened, claimed);
    tabs.set(tab.id, tab);
    return tab;
  };
  const openTab = async (connection, url, sessionName) => {
    const input = { action: 'open' };
    const title = sessionName ?? sessionNames.get(callIdentity().session);
    if (title !== undefined) input.title = String(title);
    const opened = await relay.step(connection, input);
    const tab = register(connection, opened, false);
    if (url !== undefined && url !== null && url !== '') await tab.goto(url);
    return tab;
  };
  const sessionTabs = connection => [...tabs.values()].filter(tab => !connection || tab.connection === connection);
  const userTabs = async connection => (await relay.step(connection, { action: 'userTabs' })).tabs;
  const claim = async (connection, id) => {
    const existing = tabs.get(String(id));
    if (existing && existing.connection === connection) return existing;
    const claimed = await relay.step(connection, { action: 'claim', tab: Number(id) });
    return register(connection, claimed, claimed.claimed);
  };
  const findTab = async (reference, browser) => {
    if (reference && typeof reference === 'object' && reference.mention !== undefined) throw new Error('Tab mentions are not available in this runtime; pass a tab id or { url }');
    const connection = browser !== undefined ? await resolveBrowser(browser) : null;
    if (reference && typeof reference === 'object' && reference.url !== undefined) {
      const wanted = String(reference.url);
      const same = value => value === wanted || value?.replace(/\/$/, '') === wanted.replace(/\/$/, '');
      const own = sessionTabs(connection).find(tab => same(tab.info().url)) ?? sessionTabs(connection).find(tab => tab.info().url?.startsWith(wanted));
      if (own) return own;
      for (const candidate of connection ? [connection] : await relay.connections()) {
        const list = await userTabs(candidate);
        const match = list.find(tab => same(tab.url)) ?? list.find(tab => tab.url?.startsWith(wanted));
        if (match) return claim(candidate, match.id);
      }
      throw new Error('No open tab has the URL ' + wanted);
    }
    const id = String(reference);
    const own = tabs.get(id);
    if (own && (!connection || own.connection === connection)) return own;
    if (/^\d+$/.test(id)) {
      for (const candidate of connection ? [connection] : await relay.connections()) {
        const list = await userTabs(candidate);
        if (list.some(tab => String(tab.id) === id)) return claim(candidate, id);
      }
    }
    throw new Error('Tab not found: ' + id + "; list this session's tabs with cua.listTabs() or the user's tabs with browser.user.openTabs()");
  };
  const browserObject = connection => {
    const cached = browserObjects.get(connection.id);
    if (cached) return cached;
    const live = () => {
      const current = relay.list().find(entry => entry.id === connection.id);
      if (!current) throw new Error(connection.name + ' is not connected; reopen it with the nanocode extension loaded');
      return current;
    };
    let viewport = null;
    const browser = Object.freeze({
      browserId: connection.id,
      id: connection.id,
      name: connection.name,
      family: 'chromium',
      type: 'extension',
      async documentation() { return DOCS.browser; },
      capabilities: Object.freeze({
        async list() { return [{ id: 'viewport', description: "Override the viewport size of this session's tabs for responsive or device-size testing." }]; },
        async get(id) {
          if (id !== 'viewport') throw new Error('Unknown browser capability ' + id + '; available: viewport');
          return Object.freeze({
            async documentation() { return DOCUMENTATION.viewport; },
            async get() { return viewport; },
            async set(options) {
              const width = Math.round(Number(options?.width));
              const height = Math.round(Number(options?.height));
              if (!(width > 0 && height > 0)) throw new TypeError('viewport.set expects { width, height } in CSS pixels');
              viewport = { width, height, ...(options.deviceScaleFactor ? { deviceScaleFactor: Number(options.deviceScaleFactor) } : {}), ...(options.mobile ? { mobile: true } : {}) };
              await relay.step(live(), { action: 'viewport', viewport });
              return viewport;
            },
            async reset() { viewport = null; await relay.step(live(), { action: 'viewport', viewport: null }); },
          });
        },
      }),
      tabs: Object.freeze({
        async list() { return sessionTabs(live()).map(tab => tab.info()); },
        async get(tabId) { return findTab(tabId, connection.id); },
        async new(url) { return openTab(live(), url); },
        async selected() {
          const list = await userTabs(live());
          const selected = list.find(tab => tab.selected);
          const own = selected && tabs.get(String(selected.id));
          if (own) return own;
          return sessionTabs(live()).sort((a, b) => b.used - a.used)[0];
        },
      }),
      user: Object.freeze({
        async openTabs() {
          return (await userTabs(live())).map(tab => ({ id: String(tab.id), ...(tab.lastOpened ? { lastOpened: tab.lastOpened } : {}), providerTabId: String(tab.id), ...(tab.tabGroup ? { tabGroup: tab.tabGroup } : {}), ...(tab.title ? { title: tab.title } : {}), ...(tab.url ? { url: tab.url } : {}) }));
        },
        async claimTab(tab) {
          const id = typeof tab === 'object' && tab ? tab.id ?? tab.providerTabId : tab;
          if (id === undefined || id === null) throw new TypeError('claimTab expects a tab id or an entry from user.openTabs()');
          return claim(live(), String(id));
        },
      }),
      async history(options = {}) {
        const time = value => value === undefined ? undefined : new Date(value).getTime();
        const { entries } = await relay.step(live(), { action: 'history', queries: options.queries, startTime: time(options.from), endTime: time(options.to), limit: options.limit });
        return entries;
      },
      async nameSession(name) {
        sessionNames.set(callIdentity().session, String(name));
        await relay.step(live(), { action: 'nameSession', title: String(name) });
      },
      [util.inspect.custom]() { return 'Browser ' + connection.id + ' (' + connection.name + ')'; },
    });
    browserObjects.set(connection.id, browser);
    return browser;
  };
  const show = async tab => {
    const state = await tab.getAXState({ disableDiffing: true, emit: false });
    docs(true);
    writeText(state);
    return tab;
  };
  const browserStates = async () => {
    if (!relay.available) return [];
    const list = await relay.connections(Math.min(BROWSER_WAIT_MS, 1500));
    return list.map(connection => ({ ...browserInfo(connection), tabs: sessionTabs(connection).map(tab => tab.info()) }));
  };
  const nativeApp = async target => {
    if (target instanceof NativeApp) return target;
    if (target && typeof target === 'object' && target.windowId !== undefined) {
      const windows = (await driver.call('list_windows', {})).structuredContent?.windows ?? [];
      const window = windows.find(entry => entry.window_id === Number(target.windowId));
      if (!window) throw new Error('No window has id ' + target.windowId + '; list them with cua.listWindows()');
      const apps = await listDriverApps();
      const app = apps.find(entry => entry.pid === window.pid) ?? { name: window.app_name ?? String(window.pid), pid: window.pid, running: true };
      const bound = new NativeApp(app, window);
      appCache.set(String(window.pid) + ':' + window.window_id, bound);
      return bound;
    }
    if (typeof target !== 'string' || !target.trim()) throw new TypeError('Expected an app name, bundle identifier, path or { windowId }');
    const apps = await listDriverApps();
    let app = findApp(apps, target);
    if (!app && !target.includes('/') && !/^[\w-]+(\.[\w-]+)+$/.test(target)) throw new Error('No app matches "' + target + '"; call cua.listApps() and retry with its bundle identifier');
    if (!app?.running) {
      const pid = await launch(app, target);
      app = { ...(app ?? { name: target }), pid, running: true };
    }
    const cached = appCache.get(String(app.pid));
    if (cached) return cached;
    const bound = new NativeApp(app);
    appCache.set(String(app.pid), bound);
    return bound;
  };
  const point = (x, y) => {
    if (!Number.isFinite(x) || !Number.isFinite(y)) throw new TypeError('Pass element_index or both x and y');
    return [x, y];
  };
  const computer = Object.freeze({
    target: MAC ? 'mac' : process.platform === 'win32' ? 'windows' : 'linux',
    async list_apps() { return (await listDriverApps()).filter(listed).map(appRecord); },
    async launch_app({ app } = {}) { await nativeApp(app); },
    async get_app_state({ app, disableDiff } = {}) {
      const bound = await nativeApp(app);
      const { text, screenshot } = await bound.read({ diff: !disableDiff, screenshot: true });
      let url;
      if (screenshot) {
        const file = path.join(os.tmpdir(), 'cua-' + process.pid + '-' + Date.now() + '.png');
        fs.writeFileSync(file, screenshot);
        url = pathToFileURL(file).href;
      }
      return { app: bound.id, text, ...(url ? { screenshot: url } : {}) };
    },
    async click({ app, element_index, x, y, mouse_button, click_count } = {}) {
      const bound = await nativeApp(app);
      await bound.click(element_index !== undefined && element_index !== null ? element_index : point(x, y), { mouseButton: mouse_button, clickCount: click_count });
    },
    async drag({ app, from_x, from_y, to_x, to_y } = {}) { await (await nativeApp(app)).drag(point(from_x, from_y), point(to_x, to_y)); },
    async perform_secondary_action({ app, element_index, action } = {}) { await (await nativeApp(app)).performSecondaryAction(element_index, action); },
    async press_key({ app, key } = {}) { await (await nativeApp(app)).pressKey(key); },
    async scroll({ app, element_index, x, y, direction: dir, pages } = {}) { await (await nativeApp(app)).scroll(element_index ?? point(x, y), dir, pages ?? 1); },
    async select_text({ app, element_index, text, prefix, suffix, selection_type } = {}) { await (await nativeApp(app)).selectText(element_index, text, { prefix, suffix, selectionType: selection_type }); },
    async set_value({ app, element_index, value } = {}) { await (await nativeApp(app)).setValue(element_index, value); },
    async type_text({ app, text } = {}) { await (await nativeApp(app)).typeText(text); },
  });
  const cua = {
    async initialize() { docs(false); },
    computer,
    async getState(options) {
      docs(false);
      const errors = [];
      const [apps, browsers] = await Promise.all([
        listDriverApps().then(list => list.filter(listed).map(appRecord)).catch(error => { errors.push('apps: ' + error.message); return []; }),
        browserStates().catch(error => { errors.push('browsers: ' + error.message); return []; }),
      ]);
      const state = { apps, browsers, ...(errors.length ? { errors } : {}) };
      emitState(JSON.stringify(state), options);
      return state;
    },
    async getApp(target) {
      docs(false);
      const bound = await nativeApp(target);
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
    async listWindows(options) {
      docs(false);
      const windows = ((await driver.call('list_windows', {})).structuredContent?.windows ?? []).map(window => ({ id: window.window_id, app: window.app_name ?? String(window.pid), ...(window.title ? { title: window.title } : {}) }));
      emitState(JSON.stringify(windows), options);
      return windows;
    },
    async getBrowser(options) {
      const connection = await resolveBrowser(options?.id ?? options?.extensionInstanceId);
      const browser = browserObject(connection);
      const first = !browserSent;
      docs(true);
      if (first) writeText('# Selected Browser\n- Name: ' + connection.name + '\n- Type: extension\n- ID: ' + connection.id + '\nReuse this browser binding across later turns. If a tab is stale or missing later, create a fresh tab from this browser with browser.tabs.new() or cua.createBrowserTab(browser.browserId, url).');
      return browser;
    },
    async createBrowserTab(id, url, options) {
      if (typeof id !== 'string' || !id.trim()) throw new Error('createBrowserTab requires a browser ID such as "chrome" or an id from cua.getState()');
      const connection = await resolveBrowser(id);
      if (options?.visible === true) throw new Error('Browser ' + connection.name + ' does not support visible: true; its tabs always open in the background');
      return show(await openTab(connection, url, options?.sessionName));
    },
    async getTab(reference, options) {
      if (reference === undefined || reference === null || reference === '') throw new Error('getTab requires a tab id or { url }');
      return show(await findTab(reference, options?.browser));
    },
    async listBrowsers(options) {
      docs(false);
      const list = (await browserStates()).map(({ tabs: _tabs, ...info }) => info);
      emitState(JSON.stringify(list), options);
      return list;
    },
    async listTabs(options) {
      docs(false);
      const connection = options?.browser !== undefined ? await resolveBrowser(options.browser) : null;
      const list = sessionTabs(connection).map(tab => tab.info());
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
  };
  const agent = Object.freeze({
    browsers: Object.freeze({
      async list() { return (await relay.connections()).map(browserInfo); },
      async get(id) { return browserObject(await resolveBrowser(id)); },
    }),
    documentation: Object.freeze({
      async get(name) {
        const text = DOCUMENTATION[String(name).replace(/\.md$/, '')];
        if (!text) throw new Error('No documentation named ' + name + '; available: ' + Object.keys(DOCUMENTATION).filter(key => !['viewport', 'pageAssets'].includes(key)).join(', '));
        return text;
      },
    }),
  });
  return { cua: Object.freeze(cua), agent };
};

const baseline = new Map(Reflect.ownKeys(globalThis).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
const installGlobals = () => {
  const created = createCua();
  globalThis.cua = created.cua;
  globalThis.agent = created.agent;
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
      relay.broadcast({ type: 'turn_ended', session, turn });
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
