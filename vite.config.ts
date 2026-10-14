import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

const ui = fileURLToPath(new URL("./ui/", import.meta.url));

// Tauri 的移动端打包会把产物塞进 apk/ipa 内部路径，绝对 base 在 Android 上会指到根，
// 所以固定相对 base；端口固定 1420 是 tauri.conf.json 里 devUrl 对得上用的。
export default defineConfig({
  root: ui,
  base: "./",
  resolve: {
    // 组件源里的 "@/lib/utils"、"@/components/ui/*" 是 shadcn 的别名约定，
    // 指向 ui/src（不是仓库根的 src，那底下没有前端代码）。
    alias: { "@": fileURLToPath(new URL("./ui/src/", import.meta.url)) },
  },
  server: { port: 1420, strictPort: true, host: process.env.TAURI_DEV_HOST },
  clearScreen: false,
  plugins: [react(), tailwindcss()],
  build: {
    outDir: fileURLToPath(new URL("../dist/", import.meta.url)),
    emptyOutDir: true,
    target: "es2022",
  },
});
