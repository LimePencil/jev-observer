import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests-integration',
  outputDir: './test-results-integration',
  workers: 1,
  timeout: 90_000,
  use: { viewport: { width: 1440, height: 1000 }, colorScheme: 'light', trace: 'retain-on-failure' },
  reporter: 'list',
});
