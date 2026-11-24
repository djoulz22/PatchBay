import { BROWSERS, decodePairing, detectBrowser } from "./protocol.js";

const offscreenUrl = chrome.runtime.getURL("offscreen.html");
const togglingTabs = new Set();
let openingOffscreen;

async function hasOffscreen() {
  const contexts = await chrome.runtime.getContexts({
    contextTypes: ["OFFSCREEN_DOCUMENT"],
    documentUrls: [offscreenUrl],
  });
  return contexts.length > 0;
}

async function openOffscreen() {
  openingOffscreen ??= (async () => {
    if (await hasOffscreen()) return;
    await chrome.offscreen.createDocument({
      url: "offscreen.html",
      reasons: ["USER_MEDIA"],
      justification: "Récupérer l'audio des onglets sélectionnés par l'utilisateur.",
    });
  })().finally(() => { openingOffscreen = undefined; });
  await openingOffscreen;
}

function askOffscreen(type, info = {}) {
  return chrome.runtime.sendMessage({ ...info, target: "offscreen", type });
}

async function showAction(tabId, active, error) {
  const text = active ? "ON" : error ? "!" : "";
  try {
    await chrome.action.setBadgeBackgroundColor({ tabId, color: active ? "#167461" : "#9e4053" });
    await chrome.action.setBadgeText({ tabId, text });
    await chrome.action.setTitle({
      tabId,
      title: active ? "Arrêter la capture Patchbay" : error || "Envoyer l'onglet à Patchbay",
    });
  } catch {
    // The tab can disappear while its capture is stopping.
  }
}

function storageId(tabId) {
  return `tab:${tabId}`;
}

async function getSourceId(tabId) {
  const field = storageId(tabId);
  const data = await chrome.storage.session.get(field);
  if (data[field]) return data[field];
  const id = crypto.randomUUID();
  await chrome.storage.session.set({ [field]: id });
  return id;
}

async function toggleCapture(tab) {
  const tabId = tab.id;
  if (!Number.isInteger(tabId) || togglingTabs.has(tabId)) return;
  togglingTabs.add(tabId);
  try {
    if (await hasOffscreen() && (await askOffscreen("status", { tabId }))?.active) {
      await askOffscreen("stop", { tabId });
      await showAction(tabId, false);
      return;
    }
    const settings = await chrome.storage.local.get(["pairing", "browser"]);
    if (!decodePairing(settings.pairing)) {
      await chrome.runtime.openOptionsPage();
      return;
    }
    await openOffscreen();
    const streamId = await chrome.tabCapture.getMediaStreamId({ targetTabId: tabId });
    const sourceId = await getSourceId(tabId);
    const title = Array.from((tab.title || "").replace(/\p{Cc}/gu, " ").trim()).slice(0, 160).join("");
    const browser = BROWSERS.includes(settings.browser) ? settings.browser : await detectBrowser();
    const response = await askOffscreen("start", {
      tabId, sourceId, streamId, title: title || "Onglet", browser, pairing: settings.pairing,
    });
    if (!response?.ok) throw new Error(response?.error || "Impossible de démarrer la capture.");
    await showAction(tabId, true);
  } catch (error) {
    await showAction(tabId, false, error.message || "Impossible de démarrer la capture.");
  } finally {
    togglingTabs.delete(tabId);
  }
}

chrome.action.onClicked.addListener(tab => { void toggleCapture(tab); });

chrome.runtime.onMessage.addListener((message, sender) => {
  if (sender.url !== offscreenUrl || message?.target !== "worker" || message.type !== "ended") return;
  void showAction(message.tabId, false, message.error);
});

chrome.tabs.onRemoved.addListener(tabId => {
  void chrome.storage.session.remove(storageId(tabId)).catch(() => {});
  void hasOffscreen().then(open => {
    if (open) return askOffscreen("stop", { tabId });
  }).catch(() => {});
});

chrome.storage.onChanged.addListener((changes, namespace) => {
  if (namespace !== "local" || !changes.pairing?.oldValue || changes.pairing.newValue === changes.pairing.oldValue) return;
  void hasOffscreen().then(open => {
    if (open) return askOffscreen("unpair", { pairing: changes.pairing.oldValue });
  }).catch(() => {});
});
