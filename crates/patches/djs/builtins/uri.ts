// The URI functions of the global object (19.2.6) and Annex B's escape/unescape
// (B.2.1). The work is in leaf intrinsics (crates/djs-core/src/builtins/uri.rs).

/**
 * decodeURI ( encodedURI ), 19.2.6.1.
 * @builtin globalThis.decodeURI
 */
function GlobalDecodeURI(encodedURI: Value): Str {
  return UriDecode(ToString(encodedURI), false);
}

/**
 * decodeURIComponent ( encodedURIComponent ), 19.2.6.2.
 * @builtin globalThis.decodeURIComponent
 */
function GlobalDecodeURIComponent(encodedURIComponent: Value): Str {
  return UriDecode(ToString(encodedURIComponent), true);
}

/**
 * encodeURI ( uri ), 19.2.6.3.
 * @builtin globalThis.encodeURI
 */
function GlobalEncodeURI(uri: Value): Str {
  return UriEncode(ToString(uri), false);
}

/**
 * encodeURIComponent ( uriComponent ), 19.2.6.4.
 * @builtin globalThis.encodeURIComponent
 */
function GlobalEncodeURIComponent(uriComponent: Value): Str {
  return UriEncode(ToString(uriComponent), true);
}

/**
 * escape ( string ), B.2.1.1.
 * @builtin globalThis.escape
 */
function GlobalEscape(string: Value): Str {
  return StrEscape(ToString(string));
}

/**
 * unescape ( string ), B.2.1.2.
 * @builtin globalThis.unescape
 */
function GlobalUnescape(string: Value): Str {
  return StrUnescape(ToString(string));
}
