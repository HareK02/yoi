import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [sveltekit()],

  server: {
    host: "localhost",
    port: 5173,
    strictPort: true,
    allowedHosts: ["develop.hareworks.net"],
    watch: {
      ignored: [
        "**/.tmp*",
        "**/.tmp*/**",
        "**/vite.config.*.timestamp-*.mjs",
      ],
    },

    proxy: {
      "/api": {
        target: "http://127.0.0.1:8787",
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
