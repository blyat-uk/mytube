import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

// Tests run inside workerd against the real Worker and a real SQLite-backed Durable Object,
// configured from wrangler.jsonc. The Turnstile keys are Cloudflare's always-pass test pair,
// the same as .dev.vars.example, so a local .dev.vars can't change what the tests see.
export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.jsonc" },
      miniflare: {
        bindings: {
          TURNSTILE_SITE_KEY: "1x00000000000000000000AA",
          TURNSTILE_SECRET: "1x0000000000000000000000000000000AA",
        },
      },
    }),
  ],
});
