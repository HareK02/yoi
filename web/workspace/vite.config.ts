import { svelteTesting } from "@testing-library/svelte/vite";
import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";
import { CODEMIRROR_VITE_DEDUPE } from "./src/lib/workspace/config-source/vite-dedupe";

export default defineConfig({
  plugins: [sveltekit(), svelteTesting()],

  resolve: {
    dedupe: CODEMIRROR_VITE_DEDUPE,
  },

  server: {
    host: "localhost",
    port: 5173,
    strictPort: true,
    allowedHosts: ["develop.hareworks.net"],
    watch: {
      ignored: [
        "**/.tmp*",
        "**/.tmp*/**",
        // Config bundling creates and immediately deletes these files.
        "**/vite.config.*.timestamp-*.mjs",
        "**/vitest.config.*.timestamp-*.mjs",
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
