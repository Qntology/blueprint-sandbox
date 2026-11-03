import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Tauri 는 고정 포트를 씁니다. zoro-gravity(1420)와 함께 띄울 수 있게 1421 을 씁니다.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  base: "./",
  build: { outDir: "dist", emptyOutDir: true, chunkSizeWarningLimit: 4000 },
  server: { port: 1421, strictPort: true },
});
