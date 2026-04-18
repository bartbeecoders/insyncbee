import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    // Playwright tests live under tests/e2e and use a different runner.
    exclude: ["node_modules", "dist", "tests/**"],
  },
});
