// L'écoute de l'analyseur (Santé › Mires et analyse) : un AudioWorklet qui joue le PCM stéréo
// 16 bits reçu du Core. Un tampon circulaire d'environ 2 s ; la lecture démarre une fois 100 ms
// reçues, et rattrape si le retard dépasse 300 ms (on reste près du direct).
class PcmPlayer extends AudioWorkletProcessor {
  constructor() {
    super();
    this.size = 96000;
    this.buf = new Float32Array(this.size * 2);
    this.w = 0;
    this.r = 0;
    this.started = false;
    this.port.onmessage = (e) => {
      const pcm = new Int16Array(e.data);
      for (let i = 0; i + 1 < pcm.length; i += 2) {
        const at = (this.w % this.size) * 2;
        this.buf[at] = pcm[i] / 32768;
        this.buf[at + 1] = pcm[i + 1] / 32768;
        this.w++;
      }
    };
  }

  process(_inputs, outputs) {
    const [l, r] = outputs[0];
    const ahead = this.w - this.r;
    if (!this.started && ahead >= 4800) this.started = true;
    // Trop de retard (le réseau a hoqueté) : on revient à 100 ms du direct.
    if (ahead > 14400) this.r = this.w - 4800;
    for (let i = 0; i < l.length; i++) {
      if (this.started && this.r < this.w) {
        const at = (this.r % this.size) * 2;
        l[i] = this.buf[at];
        if (r) r[i] = this.buf[at + 1];
        this.r++;
      } else {
        l[i] = 0;
        if (r) r[i] = 0;
        this.started = false;
      }
    }
    return true;
  }
}

registerProcessor('pcm-player', PcmPlayer);
