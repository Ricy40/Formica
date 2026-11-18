import { defineConfig } from "vite";

// The viewer only renders. In dev, `/ws` is proxied to sim-server (cargo run -p sim-server).
export default defineConfig({
  server: {
    port: 5173,
    proxy: {
      "/ws": { target: "ws://127.0.0.1:7878", ws: true },
    },
  },
  build: {
    target: "es2022",
    // three.js (WebGPU build) alone is ~850 kB minified; that's expected.
    chunkSizeWarningLimit: 1500,
  },
  test: {
    environment: "node",
  },
} as import("vite").UserConfig & { test: unknown });
