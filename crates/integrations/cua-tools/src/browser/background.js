const HOST = 'xyz.nanocode.browser';
const PROTOCOL = 2;
const COLORS = ['purple', 'blue', 'cyan', 'green', 'orange', 'pink', 'yellow', 'red', 'grey'];
const BINDING = '__nanocodeMutation';
const LOG_LIMIT = 1000;
const POLL_MS = 15;
const SETUP = "(() => { const key = Symbol.for('nanocode.mutations'); const fresh = globalThis." + BINDING + "; let holder = globalThis[key]; if (!holder) { holder = { report: null }; Object.defineProperty(globalThis, key, { value: holder }); new MutationObserver(() => { try { holder.report && holder.report(''); } catch {} }).observe(document, { subtree: true, childList: true, attributes: true, characterData: true }); } if (typeof fresh === 'function') { holder.report = fresh; try { delete globalThis." + BINDING + "; } catch {} } })()";
const DIALOG_SAFE = new Set(['Page.handleJavaScriptDialog', 'Page.getNavigationHistory', 'Page.getFrameTree']);
const LEVELS = { log: 'log', debug: 'debug', info: 'info', error: 'error', warning: 'warn', assert: 'error', trace: 'debug', dir: 'log', dirxml: 'log', table: 'log', clear: 'log', count: 'log', timeEnd: 'log' };
const tabs = new Map();
const pages = new Map();
const groups = new Map();
const names = new Map();
const viewports = new Map();
const attached = new Set();
const attaching = new Map();
let port = null;

const send = message => port?.postMessage(message);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

const brand = () => {
  const brands = (navigator.userAgentData?.brands ?? []).map(entry => entry.brand);
  for (const [needle, name] of [['Brave', 'Brave'], ['Microsoft Edge', 'Edge'], ['Opera', 'Opera'], ['Vivaldi', 'Vivaldi'], ['Google Chrome', 'Chrome'], ['Chromium', 'Chromium']]) {
    if (brands.includes(needle)) return name;
  }
  return 'Chrome';
};

const connect = () => {
  if (port) return;
  let next;
  try {
    next = chrome.runtime.connectNative(HOST);
  } catch {
    return;
  }
  port = next;
  next.onMessage.addListener(message => {
    handle(message).catch(() => {});
  });
  next.onDisconnect.addListener(() => {
    void chrome.runtime.lastError;
    if (port === next) port = null;
    release();
    setTimeout(connect, 2000);
  });
  chrome.action.setBadgeText({ text: '' });
};

const release = () => {
  for (const tabId of attached) chrome.debugger.detach({ tabId }).catch(() => {});
  attached.clear();
  tabs.clear();
  pages.clear();
  groups.clear();
  names.clear();
  viewports.clear();
};

const colorFor = session => {
  let hash = 0;
  for (const char of session) hash = (hash * 31 + char.charCodeAt(0)) | 0;
  return COLORS[Math.abs(hash) % COLORS.length];
};

const page = tabId => {
  let state = pages.get(tabId);
  if (!state) {
    state = { logs: [], dialog: null, mutation: 0, network: 0, inflight: new Map(), loading: false, started: 0, navigating: 0, domAt: 0, loadAt: 0, navigations: 0, url: '', mainFrame: null, chooser: null, waiters: new Set(), frames: new Map() };
    pages.set(tabId, state);
  }
  return state;
};

const wake = state => {
  for (const waiter of state.waiters) waiter();
};

const render = args => (args ?? []).map(arg => {
  if (arg.type === 'string') return arg.value;
  if (arg.value !== undefined) return typeof arg.value === 'object' ? JSON.stringify(arg.value) : String(arg.value);
  if (arg.unserializableValue) return arg.unserializableValue;
  if (arg.preview?.properties) {
    const body = arg.preview.properties.map(property => (arg.preview.subtype === 'array' ? '' : property.name + ': ') + property.value).join(', ');
    return arg.preview.subtype === 'array' ? '[' + body + ']' : (arg.className && arg.className !== 'Object' ? arg.className + ' ' : '') + '{' + body + (arg.preview.overflow ? ', …' : '') + '}';
  }
  return arg.description ?? arg.type;
}).join(' ');

const log = (state, level, message, url, timestamp) => {
  state.logs.push({ level, message: String(message ?? ''), timestamp: new Date(timestamp && timestamp > 1e12 ? timestamp : timestamp ? timestamp * 1000 : Date.now()).toISOString(), ...(url ? { url } : {}) });
  if (state.logs.length > LOG_LIMIT) state.logs.splice(0, state.logs.length - LOG_LIMIT);
};

chrome.debugger.onEvent.addListener((source, method, params) => {
  const state = pages.get(source.tabId);
  if (!state) return;
  const now = Date.now();
  const main = !source.sessionId;
  switch (method) {
    case 'Runtime.bindingCalled':
      if (params.name === BINDING) state.mutation = now;
      break;
    case 'Runtime.consoleAPICalled':
      log(state, LEVELS[params.type] ?? 'log', render(params.args), params.stackTrace?.callFrames?.[0]?.url, params.timestamp);
      break;
    case 'Runtime.exceptionThrown': {
      const details = params.exceptionDetails ?? {};
      log(state, 'error', details.exception?.description ?? details.text, details.url ?? details.stackTrace?.callFrames?.[0]?.url, params.timestamp);
      break;
    }
    case 'Log.entryAdded': {
      const entry = params.entry ?? {};
      log(state, { verbose: 'debug', info: 'info', warning: 'warn', error: 'error' }[entry.level] ?? 'log', entry.text, entry.url, entry.timestamp);
      break;
    }
    case 'Page.javascriptDialogOpening':
      if (main) state.dialog = { type: params.type, message: params.message, defaultPrompt: params.defaultPrompt ?? '', url: params.url };
      break;
    case 'Page.javascriptDialogClosed':
      if (main) state.dialog = null;
      break;
    case 'Page.frameStartedLoading':
      if (main && params.frameId === state.mainFrame) { state.loading = true; state.started = now; state.navigating = 0; }
      break;
    case 'Page.frameStoppedLoading':
      if (main && params.frameId === state.mainFrame) state.loading = false;
      break;
    case 'Page.frameRequestedNavigation':
    case 'Page.frameScheduledNavigation':
      if (main && params.frameId === state.mainFrame) state.navigating = now;
      break;
    case 'Page.frameClearedScheduledNavigation':
      if (main && params.frameId === state.mainFrame) state.navigating = 0;
      break;
    case 'Page.frameNavigated':
      if (main && !params.frame.parentId) { state.mainFrame = params.frame.id; state.navigating = 0; state.url = params.frame.url + (params.frame.urlFragment ?? ''); state.navigations++; }
      break;
    case 'Page.navigatedWithinDocument':
      if (main && params.frameId === state.mainFrame) { state.url = params.url; state.navigations++; }
      break;
    case 'Page.lifecycleEvent':
      if (main && params.frameId === state.mainFrame) {
        if (params.name === 'DOMContentLoaded') state.domAt = now;
        if (params.name === 'load') state.loadAt = now;
      }
      break;
    case 'Page.loadEventFired':
      if (main) state.loadAt = now;
      break;
    case 'Network.requestWillBeSent':
      if (params.type !== 'WebSocket' && params.type !== 'EventSource') { state.inflight.set((source.sessionId ?? '') + params.requestId, now); state.network = now; }
      break;
    case 'Network.loadingFinished':
    case 'Network.loadingFailed':
      if (state.inflight.delete((source.sessionId ?? '') + params.requestId)) state.network = now;
      break;
    case 'Page.fileChooserOpened':
      state.chooser = { backendNodeId: params.backendNodeId, mode: params.mode, frameId: params.frameId, session: source.sessionId };
      break;
    case 'Target.attachedToTarget':
      if (params.targetInfo?.type === 'iframe') {
        state.frames.set(params.targetInfo.targetId, { session: params.sessionId, url: params.targetInfo.url });
        const child = { tabId: source.tabId, sessionId: params.sessionId };
        for (const [command, args] of [['Runtime.enable', {}], ['Network.enable', { maxTotalBufferSize: 1 << 20 }], ['Runtime.addBinding', { name: BINDING }], ['Page.addScriptToEvaluateOnNewDocument', { source: SETUP }], ['Runtime.evaluate', { expression: SETUP }], ['Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }], ['Runtime.runIfWaitingForDebugger', {}]]) {
          chrome.debugger.sendCommand(child, command, args).catch(() => {});
        }
      }
      break;
    case 'Target.detachedFromTarget':
      for (const [frame, entry] of state.frames) if (entry.session === params.sessionId) state.frames.delete(frame);
      break;
    case 'Target.targetInfoChanged':
      if (state.frames.has(params.targetInfo?.targetId)) state.frames.get(params.targetInfo.targetId).url = params.targetInfo.url;
      break;
  }
  wake(state);
});

const liveGroup = async session => {
  const group = groups.get(session);
  if (!group) return null;
  try {
    const live = await chrome.tabGroups.get(group.id);
    return { ...group, windowId: live.windowId };
  } catch {
    groups.delete(session);
    return null;
  }
};

const nameSession = async (session, title) => {
  const text = String(title ?? '').trim();
  if (!text) return { named: false };
  names.set(session, text);
  const group = await liveGroup(session);
  if (group && group.title !== text) {
    await chrome.tabGroups.update(group.id, { title: text });
    groups.set(session, { id: group.id, title: text });
  }
  return { named: true, title: text };
};

const open = async (session, turn, input) => {
  const group = await liveGroup(session);
  const title = String(input.title ?? '').trim() || names.get(session) || group?.title || 'nanocode';
  names.set(session, title);
  let windowId = group?.windowId;
  if (windowId === undefined) {
    try {
      windowId = (await chrome.windows.getLastFocused({ windowTypes: ['normal'] })).id;
    } catch {
      windowId = undefined;
    }
  }
  let tab;
  if (windowId === undefined) {
    const created = await chrome.windows.create({ url: 'about:blank', focused: false });
    windowId = created.id;
    tab = created.tabs[0];
  } else {
    tab = await chrome.tabs.create({ windowId, url: 'about:blank', active: false });
  }
  const groupId = await chrome.tabs.group(group ? { groupId: group.id, tabIds: [tab.id] } : { tabIds: [tab.id], createProperties: { windowId } });
  if (!group) await chrome.tabGroups.update(groupId, { title, color: colorFor(session) });
  else if (group.title !== title) await chrome.tabGroups.update(groupId, { title });
  groups.set(session, { id: groupId, title });
  tabs.set(tab.id, { session, turn });
  await attach(tab.id, session);
  return { tab: tab.id, url: 'about:blank', title: '' };
};

const owned = (session, turn, tabId) => {
  const known = tabs.get(tabId);
  if (!known || known.session !== session) throw new Error("tab is not one of this session's tabs; open one with cua.createBrowserTab() or claim it with browser.user.claimTab()");
  known.turn = turn;
  return tabId;
};

const earlier = (value, results) => {
  if (Array.isArray(value)) return value.map(item => earlier(item, results));
  if (value && typeof value === 'object') {
    if (value.$from) {
      let found = results[value.$from.command]?.result;
      for (const key of String(value.$from.pointer ?? '').split('/').slice(1)) found = found?.[key];
      if (found === undefined) throw new Error('$from ' + value.$from.command + value.$from.pointer + ' is not in an earlier result');
      return found;
    }
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, earlier(item, results)]));
  }
  return value;
};

const applyViewport = async (tabId, viewport) => {
  const target = { tabId };
  if (viewport) await chrome.debugger.sendCommand(target, 'Emulation.setDeviceMetricsOverride', { width: viewport.width, height: viewport.height, deviceScaleFactor: viewport.deviceScaleFactor ?? 0, mobile: !!viewport.mobile });
  else await chrome.debugger.sendCommand(target, 'Emulation.clearDeviceMetricsOverride', {});
};

const attach = async (tabId, session) => {
  if (attached.has(tabId)) return;
  if (attaching.has(tabId)) return attaching.get(tabId);
  const work = (async () => {
    try {
      await chrome.debugger.attach({ tabId }, '1.3');
    } catch (error) {
      if (!String(error?.message ?? error).includes('already attached')) throw error;
    }
    attached.add(tabId);
    const state = page(tabId);
    const target = { tabId };
    const quiet = (command, args) => chrome.debugger.sendCommand(target, command, args ?? {}).catch(() => undefined);
    const tree = await quiet('Page.getFrameTree');
    state.mainFrame = tree?.frameTree?.frame?.id ?? state.mainFrame;
    state.url = tree?.frameTree?.frame?.url ?? state.url;
    await Promise.all([
      quiet('Emulation.setFocusEmulationEnabled', { enabled: true }),
      quiet('Page.enable'),
      quiet('Page.setLifecycleEventsEnabled', { enabled: true }),
      quiet('Runtime.enable'),
      quiet('Log.enable'),
      quiet('Network.enable', { maxTotalBufferSize: 1 << 20, maxResourceBufferSize: 1 << 19 }),
      quiet('Runtime.addBinding', { name: BINDING }),
      quiet('Page.addScriptToEvaluateOnNewDocument', { source: SETUP }),
      quiet('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: false, flatten: true }),
    ]);
    await quiet('Runtime.evaluate', { expression: SETUP });
    const viewport = viewports.get(session ?? tabs.get(tabId)?.session);
    if (viewport) await applyViewport(tabId, viewport).catch(() => {});
  })();
  attaching.set(tabId, work);
  try {
    await work;
  } finally {
    attaching.delete(tabId);
  }
};

const detach = async tabId => {
  if (attached.delete(tabId)) await chrome.debugger.detach({ tabId }).catch(() => {});
  pages.delete(tabId);
};

const settled = (state, options, since) => {
  const now = Date.now();
  if (state.dialog) return true;
  if (now - since < (options.minMs ?? 0)) return false;
  if (state.navigating && now - state.navigating < 5000) return false;
  if (options.load && state.loadAt < since && state.navigations > (options.navigations ?? Infinity)) return false;
  if (state.loading && !(state.domAt >= state.started && now - state.domAt >= (options.load ? 3000 : 1000))) return false;
  if (now - state.mutation < (options.quietMs ?? 100)) return false;
  if (options.networkIdle) {
    if (state.inflight.size && now - state.network < 500) return false;
  } else {
    let busy = 0;
    for (const started of state.inflight.values()) if (now - started < 3000) busy++;
    if (busy && now - state.network < 400) return false;
  }
  return true;
};

const settle = async (tabId, options) => {
  const state = pages.get(tabId);
  if (!state || !options) return null;
  const since = Date.now();
  const deadline = since + (options.timeoutMs ?? 5000);
  while (!settled(state, options, since)) {
    if (Date.now() >= deadline) return { ms: Date.now() - since, timedOut: true };
    await sleep(POLL_MS);
  }
  return { ms: Date.now() - since, ...(state.dialog ? { dialog: state.dialog } : {}) };
};

const cdp = async (tabId, input) => {
  await attach(tabId);
  const state = page(tabId);
  const before = input.before ? await settle(tabId, input.before) : null;
  const results = [];
  for (const command of input.commands ?? []) {
    if (state.dialog && !DIALOG_SAFE.has(command.method)) {
      results.push({ error: 'A JavaScript ' + state.dialog.type + ' dialog is open on this tab (' + JSON.stringify(state.dialog.message) + '); handle it with tab.getJsDialog()', dialog: state.dialog });
      continue;
    }
    let params;
    try {
      params = earlier(command.params ?? {}, results);
    } catch (error) {
      results.push({ error: String(error?.message ?? error) });
      continue;
    }
    const target = command.session ? { tabId, sessionId: command.session } : { tabId };
    const sending = chrome.debugger.sendCommand(target, command.method, params).then(result => ({ result: result ?? {} }), error => ({ error: String(error?.message ?? error) }));
    if (DIALOG_SAFE.has(command.method)) {
      results.push(await sending);
      continue;
    }
    const watch = untilDialog(state);
    const outcome = await Promise.race([sending, watch.promise]);
    watch.cancel();
    results.push(outcome.dialog ? { result: {}, dialogOpened: true } : outcome);
  }
  const after = input.after ? await settle(tabId, input.after) : null;
  return { tab: tabId, results, ...(before ? { before } : {}), ...(after ? { after } : {}), url: state.url, navigations: state.navigations, ...(state.dialog ? { dialog: state.dialog } : {}) };
};

const untilDialog = state => {
  let check;
  const promise = new Promise(resolve => {
    check = () => {
      if (!state.dialog) return;
      state.waiters.delete(check);
      resolve({ dialog: state.dialog });
    };
    state.waiters.add(check);
    check();
  });
  return { promise, cancel: () => state.waiters.delete(check) };
};

const close = async tabId => {
  tabs.delete(tabId);
  await detach(tabId);
  await chrome.tabs.remove(tabId).catch(() => {});
};

const groupTitles = async () => {
  const titles = new Map();
  try {
    for (const group of await chrome.tabGroups.query({})) titles.set(group.id, group.title);
  } catch {}
  return titles;
};

const userTabs = async session => {
  const [all, titles] = await Promise.all([chrome.tabs.query({}), groupTitles()]);
  let focused;
  try {
    focused = (await chrome.windows.getLastFocused({ windowTypes: ['normal'] })).id;
  } catch {}
  return all.filter(tab => tab.id !== undefined && tab.id >= 0).map(tab => {
    const known = tabs.get(tab.id);
    return {
      id: tab.id,
      title: tab.title,
      url: tab.url || tab.pendingUrl,
      windowId: tab.windowId,
      active: tab.active,
      selected: tab.active && tab.windowId === focused,
      ...(tab.lastAccessed ? { lastOpened: new Date(tab.lastAccessed).toISOString() } : {}),
      ...(tab.groupId >= 0 && titles.get(tab.groupId) ? { tabGroup: titles.get(tab.groupId) } : {}),
      ...(known ? { owner: known.session === session ? (known.claimed ? 'claimed' : 'agent') : 'other' } : {}),
    };
  }).sort((a, b) => String(b.lastOpened ?? '').localeCompare(String(a.lastOpened ?? '')));
};

const claim = async (session, turn, tabId) => {
  const tab = await chrome.tabs.get(tabId);
  const known = tabs.get(tabId);
  if (known && known.session !== session) throw new Error('tab ' + tabId + ' belongs to another automation session');
  if (!known) tabs.set(tabId, { session, turn, claimed: true });
  else known.turn = turn;
  await attach(tabId, session);
  return { tab: tabId, url: tab.url, title: tab.title, claimed: !known || !!known.claimed };
};

const history = async input => {
  const queries = Array.isArray(input.queries) && input.queries.length ? input.queries : [''];
  const seen = new Map();
  for (const text of queries) {
    const items = await chrome.history.search({ text: String(text), startTime: input.startTime ?? 0, ...(input.endTime ? { endTime: input.endTime } : {}), maxResults: input.limit ?? 100 });
    for (const item of items) if (!seen.has(item.url) || seen.get(item.url).lastVisitTime < item.lastVisitTime) seen.set(item.url, item);
  }
  return [...seen.values()].sort((a, b) => b.lastVisitTime - a.lastVisitTime).slice(0, input.limit ?? 100).map(item => ({ dateVisited: new Date(item.lastVisitTime).toISOString(), url: item.url, ...(item.title ? { title: item.title } : {}) }));
};

const waitFor = (state, test, timeoutMs) => new Promise((resolve, reject) => {
  const done = () => {
    let value;
    try {
      value = test();
    } catch (error) {
      cleanup();
      reject(error);
      return;
    }
    if (value !== undefined) {
      cleanup();
      resolve(value);
    }
  };
  const timer = setTimeout(() => {
    cleanup();
    reject(new Error('Timed out after ' + timeoutMs + ' ms'));
  }, timeoutMs);
  const cleanup = () => {
    clearTimeout(timer);
    state?.waiters.delete(done);
  };
  state?.waiters.add(done);
  done();
});

const fileChooser = async (tabId, timeoutMs) => {
  await attach(tabId);
  const state = page(tabId);
  state.chooser = null;
  const target = { tabId };
  await chrome.debugger.sendCommand(target, 'Page.setInterceptFileChooserDialog', { enabled: true });
  try {
    return await waitFor(state, () => state.chooser ?? undefined, timeoutMs ?? 30000);
  } finally {
    await chrome.debugger.sendCommand(target, 'Page.setInterceptFileChooserDialog', { enabled: false }).catch(() => {});
  }
};

const downloaded = (id, timeoutMs) => new Promise((resolve, reject) => {
  const check = async () => {
    const [item] = await chrome.downloads.search({ id });
    if (item?.state === 'complete') finish(resolve, { path: item.filename, url: item.finalUrl || item.url, mime: item.mime, bytes: item.fileSize });
    else if (item?.state === 'interrupted') finish(reject, new Error('download interrupted: ' + (item.error ?? 'unknown')));
  };
  const changed = delta => { if (delta.id === id && delta.state) check(); };
  const timer = setTimeout(() => finish(reject, new Error('download did not finish within ' + timeoutMs + ' ms')), timeoutMs);
  const finish = (callback, value) => {
    clearTimeout(timer);
    chrome.downloads.onChanged.removeListener(changed);
    callback(value);
  };
  chrome.downloads.onChanged.addListener(changed);
  check();
});

const nextDownload = timeoutMs => new Promise((resolve, reject) => {
  const created = item => {
    clearTimeout(timer);
    chrome.downloads.onCreated.removeListener(created);
    downloaded(item.id, Math.max(1000, deadline - Date.now())).then(resolve, reject);
  };
  const deadline = Date.now() + timeoutMs;
  const timer = setTimeout(() => {
    chrome.downloads.onCreated.removeListener(created);
    reject(new Error('no download started within ' + timeoutMs + ' ms'));
  }, timeoutMs);
  chrome.downloads.onCreated.addListener(created);
});

const sessionTabs = session => [...tabs].filter(([, tab]) => tab.session === session).map(([tabId]) => tabId);

const step = async ({ session, turn, input }) => {
  if (typeof session !== 'string' || !session) throw new Error('invalid browser session');
  switch (input?.action) {
    case 'open':
      return open(session, turn, input);
    case 'cdp':
      return cdp(owned(session, turn, input.tab), input);
    case 'state': {
      const tabId = owned(session, turn, input.tab);
      const state = page(tabId);
      const tab = await chrome.tabs.get(tabId).catch(() => null);
      return { tab: tabId, url: tab?.url || state.url, title: tab?.title, loading: state.loading, navigations: state.navigations, loadAt: state.loadAt, domAt: state.domAt, started: state.started, now: Date.now(), ...(state.dialog ? { dialog: state.dialog } : {}) };
    }
    case 'close':
      await close(owned(session, turn, input.tab));
      return { tab: input.tab, closed: true };
    case 'keep':
      tabs.get(owned(session, turn, input.tab)).kept = true;
      return { tab: input.tab, kept: true };
    case 'logs': {
      const state = page(owned(session, turn, input.tab));
      return { logs: state.logs };
    }
    case 'frames': {
      const state = page(owned(session, turn, input.tab));
      return { frames: [...state.frames].map(([frameId, entry]) => ({ frameId, ...entry })) };
    }
    case 'fileChooser':
      return fileChooser(owned(session, turn, input.tab), input.timeoutMs);
    case 'userTabs':
      return { tabs: await userTabs(session) };
    case 'claim':
      return claim(session, turn, Number(input.tab));
    case 'history':
      return { entries: await history(input) };
    case 'nameSession':
      return nameSession(session, input.title);
    case 'viewport': {
      if (input.viewport) viewports.set(session, input.viewport);
      else viewports.delete(session);
      for (const tabId of sessionTabs(session)) if (attached.has(tabId)) await applyViewport(tabId, input.viewport).catch(() => {});
      return { viewport: input.viewport ?? null };
    }
    case 'waitDownload':
      return nextDownload(input.timeoutMs ?? 30000);
    case 'download': {
      const id = await chrome.downloads.download({ url: input.url, conflictAction: 'uniquify', saveAs: false });
      return downloaded(id, input.timeoutMs ?? 120000);
    }
    default:
      throw new Error('unknown browser action ' + input?.action);
  }
};

const endTurn = async (session, turn) => {
  for (const [tabId, tab] of [...tabs]) {
    if (tab.session !== session || tab.turn !== turn) continue;
    if (tab.kept || tab.claimed) await detach(tabId);
    else await close(tabId);
  }
};

const handle = async message => {
  if (message.type === 'hello') {
    send({ type: 'hello', protocol: PROTOCOL, browser: { name: brand(), version: navigator.userAgentData?.brands?.find(entry => /Chrom|Brave|Edge/.test(entry.brand))?.version ?? '' }, extension: chrome.runtime.getManifest().version });
    return;
  }
  if (message.type === 'disconnected') {
    for (const [tabId, tab] of [...tabs]) if (!tab.kept && !tab.claimed) await close(tabId);
    release();
    return;
  }
  if (message.type === 'turn_ended') {
    await endTurn(message.session, message.turn);
    return;
  }
  if (message.type !== 'call') return;
  try {
    send({ type: 'result', id: message.id, ok: true, result: await step(message) });
  } catch (error) {
    send({ type: 'result', id: message.id, ok: false, error: String(error?.message ?? error) });
  }
};

chrome.tabs.onRemoved.addListener(tabId => {
  tabs.delete(tabId);
  attached.delete(tabId);
  pages.delete(tabId);
});
chrome.debugger.onDetach.addListener(({ tabId }) => {
  attached.delete(tabId);
  pages.delete(tabId);
});
chrome.alarms.create('connect', { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(connect);
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
chrome.action.onClicked.addListener(connect);
connect();
