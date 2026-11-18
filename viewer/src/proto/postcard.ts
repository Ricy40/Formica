// Minimal postcard reader/writer. Must match the Rust `postcard` crate exactly:
// - u8/bool: 1 byte; u16/u32/u64: LEB128 varint; i16/i32/i64: zigzag + varint
// - f32: 4 bytes little-endian
// - Vec<T>/String: varint length + items; [T; N]: N items, no length
// - enum: varint variant index + payload; struct: fields in declaration order
// The golden fixtures in crates/sim-proto/fixtures are decoded by proto.test.ts to catch drift.

export class ProtoError extends Error {}

export class Reader {
  private view: DataView;
  pos = 0;

  constructor(private bytes: Uint8Array) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  get remaining(): number {
    return this.bytes.length - this.pos;
  }

  u8(): number {
    if (this.pos >= this.bytes.length) throw new ProtoError("unexpected end of message");
    return this.bytes[this.pos++];
  }

  bool(): boolean {
    const b = this.u8();
    if (b > 1) throw new ProtoError(`invalid bool ${b}`);
    return b === 1;
  }

  /** Varint up to 32 bits. */
  u32(): number {
    let result = 0;
    let shift = 0;
    for (let i = 0; i < 5; i++) {
      const b = this.u8();
      result |= (b & 0x7f) << shift;
      if ((b & 0x80) === 0) return result >>> 0;
      shift += 7;
    }
    throw new ProtoError("varint u32 too long");
  }

  u16(): number {
    const v = this.u32();
    if (v > 0xffff) throw new ProtoError("u16 out of range");
    return v;
  }

  u64(): bigint {
    let result = 0n;
    let shift = 0n;
    for (let i = 0; i < 10; i++) {
      const b = this.u8();
      result |= BigInt(b & 0x7f) << shift;
      if ((b & 0x80) === 0) return result;
      shift += 7n;
    }
    throw new ProtoError("varint u64 too long");
  }

  /** u64 that is known to fit in a JS number (ticks, counts). */
  u64n(): number {
    const v = this.u64();
    if (v > BigInt(Number.MAX_SAFE_INTEGER)) throw new ProtoError("u64 exceeds safe integer");
    return Number(v);
  }

  i32(): number {
    const z = this.u32();
    return (z >>> 1) ^ -(z & 1);
  }

  f32(): number {
    if (this.remaining < 4) throw new ProtoError("unexpected end of message");
    const v = this.view.getFloat32(this.pos, true);
    this.pos += 4;
    return v;
  }

  /** Vec<u8>: returns a view into the message (no copy). */
  bytesView(): Uint8Array {
    const len = this.u32();
    if (this.remaining < len) throw new ProtoError("byte array overruns message");
    const out = this.bytes.subarray(this.pos, this.pos + len);
    this.pos += len;
    return out;
  }

  string(): string {
    return new TextDecoder().decode(this.bytesView());
  }

  vec<T>(item: (r: Reader) => T): T[] {
    const n = this.u32();
    const out: T[] = [];
    for (let i = 0; i < n; i++) out.push(item(this));
    return out;
  }

  i32x3(): [number, number, number] {
    return [this.i32(), this.i32(), this.i32()];
  }
}

export class Writer {
  private buf: number[] = [];

  u8(v: number): this {
    this.buf.push(v & 0xff);
    return this;
  }

  u32(v: number): this {
    v >>>= 0;
    while (v >= 0x80) {
      this.buf.push((v & 0x7f) | 0x80);
      v >>>= 7;
    }
    this.buf.push(v);
    return this;
  }

  i32(v: number): this {
    return this.u32(((v << 1) ^ (v >> 31)) >>> 0);
  }

  f32(v: number): this {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setFloat32(0, v, true);
    for (const x of b) this.buf.push(x);
    return this;
  }

  finish(): Uint8Array {
    return Uint8Array.from(this.buf);
  }
}
