import { defineConfig } from '@playwright/test';

const port = 18080;

export default defineConfig({
  testDir: '.',
  testMatch: 'chat.spec.ts',
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? 'github' : 'list',
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    browserName: 'chromium',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: `cargo run --manifest-path ../Cargo.toml --bin no-llm-api -- --bind-address 127.0.0.1:${port} --scenario slow --dataset-path ../data/conversations.parquet`,
    url: `http://127.0.0.1:${port}/ready`,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
