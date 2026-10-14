import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import process from "node:process";
import { join, resolve } from "node:path";

// 路径一律以 process.cwd() 为锚，不用 import.meta.url：
// Vite 会先把配置文件打包成一个临时 mjs 再加载，实测（run 38581629650）那时
// import.meta.url 不再指向仓库根的 vite.config.ts，于是 outDir 算到仓库外面 ——
// 构建日志里输出前缀是 `../../dist/` 而不是 `../dist/` 就是证据，
// 而 tauri::generate_context! 仍然报 "frontendDist ... doesn't exist"。
// 配套要求：每条 npm 步骤都必须显式 working-directory: '.'，cwd 才是仓库根。
const repo = resolve(process.cwd());

export default defineConfig({
  root: join(repo, "ui"),
  base: "./",
  resolve: {
    // 组件源里的 "@/lib/utils"、"@/components/ui/*" 是 shadcn 的别名约定
    alias: { "@": join(repo, "ui/src") },
  },
  server: { port: 1420, strictPort: true, host: process.env.TAURI_DEV_HOST },
  clearScreen: false,
  plugins: [react(), tailwindcss()],
  build: {
    outDir: join(repo, "dist"),
    emptyOutDir: true,
    target: "es2022",
  },
});
