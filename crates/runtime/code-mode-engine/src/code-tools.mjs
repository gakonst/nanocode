

const ignore = () => {};
export function createCodeTools(names, invoke) {
  const helpers = {

    observe(promise) {
      promise.catch(ignore);
      return promise;
    },
    dispatch(name, input) {
      try { return invoke(name, input); } catch (error) { return Promise.reject(error); }
    },
    async unavailable(name) {
      const error = new Error(`TOOL_NOT_AVAILABLE: Tool "${name}" is not available in this Code Mode execution. Check ALL_TOOLS for callable names. A tool exposed directly by the host may need to be called through its direct tool entry instead.`);
      error.code = "TOOL_NOT_AVAILABLE";
      error.tool = name;
      throw error;
    },
  };
  const missing = new Proxy(Object.create(null), {
    get(_, name) {


      if (typeof name !== "string" || name === "then") return undefined;
      const deferred = /^(mcp__.+?__)(.+)$/.exec(name);
      const caller = deferred && deferred[1] + "call_tool";
      if (caller && Object.prototype.hasOwnProperty.call(target, caller)) {
        return (input) => target[caller]({ name: deferred[2], arguments: input ?? {} });
      }
      return {
        missing(input) {
          if (name in Object.prototype || name === "toJSON") {
            return helpers.observe(helpers.unavailable(name));
          }
          return helpers.observe(helpers.dispatch(name, input));
        },
      }.missing;
    },
  });
  const target = Object.create(missing);
  for (const name of names) {
    Object.defineProperty(target, name, {
      enumerable: true,
      value(input) {
        return helpers.observe(helpers.dispatch(name, input));
      },
    });
  }
  return Object.freeze(target);
}
