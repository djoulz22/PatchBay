export const SAMPLE_RATE = 48_000;
export const BROWSERS = ["Brave", "Chrome", "Edge", "Chromium"];

export function decodePairing(value) {
  if (typeof value !== "string") return null;
  const match = /^([1-9]\d{0,4}):([a-f0-9]{32})$/.exec(value.trim());
  if (!match || Number(match[1]) > 65_535) return null;
  return { port: Number(match[1]), token: match[2] };
}

export async function detectBrowser() {
  try {
    if (await navigator.brave?.isBrave?.()) return "Brave";
  } catch {
    // The option to choose a browser name remains available.
  }
  if (/\bEdg\//.test(navigator.userAgent)) return "Edge";
  if (/\bChromium\//.test(navigator.userAgent)) return "Chromium";
  return "Chrome";
}

export async function send(pairing, path, method, { body, type, signal } = {}) {
  const deadline = AbortSignal.timeout(1_500);
  const headers = { Authorization: `Bearer ${pairing.token}` };
  if (type) headers["Content-Type"] = type;
  let response;
  try {
    response = await fetch(`http://127.0.0.1:${pairing.port}${path}`, {
      method,
      body,
      headers,
      credentials: "omit",
      cache: "no-store",
      redirect: "error",
      signal: signal ? AbortSignal.any([signal, deadline]) : deadline,
    });
  } catch {
    throw new Error("Patchbay est indisponible. Vérifie qu'il est ouvert et renouvelle l'association.");
  }
  if (response.status === 204) return;
  if (response.status === 401 || response.status === 403) {
    throw new Error("Association refusée. Copie un nouveau code depuis Patchbay.");
  }
  if (response.status === 404) throw new Error("Capture interrompue. Clique de nouveau pour reprendre.");
  throw new Error(`Patchbay a refusé la capture (HTTP ${response.status}).`);
}
