const FRAMES_PER_PACKET = 960;
const BYTES_PER_FRAME = 8;
const PACKET_BYTES = FRAMES_PER_PACKET * BYTES_PER_FRAME;
const PACKET_COUNT = 4;

class PatchbayPcm extends AudioWorkletProcessor {
  constructor() {
    super();
    this.pool = Array.from({ length: PACKET_COUNT }, () => new ArrayBuffer(PACKET_BYTES));
    this.packet = null;
    this.offset = 0;
    this.port.onmessage = ({ data }) => {
      if (data instanceof ArrayBuffer && data.byteLength === PACKET_BYTES) this.pool.push(data);
    };
  }

  process(inputs, outputs) {
    const [left, right = left] = inputs[0] ?? [];
    const frames = outputs[0][0].length;
    let start = 0;
    while (start < frames) {
      if (!this.packet) {
        const available = this.pool.pop();
        if (!available) break;
        this.packet = new DataView(available);
        this.offset = 0;
      }
      const count = Math.min(frames - start, (PACKET_BYTES - this.offset) / BYTES_PER_FRAME);
      for (let frame = start; frame < start + count; frame++) {
        this.packet.setFloat32(this.offset, left?.[frame] ?? 0, true);
        this.packet.setFloat32(this.offset + 4, right?.[frame] ?? 0, true);
        this.offset += BYTES_PER_FRAME;
      }
      start += count;
      if (this.offset === PACKET_BYTES) {
        const packet = this.packet.buffer;
        this.packet = null;
        this.port.postMessage(packet, [packet]);
      }
    }
    return true;
  }
}

registerProcessor("patchbay-pcm", PatchbayPcm);
