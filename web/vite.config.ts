import solid from "@solidjs/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

const BACKEND_ORIGIN = "http://localhost:3000";

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  server: {
    proxy: {
      "/api": BACKEND_ORIGIN,
      "/openapi.json": BACKEND_ORIGIN,
    },
  },
});
