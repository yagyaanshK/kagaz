// Microphone for doorbell calls: takes the page's audio (44.1 or 48 kHz)
// down to 8 kHz 16-bit mono, the telephone rate, and posts 40 ms blocks.
class KagazMic extends AudioWorkletProcessor {
  constructor() {
    super();
    this.step = sampleRate / 8000; // input samples per output sample
    this.pos = 0; // fractional read position into the carried input
    this.carry = new Float32Array(0);
    this.out = new Int16Array(320);
    this.n = 0;
  }

  process(inputs) {
    const ch = inputs[0] && inputs[0][0];
    if (!ch) return true;
    const buf = new Float32Array(this.carry.length + ch.length);
    buf.set(this.carry);
    buf.set(ch, this.carry.length);
    let pos = this.pos;
    while (pos + this.step < buf.length) {
      // Average the input span this output sample covers: a cheap
      // low-pass that keeps the downsampling from aliasing.
      const a = Math.floor(pos);
      const b = Math.floor(pos + this.step);
      let sum = 0;
      for (let i = a; i < b; i++) sum += buf[i];
      const v = sum / Math.max(1, b - a);
      this.out[this.n++] = Math.max(-32768, Math.min(32767, Math.round(v * 32767)));
      if (this.n === this.out.length) {
        this.port.postMessage(this.out.slice());
        this.n = 0;
      }
      pos += this.step;
    }
    const keep = Math.floor(pos);
    this.carry = buf.slice(keep);
    this.pos = pos - keep;
    return true;
  }
}

registerProcessor("kagaz-mic", KagazMic);
