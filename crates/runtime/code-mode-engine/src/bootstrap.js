((createCodeTools, valueHelpers) => {
  const { stringify, storeSnapshot, normalizeImage, normalizeAudio, generatedImageItems } = valueHelpers;
  const native = (name) => {
    const value = globalThis[name];
    delete globalThis[name];
    return value;
  };
  const nativeTool = native("__nanocodeTool");
  const nativeContent = native("__nanocodeContent");
  const nativeText = native("__nanocodeText");
  const nativeLoad = native("__nanocodeLoad");
  const nativeNotify = native("__nanocodeNotify");
  const nativeYield = native("__nanocodeYield");
  const nativeSetTimeout = native("__nanocodeSetTimeout");
  const nativeClearTimeout = native("__nanocodeClearTimeout");
  const nativeValue = native("__nanocodeValue");
  const nativeSettled = native("__nanocodeSettled");
  const nativeNext = native("__nanocodeNext");
  const nativeCatalog = native("__nanocodeCatalog");
  const definitionsJson = native("__nanocodeDefinitions");
  const preludeSource = native("__nanocodePrelude");
  const nativeBytes = {
    put: native("__nanocodeBytesPut"),
    take: native("__nanocodeBytesTake"),
    text: native("__nanocodeBytesText"),
    discard: native("__nanocodeBytesDiscard"),
    decode: native("__nanocodeBytesDecode"),
    encode: native("__nanocodeBytesEncode"),
    toBase64: native("__nanocodeBytesToBase64"),
    fromBase64: native("__nanocodeBytesFromBase64"),
    digest: native("__nanocodeBytesDigest"),
    random: native("__nanocodeBytesRandom"),
  };
  if (nativeBytes.put) {
    globalThis.__nanocodeBytes = {
      put: nativeBytes.put,
      take: (id) => nativeBytes.take(id).buffer,
      text: nativeBytes.text,
      discard: nativeBytes.discard,
      decode: nativeBytes.decode,
      encode: (value) => nativeBytes.encode(value).buffer,
      toBase64: nativeBytes.toBase64,
      fromBase64: (text) => nativeBytes.fromBase64(text).buffer,
      digest: (algorithm, buffer, offset, length) => nativeBytes.digest(algorithm, buffer, offset, length).buffer,
      random: (length) => nativeBytes.random(length).buffer,
    };
  }
  for (const name of ["console", "Atomics", "SharedArrayBuffer", "WebAssembly"]) delete globalThis[name];
  const OriginalDate = Date;
  const clockNow = () => nativeValue(0);
  function JournaledDate(...args) {
    if (new.target === undefined) return new OriginalDate(clockNow()).toString();
    return Reflect.construct(OriginalDate, args.length === 0 ? [clockNow()] : args, new.target);
  }
  Object.setPrototypeOf(JournaledDate, OriginalDate);
  JournaledDate.prototype = OriginalDate.prototype;
  Object.defineProperty(JournaledDate, "name", { value: "Date", configurable: true });
  Object.defineProperty(JournaledDate, "length", { value: 7, configurable: true });
  Object.defineProperty(JournaledDate, "now", { value: clockNow, writable: true, configurable: true });
  OriginalDate.prototype.constructor = JournaledDate;
  Object.defineProperty(globalThis, "Date", { value: JournaledDate, writable: true, configurable: true });
  const originalRandom = Math.random;
  Object.defineProperty(Math, "random", { value: () => nativeValue(2, originalRandom()), writable: true, configurable: true });
  const journaledPerformance = Object.create(null, {
    now: { value: () => nativeValue(1), writable: true, configurable: true },
  });
  Object.defineProperty(globalThis, "performance", { value: journaledPerformance, writable: true, configurable: true, enumerable: true });
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const bodyLine = Number(/<input>:(\d+):/.exec(new Function("return new Error().stack")())?.[1] ?? 1);
  const jsonParse = JSON.parse;
  const jsonStringify = JSON.stringify;
  const objectToString = Object.prototype.toString;
  const hasOwn = Object.prototype.hasOwnProperty;
  const cloneValue = typeof globalThis.structuredClone === "function"
    ? globalThis.structuredClone
    : (value) => value === undefined
      ? undefined
      : jsonParse(jsonStringify(value));
  const now = typeof globalThis.performance?.now === "function"
    ? () => globalThis.performance.now()
    : () => Date.now();
  const EXIT = Symbol("exit");
  const pendingTools = new Map();
  const timers = new Map();
  let stopCell = () => {};
  let settle = () => {};
  let failure;

  function isError(value) {
    return value instanceof Error || objectToString.call(value) === "[object Error]";
  }

  function indent(text, prefix = "  ") {
    return text.replace(/^/gm, prefix);
  }

  function located(stack) {
    return stack.replace(/<input>:(\d+):/g, (match, line) => "<input>:" + (line - bodyLine + 1) + ":");
  }

  function errorText(error, seen = []) {
    const name = error.name === undefined ? "Error" : String(error.name);
    const message = error.message === undefined ? "" : String(error.message);
    const label = message ? name + ": " + message : name;
    const stack = typeof error.stack === "string" ? located(error.stack.replace(/\n+$/, "")) : "";
    let text = !stack ? label : stack.startsWith(label) || /^\S/.test(stack) && !/^\s*at /.test(stack) ? stack : label + "\n" + stack;
    if (seen.length >= 8 || seen.includes(error)) return text;
    const inner = [...seen, error];
    const details = [];
    if (hasOwn.call(error, "cause")) details.push("[cause]: " + describe(error.cause, inner));
    if (typeof AggregateError === "function" && error instanceof AggregateError && Array.isArray(error.errors)) {
      details.push(error.errors.length
        ? "[errors]: [\n" + error.errors.map((item) => indent(describe(item, inner))).join(",\n") + "\n]"
        : "[errors]: []");
    }
    if (details.length) text += " {\n" + details.map((detail) => indent(detail)).join("\n") + "\n}";
    return text;
  }

  function special(value) {
    if (typeof value === "bigint") return value + "n";
    if (typeof value === "symbol") return value.toString();
    if (typeof value === "function") {
      return /^class[\s{]/.test(Function.prototype.toString.call(value))
        ? "[class " + (value.name || "(anonymous)") + "]"
        : "[Function: " + (value.name || "(anonymous)") + "]";
    }
    if (value === null || typeof value !== "object") return value;
    if (isError(value)) return errorText(value);
    if (value instanceof Map) {
      return [...value.keys()].every((key) => typeof key === "string" || typeof key === "number")
        ? Object.fromEntries(value)
        : [...value.entries()];
    }
    if (value instanceof Set) return [...value];
    if (value instanceof RegExp) return String(value);
    if (value instanceof Promise) return "Promise";
    if (value instanceof WeakMap || value instanceof WeakSet) return objectToString.call(value).slice(8, -1);
    if (value instanceof ArrayBuffer) return "ArrayBuffer(" + value.byteLength + ")";
    if (ArrayBuffer.isView(value) && !(value instanceof DataView)) {
      return value.length <= 1000
        ? Array.from(value, (item) => typeof item === "bigint" ? item + "n" : item)
        : value.constructor.name + "(" + value.length + ")";
    }
    return value;
  }

  function encode(value, limit) {
    const depths = new Map();
    const path = [];
    const onPath = new Set();
    return jsonStringify(value, function (key, item) {
      const depth = (depths.get(this) ?? -1) + 1;
      const original = item;
      item = special(item);
      if (item === null || typeof item !== "object") return item;
      while (path.length > depth) onPath.delete(path.pop());
      if (onPath.has(original) || onPath.has(item)) return "[Circular]";
      if (depth > limit) return Array.isArray(item) ? "[Array]" : "[Object]";
      path.push(original);
      onPath.add(original);
      onPath.add(item);
      depths.set(item, depth);
      return item;
    });
  }

  function inspect(value, limit = Infinity) {
    if (typeof value === "string") return jsonStringify(value);
    if (typeof value === "bigint" || typeof value === "symbol" || typeof value === "function") return special(value);
    if (value === null || typeof value !== "object") return String(value);
    if (isError(value)) return errorText(value);
    try {
      return encode(value, limit) ?? String(value);
    } catch (error) {
      if (!(error instanceof RangeError) || limit <= 64) return String(value);
      return inspect(value, 64);
    }
  }

  function describe(value, seen = []) {
    return isError(value) ? errorText(value, seen) : inspect(value);
  }

  function thrownText(value) {
    if (isError(value)) return errorText(value);
    if (typeof value === "string" && value) return value;
    return "Uncaught " + inspect(value);
  }

  function uncaught(reason, rejection) {
    if (reason === EXIT) {
      stopCell();
      return undefined;
    }
    const message = rejection
      ? "Unhandled promise rejection: " + describe(reason)
      : thrownText(reason);
    failure ??= message;
    stopCell();
    return message;
  }

  function failureText(payload) {
    if (typeof payload === "string") return payload;
    if (payload === null || typeof payload !== "object") return payload === undefined ? "" : jsonStringify(payload);
    if (Array.isArray(payload.content)) {
      const texts = payload.content
        .map((item) => typeof item?.text === "string" ? item.text : "")
        .filter(Boolean);
      if (texts.length) return texts.join("\n");
    }
    for (const key of ["message", "error", "detail"]) {
      if (typeof payload[key] === "string" && payload[key]) return payload[key];
    }
    return jsonStringify(payload) ?? "";
  }

  function toolFailure(name, payload) {
    if (payload instanceof Error) return payload;
    const message = failureText(payload) || `tool ${name} failed`;
    const error = new Error(message);
    Object.defineProperty(error, "message", { value: message, enumerable: true, configurable: true, writable: true });
    error.stack = `Error: ${message}`;
    error.tool = name;
    if (payload !== null && typeof payload === "object") {
      for (const key of Object.keys(payload)) {
        if (key !== "message" && key !== "name" && key !== "stack") error[key] = payload[key];
      }
      Object.defineProperty(error, "detail", { value: payload, configurable: true, writable: true });
    }
    return error;
  }

  function storageKey(value, helper) {
    try {
      return `${value}`;
    } catch {
      throw `${helper} key must be a string`;
    }
  }

  function format(values) {
    if (values.length === 1 && typeof values[0] === "string") return values[0];
    const [first, ...rest] = values;
    let head;
    if (typeof first === "string" && rest.length) {
      head = first.replace(/%([sdifjoOc%])/g, (match, kind) => {
        if (kind === "%") return "%";
        if (!rest.length) return match;
        const value = rest.shift();
        if (kind === "s") {
          if (typeof value === "string") return value;
          if (typeof value === "bigint") return value + "n";
          if (typeof value === "symbol") return value.toString();
          return value !== null && typeof value === "object" && !isError(value) ? inspect(value, 1) : describe(value);
        }
        if (kind === "d" || kind === "i") {
          if (typeof value === "bigint") return value + "n";
          if (typeof value === "symbol" || value !== null && typeof value === "object") return "NaN";
          return String(kind === "i" ? parseInt(value) : Number(value));
        }
        if (kind === "f") return typeof value === "symbol" ? "NaN" : String(parseFloat(value));
        if (kind === "j") {
          try {
            return jsonStringify(value) ?? "undefined";
          } catch (error) {
            return error instanceof TypeError && /circular/i.test(error.message) ? "[Circular]" : String(error);
          }
        }
        if (kind === "c") return "";
        return describe(value);
      });
    } else if (first !== undefined || values.length) {
      head = typeof first === "string" ? first : describe(first);
    } else {
      head = "";
    }
    return [head, ...rest.map((value) => typeof value === "string" ? value : describe(value))].join(" ");
  }

  function tableCell(value) {
    if (typeof value === "string") return "'" + value + "'";
    return describe(value);
  }

  function tableText(data, columns) {
    const rows = data instanceof Map
      ? [...data.entries()]
      : data instanceof Set
        ? [...data].map((value, index) => [index, value])
        : Object.entries(data);
    const keys = [];
    let values = false;
    for (const [, row] of rows) {
      if (row !== null && typeof row === "object") {
        for (const key of Object.keys(row)) if (!keys.includes(key)) keys.push(key);
      } else {
        values = true;
      }
    }
    const chosen = Array.isArray(columns) ? columns.map(String) : keys;
    const header = ["(index)", ...chosen, ...(values ? ["Values"] : [])];
    const body = rows.map(([index, row]) => {
      const object = row !== null && typeof row === "object";
      return [
        String(index),
        ...chosen.map((key) => object && key in row ? tableCell(row[key]) : ""),
        ...(values ? [object ? "" : tableCell(row)] : []),
      ];
    });
    const widths = header.map((title, index) => Math.max(title.length, ...body.map((cells) => cells[index].length)));
    const rule = (left, middle, right) => left + widths.map((width) => "─".repeat(width + 2)).join(middle) + right;
    const line = (cells) => "│" + cells.map((cell, index) => " " + cell.padEnd(widths[index]) + " ").join("│") + "│";
    return [rule("┌", "┬", "┐"), line(header), rule("├", "┼", "┤"), ...body.map(line), rule("└", "┴", "┘")].join("\n");
  }

  function elapsed(start) {
    const ms = now() - start;
    return ms >= 1000 ? (ms / 1000).toFixed(3) + "s" : ms.toFixed(3) + "ms";
  }

  function createConsole(text) {
    let prefix = "";
    const counts = new Map();
    const timers = new Map();
    const print = (line) => text(prefix ? indent(line, prefix) : line);
    const log = (...values) => print(format(values));
    const label = (value) => value === undefined ? "default" : String(value);
    return Object.freeze({
      log,
      info: log,
      warn: log,
      error: log,
      debug: log,
      trace(...values) {
        const frames = located(String(new Error().stack ?? "")).split("\n").filter((line) => /^\s+at /.test(line)).slice(1);
        print(["Trace" + (values.length ? ": " + format(values) : ""), ...frames].join("\n"));
      },
      dir(value, options) {
        const depth = options?.depth === null ? Infinity : typeof options?.depth === "number" ? options.depth : 2;
        print(isError(value) ? errorText(value) : inspect(value, depth));
      },
      dirxml: log,
      table(data, columns) {
        if (data === null || typeof data !== "object") return log(data);
        print(tableText(data, columns));
      },
      assert(condition, ...values) {
        if (condition) return;
        if (typeof values[0] === "string") values[0] = "Assertion failed: " + values[0];
        else values.unshift("Assertion failed");
        log(...values);
      },
      count(name) {
        const key = label(name);
        const count = (counts.get(key) ?? 0) + 1;
        counts.set(key, count);
        print(key + ": " + count);
      },
      countReset(name) {
        const key = label(name);
        if (!counts.delete(key)) print("Warning: Count for '" + key + "' does not exist");
      },
      group(...values) {
        if (values.length) log(...values);
        prefix += "  ";
      },
      groupCollapsed(...values) {
        if (values.length) log(...values);
        prefix += "  ";
      },
      groupEnd() {
        prefix = prefix.slice(2);
      },
      time(name) {
        const key = label(name);
        if (timers.has(key)) return print("Warning: Label '" + key + "' already exists for console.time()");
        timers.set(key, now());
      },
      timeLog(name, ...values) {
        const key = label(name);
        if (!timers.has(key)) return print("Warning: No such label '" + key + "' for console.timeLog()");
        print(key + ": " + elapsed(timers.get(key)) + (values.length ? " " + format(values) : ""));
      },
      timeEnd(name) {
        const key = label(name);
        if (!timers.has(key)) return print("Warning: No such label '" + key + "' for console.timeEnd()");
        print(key + ": " + elapsed(timers.get(key)));
        timers.delete(key);
      },
      clear() {},
    });
  }

  function setTimeout(callback, delay, ...args) {
    if (typeof callback !== "function") throw "setTimeout expects a function callback";
    const id = nativeSetTimeout(Number(delay));
    timers.set(id, () => {
      try {
        callback(...args);
      } catch (error) {
        uncaught(error, false);
      }
    });
    return id;
  }

  function clearTimeout(value) {
    if (value === null || value === undefined) return;
    let id;
    try {
      id = Number(value);
    } catch {
      throw "clearTimeout expects a numeric timeout id";
    }
    if (!Number.isFinite(id) || id <= 0) return;
    id = Math.trunc(id);
    timers.delete(id);
    nativeClearTimeout(id);
  }

  function callTool(name, encodedInput, host) {
    const id = nativeTool(name, encodedInput, host);
    return new Promise((resolve, reject) => {
      pendingTools.set(id, [resolve, reject]);
    });
  }

  async function pump() {
    for (;;) {
      const event = await nativeNext();
      if (event === null) {
        settle();
      } else if (typeof event === "number") {
        const fire = timers.get(event);
        timers.delete(event);
        if (fire) fire();
      } else if (Array.isArray(event)) {
        for (const reason of event) uncaught(reason, true);
      } else {
        const settlers = pendingTools.get(event.id);
        pendingTools.delete(event.id);
        if (settlers) settlers[event.ok ? 0 : 1](event.value);
      }
    }
  }

  const prepareCell = function () {
    const definitions = jsonParse(definitionsJson);
    let descriptions;
    let schemas;
    const stored = new Map();
    const storedWrites = new Map();
    const declaredTools = Object.create(null);
    const stopped = new Promise((resolve) => {
      stopCell = resolve;
    });
    const finish = async (promise) => {
      try {
        await Promise.race([promise, stopped]);
      } catch (error) {
        if (error !== EXIT) throw error;
      }
      await new Promise((resolve) => {
        settle = resolve;
        nativeSettled();
      });
    };
    const terminal = () => failure === undefined
      ? jsonStringify({ type: "done", stored: Object.fromEntries(storedWrites) })
      : jsonStringify({ type: "error", message: failure, stored: Object.fromEntries(storedWrites) });
    const invokeTool = (name, input, host = false) => {
      const encodedInput = input === undefined ? "null" : jsonStringify(input);
      return callTool(name, encodedInput, host)
        .then(jsonParse, (payload) => Promise.reject(toolFailure(name, jsonParse(payload))));
    };
    for (const definition of definitions) {
      declaredTools[definition.name] = (input) => invokeTool(
        definition.tool_name,
        input === undefined && definition.kind === "function" ? {} : input,
      );
    }
    const tools = createCodeTools(
      Object.keys(declaredTools),
      (name, input) => declaredTools[name] ? declaredTools[name](input) : invokeTool(name, input),
    );

    function text(value) {
      nativeText(stringify(value));
    }

    const console = createConsole(text);

    function notify(value) {
      const notification = stringify(value);
      if (!notification.trim()) throw "notify expects non-empty text";
      nativeNotify(notification);
    }

    function image(value, detail) {
      if (!Array.isArray(value)) return nativeContent(jsonStringify(normalizeImage(value, detail)));
      if (!value.length) throw "image expects at least one image item in an array";
      for (const item of value.map((entry) => normalizeImage(entry, detail))) nativeContent(jsonStringify(item));
    }

    function audio(value) {
      nativeContent(jsonStringify(normalizeAudio(value)));
    }

    function generatedImage(value) {
      for (const item of generatedImageItems(value)) {
        nativeContent(jsonStringify(item));
      }
    }

    function store(key, value) {
      const normalizedKey = storageKey(key, "store");
      const [, normalizedValue] = storeSnapshot(normalizedKey, value);
      stored.set(normalizedKey, normalizedValue);
      storedWrites.set(normalizedKey, normalizedValue);
    }

    function load(key) {
      const normalizedKey = storageKey(key, "load");
      if (stored.has(normalizedKey)) return cloneValue(stored.get(normalizedKey));
      const encoded = nativeLoad(normalizedKey);
      return encoded === undefined ? undefined : jsonParse(encoded);
    }

    function yield_control() {
      nativeYield();
    }

    function exit() { throw EXIT; }

    const describe = (index) => (descriptions ??= jsonParse(nativeCatalog(0)))[index];
    const allTools = Object.freeze(definitions.map((tool, index) => {
      return Object.freeze(Object.defineProperty({ name: tool.name }, "description", {
        enumerable: true,
        get: () => describe(index),
      }));
    }));
    function toolSchema(name) {
      if (typeof name !== "string" || !name) {
        throw "toolSchema expects a non-empty tool name";
      }
      const index = definitions.findIndex((tool) =>
        tool.name === name || tool.tool_name === name
      );
      if (index < 0) return undefined;
      schemas ??= jsonParse(nativeCatalog(1));
      const [inputSchema, outputSchema] = schemas[index];
      return cloneValue({
        inputSchema: inputSchema ?? null,
        outputSchema: outputSchema ?? null,
      });
    }
    const cellGlobals = {
      tools,
      ALL_TOOLS: allTools,
      toolSchema,
      text,
      image,
      audio,
      generatedImage,
      notify,
      store,
      load,
      yield_control,
      exit,
      setTimeout,
      clearTimeout,
      console,
    };
    const defineGlobals = (globals) => {
      for (const name of Object.keys(globals)) {
        Object.defineProperty(globalThis, name, {
          value: globals[name],
          writable: true,
          configurable: true,
          enumerable: false,
        });
      }
    };
    defineGlobals(cellGlobals);
    return async function runCell(source) {
      pump();
      try {
        if (preludeSource) {
          defineGlobals(preludeSource(tools, (name, input) => invokeTool(name, input, true)) ?? {});
        }
        const script = new AsyncFunction(source);
        await finish(script());
        return terminal();
      } catch (error) {
        return jsonStringify({
          type: "error",
          message: failure ?? thrownText(error),
          stored: Object.fromEntries(storedWrites),
        });
      }
    };
  };
  return prepareCell();
})
