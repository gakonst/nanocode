const HOST = 'xyz.nanocode.browser';
const COLORS = ['purple', 'blue', 'cyan', 'green', 'orange', 'pink', 'yellow', 'red', 'grey'];
const tabs = new Map();
const groups = new Map();
const attached = new Set();
let port = null;

const send = message => port?.postMessage(message);

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
  groups.clear();
};

const colorFor = session => {
  let hash = 0;
  for (const char of session) hash = (hash * 31 + char.charCodeAt(0)) | 0;
  return COLORS[Math.abs(hash) % COLORS.length];
};

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

const open = async (session, turn, input) => {
  const group = await liveGroup(session);
  const title = String(input.title ?? '').trim() || group?.title || 'nanocode';
  const url = input.url ?? 'about:blank';
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
    const created = await chrome.windows.create({ url, focused: false });
    windowId = created.id;
    tab = created.tabs[0];
  } else {
    tab = await chrome.tabs.create({ windowId, url, active: false });
  }
  const groupId = await chrome.tabs.group(group ? { groupId: group.id, tabIds: [tab.id] } : { tabIds: [tab.id], createProperties: { windowId } });
  if (!group) await chrome.tabGroups.update(groupId, { title, color: colorFor(session) });
  else if (group.title !== title) await chrome.tabGroups.update(groupId, { title });
  groups.set(session, { id: groupId, title });
  tabs.set(tab.id, { session, turn });
  return { tab: tab.id, url, title: '' };
};

const owned = (session, turn, tabId) => {
  const known = tabs.get(tabId);
  if (!known || known.session !== session) throw new Error("tab is not one of this session's tabs; open one with cua.createBrowserTab()");
  known.turn = turn;
  return tabId;
};

const earlier = (value, results) => {
  if (Array.isArray(value)) return value.map(item => earlier(item, results));
  if (value && typeof value === 'object') {
    if (value.$from) {
      let found = results[value.$from.command]?.result;
      for (const key of String(value.$from.pointer ?? '').split('/').slice(1)) found = found?.[key];
      if (found === undefined) throw new Error(`$from ${value.$from.command}${value.$from.pointer} is not in an earlier result`);
      return found;
    }
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, earlier(item, results)]));
  }
  return value;
};

const attach = async tabId => {
  if (attached.has(tabId)) return;
  try {
    await chrome.debugger.attach({ tabId }, '1.3');
  } catch (error) {
    if (!String(error?.message ?? error).includes('already attached')) throw error;
  }
  attached.add(tabId);
  await chrome.debugger.sendCommand({ tabId }, 'Emulation.setFocusEmulationEnabled', { enabled: true }).catch(() => {});
};

const cdp = async (tabId, commands) => {
  await attach(tabId);
  const results = [];
  for (const command of commands) {
    try {
      const result = await chrome.debugger.sendCommand({ tabId }, command.method, earlier(command.params ?? {}, results));
      results.push({ result: result ?? {} });
    } catch (error) {
      results.push({ error: String(error?.message ?? error) });
    }
  }
  return { tab: tabId, results };
};

const close = async tabId => {
  tabs.delete(tabId);
  if (attached.delete(tabId)) await chrome.debugger.detach({ tabId }).catch(() => {});
  await chrome.tabs.remove(tabId).catch(() => {});
};

const step = async ({ session, turn, input }) => {
  if (typeof session !== 'string' || !session) throw new Error('invalid browser session');
  switch (input?.action) {
    case 'open':
      return open(session, turn, input);
    case 'cdp':
      return cdp(owned(session, turn, input.tab), input.commands ?? []);
    case 'close':
      await close(owned(session, turn, input.tab));
      return { tab: input.tab, closed: true };
    case 'keep':
      tabs.get(owned(session, turn, input.tab)).kept = true;
      return { tab: input.tab, kept: true };
    default:
      throw new Error(`unknown browser action ${input?.action}`);
  }
};

const handle = async message => {
  if (message.type === 'disconnected') {
    for (const [tabId, tab] of [...tabs]) if (!tab.kept) await close(tabId);
    release();
    return;
  }
  if (message.type === 'turn_ended') {
    for (const [tabId, tab] of [...tabs]) if (tab.session === message.session && tab.turn === message.turn && !tab.kept) await close(tabId);
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
});
chrome.debugger.onDetach.addListener(({ tabId }) => attached.delete(tabId));
chrome.alarms.create('connect', { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(connect);
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
chrome.action.onClicked.addListener(connect);
connect();
