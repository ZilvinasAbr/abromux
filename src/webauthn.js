// Passkeys and security keys for browser panes (see webauthn.m).
//
// WKWebView refuses WebAuthn for sites outside the app's associated domains,
// so this replaces navigator.credentials.create/get for public-key requests:
// the options go to the native bridge, which runs the ceremony with
// AuthenticationServices, and its result comes back shaped like a real
// PublicKeyCredential. Runs at document start in the top frame only.
(() => {
  if (window.top !== window.self || !window.isSecureContext || window.__abromuxWebAuthn) {
    return;
  }
  window.__abromuxWebAuthn = true;
  const handlers = window.webkit && window.webkit.messageHandlers;
  const handler = handlers && handlers.abromuxWebAuthn;
  const container = window.CredentialsContainer && window.CredentialsContainer.prototype;
  if (!handler || !container) {
    return;
  }
  // Captured now, before page scripts can replace them.
  const post = handler.postMessage.bind(handler);
  const originalCreate = container.create;
  const originalGet = container.get;
  const PKC = window.PublicKeyCredential;

  const bytes = (v) =>
    v instanceof ArrayBuffer
      ? new Uint8Array(v)
      : ArrayBuffer.isView(v)
        ? new Uint8Array(v.buffer, v.byteOffset, v.byteLength)
        : null;
  const encode = (v) => {
    const b = bytes(v);
    if (!b) return undefined;
    let s = "";
    for (const c of b) s += String.fromCharCode(c);
    return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  };
  const decode = (s) => {
    if (typeof s !== "string") return null;
    const b = atob(s.replace(/-/g, "+").replace(/_/g, "/"));
    const out = new Uint8Array(b.length);
    for (let i = 0; i < b.length; i++) out[i] = b.charCodeAt(i);
    return out.buffer;
  };
  const str = (v) => (typeof v === "string" ? v : undefined);
  const lower = (v) => (typeof v === "string" ? v.trim().toLowerCase() : undefined);
  const descriptor = (d) =>
    d && encode(d.id)
      ? {
          type: lower(d.type) || "public-key",
          id: encode(d.id),
          transports: Array.isArray(d.transports) ? d.transports.map(lower).filter(Boolean) : [],
        }
      : null;
  const descriptors = (list) => (Array.isArray(list) ? list.map(descriptor).filter(Boolean) : []);

  const error = (name, message) => {
    if (name === "TypeError") return new TypeError(message);
    return new DOMException(message || "The operation failed.", name || "UnknownError");
  };

  const call = (kind, payload, signal) =>
    new Promise((resolve, reject) => {
      if (signal && signal.aborted) {
        reject(signal.reason !== undefined ? signal.reason : error("AbortError", "Aborted."));
        return;
      }
      const onAbort = () => {
        post({ kind: "cancel" }).catch(() => {});
        reject(signal.reason !== undefined ? signal.reason : error("AbortError", "Aborted."));
      };
      if (signal) signal.addEventListener("abort", onAbort, { once: true });
      post({ kind, payload: JSON.stringify(payload) }).then(
        (reply) => {
          if (signal) signal.removeEventListener("abort", onAbort);
          if (reply && reply.ok) resolve(reply);
          else {
            const e = (reply && reply.error) || {};
            reject(error(e.name, e.message));
          }
        },
        () => reject(error("UnknownError", "The passkey request failed.")),
      );
    });

  const withProto = (obj, ctor) => {
    if (ctor && ctor.prototype) Object.setPrototypeOf(obj, ctor.prototype);
    return obj;
  };

  const credential = (c) => {
    const r = c.response || {};
    const extensions = c.clientExtensionResults || {};
    let response;
    if (c.responseKind === "attestation") {
      const transports = Array.isArray(r.transports) ? r.transports : [];
      response = withProto(
        {
          clientDataJSON: decode(r.clientDataJSON),
          attestationObject: decode(r.attestationObject),
          getTransports: () => [...transports],
          getAuthenticatorData: () => null,
          getPublicKey: () => null,
          getPublicKeyAlgorithm: () => null,
          toJSON: () => ({
            clientDataJSON: r.clientDataJSON,
            attestationObject: r.attestationObject,
            transports: [...transports],
          }),
        },
        window.AuthenticatorAttestationResponse,
      );
    } else {
      response = withProto(
        {
          clientDataJSON: decode(r.clientDataJSON),
          authenticatorData: decode(r.authenticatorData),
          signature: decode(r.signature),
          userHandle: r.userHandle ? decode(r.userHandle) : null,
          toJSON: () => ({
            clientDataJSON: r.clientDataJSON,
            authenticatorData: r.authenticatorData,
            signature: r.signature,
            userHandle: r.userHandle || null,
          }),
        },
        window.AuthenticatorAssertionResponse,
      );
    }
    return withProto(
      {
        type: "public-key",
        id: c.id,
        rawId: decode(c.id),
        authenticatorAttachment: c.authenticatorAttachment || null,
        response,
        getClientExtensionResults: () => ({ ...extensions }),
        toJSON: () => ({
          id: c.id,
          rawId: c.id,
          type: "public-key",
          authenticatorAttachment: c.authenticatorAttachment || null,
          response: response.toJSON(),
          clientExtensionResults: { ...extensions },
        }),
      },
      PKC,
    );
  };

  const createRequest = (pk) => {
    const rp = pk.rp || {};
    const user = pk.user || {};
    const sel = pk.authenticatorSelection || {};
    return {
      challenge: encode(pk.challenge),
      rp: { id: lower(rp.id), name: str(rp.name) },
      user: { id: encode(user.id), name: str(user.name), displayName: str(user.displayName) },
      pubKeyCredParams: Array.isArray(pk.pubKeyCredParams)
        ? pk.pubKeyCredParams
            .filter((p) => p && (lower(p.type) || "public-key") === "public-key")
            .map((p) => Number(p.alg))
            .filter(Number.isFinite)
        : [],
      excludeCredentials: descriptors(pk.excludeCredentials),
      authenticatorAttachment: lower(sel.authenticatorAttachment),
      residentKey: lower(sel.residentKey) || (sel.requireResidentKey === true ? "required" : undefined),
      userVerification: lower(sel.userVerification),
      attestation: lower(pk.attestation),
    };
  };

  const getRequest = (pk) => ({
    challenge: encode(pk.challenge),
    rpId: lower(pk.rpId),
    allowCredentials: descriptors(pk.allowCredentials),
    userVerification: lower(pk.userVerification),
    appid: str(pk.extensions && pk.extensions.appid),
  });

  const run = (kind, original, context, options, request) =>
    call(kind, request, options.signal).then((reply) =>
      reply.fallback ? original.call(context, options) : credential(reply.credential),
    );

  Object.defineProperty(container, "create", {
    configurable: true,
    writable: true,
    value: function create(options) {
      if (!options || !options.publicKey) return originalCreate.call(this, options);
      return run("create", originalCreate, this, options, createRequest(options.publicKey));
    },
  });

  Object.defineProperty(container, "get", {
    configurable: true,
    writable: true,
    value: function get(options) {
      if (!options || !options.publicKey) return originalGet.call(this, options);
      // Conditional (autofill) requests wait for the user to pick a passkey
      // from a form field; there's no such UI here, so they stay pending
      // instead of popping a sheet on page load.
      if (options.mediation === "conditional") {
        return new Promise((_, reject) => {
          const signal = options.signal;
          if (signal) {
            const abort = () =>
              reject(signal.reason !== undefined ? signal.reason : error("AbortError", "Aborted."));
            if (signal.aborted) abort();
            else signal.addEventListener("abort", abort, { once: true });
          }
        });
      }
      return run("get", originalGet, this, options, getRequest(options.publicKey));
    },
  });

  if (PKC) {
    const native = post({ kind: "capabilities" }).then(
      (reply) => !!(reply && reply.native),
      () => false,
    );
    const uvpa = PKC.isUserVerifyingPlatformAuthenticatorAvailable;
    PKC.isUserVerifyingPlatformAuthenticatorAvailable = function () {
      return native.then((on) => on || (uvpa ? uvpa.call(PKC) : false));
    };
    const conditional = PKC.isConditionalMediationAvailable;
    if (conditional) {
      PKC.isConditionalMediationAvailable = function () {
        return native.then((on) => (on ? false : conditional.call(PKC)));
      };
    }
  }
})();
