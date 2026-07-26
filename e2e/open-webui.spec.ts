import { expect, test } from '@playwright/test';

test('discovers a model and renders its streamed fixture answer', async ({ page }) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  page.on('console', message => {
    if (message.type() === 'error') {
      consoleErrors.push(message.text());
    }
  });
  page.on('pageerror', error => pageErrors.push(error.message));

  await page.goto('/', { waitUntil: 'networkidle' });
  const releaseNotes = page.getByRole('button', { name: /Okay, Let's Go!/ });
  if (await releaseNotes.isVisible()) {
    await releaseNotes.click();
  }

  await expect(page.getByRole('button', { name: /mock-gpt-4o/ }).first()).toBeVisible();
  await page.locator('#chat-input').fill('Summarize the sprint update.');

  const completionResponse = page.waitForResponse(response =>
    response.url().includes('/api/chat/completions'),
  );
  await page.locator('#chat-input').press('Enter');
  const response = await completionResponse;
  expect(response.status()).toBe(200);

  const body = page.locator('body');
  await expect.poll(async () => (await body.innerText()).includes('Sprint')).toBe(true);
  const partialBody = await body.innerText();
  expect(partialBody).not.toContain(
    'Sprint closed 14 tickets, shipped analytics, and stabilized the API.',
  );

  await expect(page.getByText(
    'Sprint closed 14 tickets, shipped analytics, and stabilized the API.',
    { exact: true },
  )).toBeVisible({ timeout: 30_000 });
  expect(consoleErrors).toEqual([]);
  expect(pageErrors).toEqual([]);
});
