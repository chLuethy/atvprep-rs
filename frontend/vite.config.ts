/// <reference types="vitest/config" />
import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid() as never],
  resolve: {
    conditions: ["development", "browser"],
  },
  build: {
    sourcemap: false,
    outDir: "dist",
    emptyOutDir: true,
  },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["tests/**/*.{test,spec}.{ts,tsx}"],
    setupFiles: ["./tests/setup.ts"],
    deps: {
      inline: [/solid-js/, /@solidjs/],
    },
  },
});
