import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// Tauri v2 固定 dev 端口，禁止自动切换，否则壳加载不到前端。
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: false,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**"] },
  },
  build: {
    target: "chrome110",
    sourcemap: false,
    outDir: "dist",
  },
  envPrefix: ["VITE_", "TAURI_"],
});
