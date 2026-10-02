import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  use: { baseURL: 'http://127.0.0.1:5173', viewport: { width: 1440, height: 1000 }, colorScheme: 'light', trace: 'retain-on-failure' },
  webServer: { command: process.env.OBSERVER_UI_PREVIEW === '1' ? 'npm run preview -- --port 5173' : 'npm run dev -- --port 5173', url: 'http://127.0.0.1:5173', reuseExistingServer: !process.env.CI },
  reporter: 'list',
});
