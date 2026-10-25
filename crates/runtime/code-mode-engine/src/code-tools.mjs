


export function createCodeTools(names, invoke) {
  const helpers = {

    observe(promise) {
      promise.catch(() => {});
      return promise;
    },
    async dispatch(name, input) {
      return invoke(name, input);
    },
    async unavailable(name) {
      const error = new Error(`TOOL_NOT_AVAILABLE: Tool "${name}" is not available in this Code Mode execution. Check ALL_TOOLS for callable names. A tool exposed directly by the host may need to be called through its direct tool entry instead.`);
      error.code = "TOOL_NOT_AVAILABLE";
      error.tool = name;
      throw error;
    },
  };
  const target = Object.create(null);
  for (const name of names) {
    Object.defineProperty(target, name, {
      enumerable: true,
      value(input) {
        return helpers.observe(helpers.dispatch(name, input));
      },
    });
  }
  Object.freeze(target);
  return new Proxy(target, {
    get(target, name) {
      if (Object.prototype.hasOwnProperty.call(target, name)) return target[name];


      if (typeof name !== "string" || name === "then") return undefined;
      const deferred = /^(mcp__.+?__)(.+)$/.exec(name);
      const caller = deferred && deferred[1] + "call_tool";
      if (caller && Object.prototype.hasOwnProperty.call(target, caller)) {
        return (input) => target[caller]({ name: deferred[2], arguments: input ?? {} });
      }
      return {
        missing() {
          return helpers.observe(helpers.unavailable(name));
        },
      }.missing;
    },
  });
}
