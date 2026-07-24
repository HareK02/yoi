import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [sveltekit()],

  server: {
    allowedHosts: ["develop.hareworks.net"],
    watch: { ignored: ["**/.tmp*", "**/.tmp*/**"] },

    proxy: {
      "/api": {
        target: "http://127.0.0.1:8787",
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
