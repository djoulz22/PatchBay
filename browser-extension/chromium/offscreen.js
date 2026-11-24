import { SAMPLE_RATE, decodePairing, send } from "./protocol.js";

const captures = new Map();

class TabCapture {
  constructor(message) {
    this.tabId = message.tabId;
    this.pairingCode = message.pairing;
    this.pairing = decodePairing(message.pairing);
    this.path = `/v1/tabs/${message.sourceId}`;
    this.controller = new AbortController();
    this.packets = [];
  }

  async start(message) {
    if (!this.pairing) throw new Error("Associe l'extension à Patchbay avant de capturer un onglet.");
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: { mandatory: { chromeMediaSource: "tab", chromeMediaSourceId: message.streamId } },
      video: false,
    });
    if (this.controller.signal.aborted) {
      this.stream.getTracks().forEach(track => track.stop());
      return;
    }
    const tracks = this.stream.getAudioTracks();
    if (!tracks.length || tracks.some(track => track.readyState === "ended")) {
      throw new Error("L'onglet n'a pas fourni de piste audio.");
    }
    for (const track of tracks) {
      track.addEventListener("ended", () => { void this.stop(); }, { once: true });
    }
    this.context = new AudioContext({
      sampleRate: SAMPLE_RATE, latencyHint: "interactive", sinkId: { type: "none" },
    });
    await this.context.audioWorklet.addModule("pcm-worklet.js");
    if (this.controller.signal.aborted) return;
    if (this.context.sampleRate !== SAMPLE_RATE) throw new Error("La capture doit fonctionner à 48 kHz.");
    this.worklet = new AudioWorkletNode(this.context, "patchbay-pcm", {
      outputChannelCount: [2],
      channelCount: 2,
      channelCountMode: "explicit",
    });
    this.worklet.port.onmessage = event => {
      if (this.controller.signal.aborted) return;
      this.packets.push(event.data);
      void this.flush();
    };
    this.registration = send(this.pairing, `${this.path}/start`, "POST", {
      body: JSON.stringify({ title: message.title, browser: message.browser }),
      type: "application/json",
    });
    await this.registration;
    if (this.controller.signal.aborted) return;
    this.source = this.context.createMediaStreamSource(this.stream);
    this.source.connect(this.worklet);
    // Keep the processing graph running with the silent sink.
    this.worklet.connect(this.context.destination);
    await this.context.resume();
  }

  async flush() {
    const signal = this.controller.signal;
    if (this.flushing || signal.aborted) return;
    this.flushing = true;
    try {
      while (this.packets.length && !signal.aborted) {
        const packet = this.packets.shift();
        await send(this.pairing, `${this.path}/pcm`, "POST", {
          body: packet, type: "application/octet-stream", signal,
        });
        if (!signal.aborted) this.worklet.port.postMessage(packet, [packet]);
      }
    } catch (error) {
      if (!signal.aborted) void this.stop(error.message);
    } finally {
      this.flushing = false;
    }
  }

  stop(error) {
    return this.stopping ??= this.release(error);
  }

  async release(error) {
    this.controller.abort();
    this.stream?.getTracks().forEach(track => track.stop());
    this.source?.disconnect();
    this.worklet?.port.close();
    this.worklet?.disconnect();
    this.packets.length = 0;
    try {
      if (this.context && this.context.state !== "closed") await this.context.close();
    } catch {
      // The media tracks are already stopped.
    }
    if (this.registration) {
      await this.registration.catch(() => {});
      await send(this.pairing, this.path, "DELETE").catch(() => {});
    }
    captures.delete(this.tabId);
    void chrome.runtime.sendMessage({ target: "worker", type: "ended", tabId: this.tabId, error }).catch(() => {});
  }
}

async function handle(message) {
  const active = captures.get(message.tabId);
  switch (message.type) {
    case "status":
      return { active: Boolean(active) };
    case "stop":
      await active?.stop();
      return { ok: true };
    case "unpair":
      await Promise.all(Array.from(captures.values())
        .filter(capture => capture.pairingCode === message.pairing)
        .map(capture => capture.stop()));
      return { ok: true };
    case "start": {
      if (active) return { ok: false, error: "L'onglet est déjà en cours de capture." };
      const capture = new TabCapture(message);
      captures.set(message.tabId, capture);
      try {
        await capture.start(message);
        return { ok: !capture.controller.signal.aborted };
      } catch (error) {
        await capture.stop(error.message);
        throw error;
      }
    }
    default:
      return { ok: false };
  }
}

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id !== chrome.runtime.id || message?.target !== "offscreen") return false;
  void handle(message).then(respond, error => respond({ ok: false, error: error.message }));
  return true;
});
