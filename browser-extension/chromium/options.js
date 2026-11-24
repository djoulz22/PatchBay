import { BROWSERS, decodePairing, send } from "./protocol.js";

const form = document.querySelector("#pairing-form");
const code = document.querySelector("#pairing");
const browser = document.querySelector("#browser");
const associate = document.querySelector("#associate");
const forget = document.querySelector("#forget");
const status = document.querySelector("#status");

function report(message, error = false) {
  status.textContent = message;
  status.classList.toggle("error", error);
}

const saved = await chrome.storage.local.get(["pairing", "browser"]);
if (BROWSERS.includes(saved.browser)) browser.value = saved.browser;
if (decodePairing(saved.pairing)) {
  forget.hidden = false;
  report("Un code d'association est enregistré.");
}

form.addEventListener("submit", async event => {
  event.preventDefault();
  const pairing = decodePairing(code.value);
  if (!pairing) {
    report("Colle le code d'association affiché dans Patchbay.", true);
    return;
  }
  associate.disabled = true;
  forget.disabled = true;
  report("Vérification du code…");
  try {
    await send(pairing, "/v1/pairing", "POST");
    await chrome.storage.local.set({ pairing: code.value.trim(), browser: browser.value });
    code.value = "";
    forget.hidden = false;
    report("Associé à Patchbay. Tu peux maintenant choisir tes onglets.");
  } catch (error) {
    report(error.message, true);
  } finally {
    associate.disabled = false;
    forget.disabled = false;
  }
});

forget.addEventListener("click", async () => {
  await chrome.storage.local.remove("pairing");
  forget.hidden = true;
  report("Extension dissociée.");
});
