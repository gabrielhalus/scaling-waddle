import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: 'e2e',
  workers: 1,
  timeout: 60_000,
  use: { baseURL: 'http://127.0.0.1:18090' },
  webServer: {
    command: './e2e/start.sh',
    url: 'http://127.0.0.1:18090/health/ready',
    reuseExistingServer: false,
    timeout: 60_000,
  },
})
