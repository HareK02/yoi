import { svelteTesting } from "@testing-library/svelte/vite";
import adapter from "@sveltejs/adapter-static";
import { sveltekit } from "@sveltejs/kit/vite";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";
import { CODEMIRROR_VITE_DEDUPE } from "./src/lib/workspace/config-source/vite-dedupe.ts";

export default defineConfig({
  plugins: [
    sveltekit({
      preprocess: vitePreprocess(),
      adapter: adapter({
        pages: "build",
        assets: "build",
        fallback: "index.html",
        precompress: false,
        strict: true,
      }),
    }),
    svelteTesting(),
  ],

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
