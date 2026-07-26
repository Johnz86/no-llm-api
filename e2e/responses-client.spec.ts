import { expect, test } from '@playwright/test';
import OpenAI from 'openai';

const client = new OpenAI({
  apiKey: 'offline-test-key',
  baseURL: 'http://127.0.0.1:18080/v1',
});

test('official client receives text and public reasoning summary', async () => {
  const response = await client.responses.create({
    model: 'mock-reasoner',
    input: 'Which release should ship?',
    reasoning: { effort: 'high', summary: 'auto' },
  });

  expect(response.object).toBe('response');
  expect(response.status).toBe('completed');
  expect(response.output_text).toBe('Ship release B.');
  const reasoning = response.output.find(item => item.type === 'reasoning');
  const message = response.output.find(item => item.type === 'message');
  expect(reasoning?.summary[0]?.text).toBe(
    'Compared readiness, blockers, rollback coverage, ownership, and recovery time.',
  );
  expect(message?.content[0]?.type).toBe('output_text');
  if (message?.content[0]?.type === 'output_text') {
    expect(message.content[0].text).toBe('Ship release B.');
  }
  expect(response.usage?.output_tokens_details.reasoning_tokens).toBe(28);
});

test('official client receives validated structured output bytes', async () => {
  const response = await client.responses.create(
    {
      model: 'mock-gpt-4o',
      input: 'Report release status.',
      text: {
        format: {
          type: 'json_schema',
          name: 'release-status',
          strict: true,
          schema: {
            type: 'object',
            properties: {
              status: { const: 'green' },
              blockers: { type: 'integer' },
            },
            required: ['status', 'blockers'],
            additionalProperties: false,
          },
        },
      },
    },
    { headers: { 'X-Simulate-Case': 'structured-output/release-status' } },
  );

  const message = response.output.find(item => item.type === 'message');
  expect(message?.content[0]?.type).toBe('output_text');
  if (message?.content[0]?.type === 'output_text') {
    expect(message.content[0].text).toBe('{"status":"green","blockers":0}');
  }
  expect(response.output_text).toBe('{"status":"green","blockers":0}');
});
