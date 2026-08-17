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
      // Only the JSON form is proxied. `/v/{id}` is a route in *this* app —
      // the receipt page — and a plain "/v/" prefix rule swallowed it, so
      // clicking "public receipt" served raw JSON instead of the page.
      // Production has no such collision: the SPA is a static bundle on its own
      // origin talking to the API across the network.
      "^/v/.*\\.json$": { target: "http://localhost:8080", changeOrigin: true },
      "/.well-known": { target: "http://localhost:8080", changeOrigin: true },
    },
  },
});
