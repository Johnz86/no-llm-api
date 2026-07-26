import { expect, test } from '@playwright/test';

test('streams a fixture incrementally to its exact final text', async ({ page }) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  page.on('console', message => {
    if (message.type() === 'error') {
      consoleErrors.push(message.text());
    }
  });
  page.on('pageerror', error => pageErrors.push(error.message));

  await page.goto('/');
  await page.locator('#message-input').fill('Reply with unicode please.');
  await page.locator('#send-button').click();

  const assistant = page.locator('.assistant-message');
  await expect(assistant).toHaveCount(1);
  await expect.poll(async () => (await assistant.textContent())?.length ?? 0).toBeGreaterThan(0);
  const partialLength = (await assistant.textContent())?.length ?? 0;
  await expect.poll(async () => (await assistant.textContent())?.length ?? 0).toBeGreaterThan(partialLength);
  await expect(assistant).toHaveText('café 🚀👍 👩‍💻 你好世界 — Grüße, Ελλάδα, العربية', {
    timeout: 30_000,
  });
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});

test('renders a spec-shaped HTTP error instead of parsing it as SSE', async ({ page }) => {
  await page.route('**/v1/chat/completions', route =>
    route.fulfill({
      status: 503,
      contentType: 'application/json',
      body: JSON.stringify({
        error: {
          message: 'Planned outage',
          type: 'server_error',
          param: null,
          code: 'service_unavailable',
        },
      }),
    }),
  );

  await page.goto('/');
  await page.locator('#message-input').fill('Trigger an outage.');
  await page.locator('#message-input').press('Enter');

  await expect(page.locator('.assistant-message')).toHaveText('Error: Planned outage');
});
