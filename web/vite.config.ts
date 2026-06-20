import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// No backend HTTP server to proxy — all game data arrives over Tauri events.
// The dev server only serves the web bundle for the Tauri webview.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    strictPort: true,
  },
});
