// WebSocket connection to sim-server with handshake and automatic reconnect.

import {
  PROTOCOL_VERSION,
  decodeServerMsg,
  encodeClientMsg,
  type ClientMsg,
  type ServerMsg,
} from "./proto/messages";

export type ConnState = "connecting" | "open" | "closed";

export class Connection {
  private ws: WebSocket | null = null;
  private retryMs = 500;

  constructor(
    private url: string,
    private onMessage: (msg: ServerMsg) => void,
    private onState: (s: ConnState) => void,
  ) {
    this.connect();
  }

  private connect() {
    this.onState("connecting");
    const ws = new WebSocket(this.url);
    ws.binaryType = "arraybuffer";
    ws.onopen = () => {
      this.retryMs = 500;
      this.onState("open");
      this.send({ type: "hello", protocolVersion: PROTOCOL_VERSION });
    };
    ws.onmessage = (ev) => {
      if (!(ev.data instanceof ArrayBuffer)) return;
      try {
        this.onMessage(decodeServerMsg(new Uint8Array(ev.data)));
      } catch (e) {
        console.error("bad server message", e);
      }
    };
    ws.onclose = () => {
      this.onState("closed");
      this.ws = null;
      setTimeout(() => this.connect(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 5000);
    };
    this.ws = ws;
  }

  send(msg: ClientMsg) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(encodeClientMsg(msg));
  }
}

export function defaultServerUrl(): string {
  const override = new URLSearchParams(location.search).get("server");
  if (override) return override;
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
}
