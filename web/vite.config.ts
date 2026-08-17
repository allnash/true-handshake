import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
    // Speech recognition and getUserMedia require a secure context. localhost
    // counts as one; testing on a phone over the LAN does not, so use a tunnel
    // or `vite --https` when you put this on a real device.
    proxy: {
      "/v1": { target: "http://localhost:8080", changeOrigin: true },
      "/v/": { target: "http://localhost:8080", changeOrigin: true },
      "/.well-known": { target: "http://localhost:8080", changeOrigin: true },
    },
  },
});
