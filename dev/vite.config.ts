// Static build of the UI preview (dev/preview.html + mock backend), for
// screenshots without a dev server:
//   npx vite build --config dev/vite.config.ts --outDir <dir>
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));

export default defineConfig({
  root,
  base: "./",
  plugins: [react()],
  build: {
    rollupOptions: { input: fileURLToPath(new URL("./preview.html", import.meta.url)) },
  },
});
