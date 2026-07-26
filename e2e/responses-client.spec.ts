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

test('official client reconstructs a reasoning-first Responses stream', async () => {
  const stream = await client.responses.create({
    model: 'mock-reasoner',
    input: 'Which release should ship?',
    reasoning: { effort: 'high', summary: 'auto' },
    stream: true,
  });
  const eventTypes: string[] = [];
  let summary = '';
  let text = '';
  let terminalOutputText: string | undefined;

  for await (const event of stream) {
    eventTypes.push(event.type);
    if (event.type === 'response.reasoning_summary_text.delta') {
      summary += event.delta;
    }
    if (event.type === 'response.output_text.delta') {
      text += event.delta;
    }
    if (event.type === 'response.completed') {
      terminalOutputText = event.response.output_text;
    }
  }

  expect(eventTypes[0]).toBe('response.created');
  expect(eventTypes.at(-1)).toBe('response.completed');
  expect(eventTypes.indexOf('response.reasoning_summary_text.done')).toBeLessThan(
    eventTypes.indexOf('response.output_text.delta'),
  );
  expect(summary).toBe(
    'Compared readiness, blockers, rollback coverage, ownership, and recovery time.',
  );
  expect(text).toBe('Ship release B.');
  expect(terminalOutputText).toBe(text);
});

test('official client preserves structured bytes across a Responses stream', async () => {
  const stream = await client.responses.create(
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
      stream: true,
    },
    { headers: { 'X-Simulate-Case': 'structured-output/release-status' } },
  );
  let text = '';
  let terminalOutputText: string | undefined;

  for await (const event of stream) {
    if (event.type === 'response.output_text.delta') {
      text += event.delta;
    }
    if (event.type === 'response.completed') {
      terminalOutputText = event.response.output_text;
    }
  }

  expect(text).toBe('{"status":"green","blockers":0}');
  expect(JSON.parse(text)).toEqual({ status: 'green', blockers: 0 });
  expect(terminalOutputText).toBe(text);
});

test('official client reconstructs refusal events without output text', async () => {
  const stream = await client.responses.create({
    model: 'mock-gpt-4o',
    input: 'Perform the disallowed deployment action.',
    stream: true,
  });
  let refusal = '';
  let terminalOutputText: string | undefined;

  for await (const event of stream) {
    if (event.type === 'response.refusal.delta') {
      refusal += event.delta;
    }
    if (event.type === 'response.completed') {
      terminalOutputText = event.response.output_text;
    }
  }

  expect(refusal).toBe(
    'I cannot perform that action, but I can help review a safe deployment plan.',
  );
  expect(terminalOutputText).toBe('');
});

test('official client retrieves and deletes an immutable stored response', async () => {
  const created = await client.responses.create({
    model: 'mock-reasoner',
    input: 'Which release should ship?',
    reasoning: { effort: 'medium', summary: 'auto' },
    store: true,
    metadata: { suite: 'official-client-persistence' },
  });

  const retrieved = await client.responses.retrieve(created.id);
  expect(retrieved).toEqual(created);

  const deleted = await client.responses.delete(created.id);
  expect(deleted).toEqual({ id: created.id, object: 'response', deleted: true });
  await expect(client.responses.retrieve(created.id)).rejects.toMatchObject({ status: 404 });
});

test('official client continues from a stored predecessor', async () => {
  const parent = await client.responses.create({
    model: 'mock-gpt-4o',
    input: 'Summarize the release in one paragraph.',
    store: true,
  });
  const child = await client.responses.create({
    model: 'mock-gpt-4o',
    input: 'Correction: use exactly five words.',
    previous_response_id: parent.id,
    store: true,
  });

  expect(child.previous_response_id).toBe(parent.id);
  expect(child.output_text).toBe('Release validated; deployment is ready.');
  expect(child.usage?.input_tokens).toBeGreaterThan(parent.usage?.total_tokens ?? 0);
  await expect(client.responses.retrieve(child.id)).resolves.toEqual(child);
});

test('official client creates and uses a deterministic conversation resource', async () => {
  const conversation = await client.conversations.create({
    metadata: { suite: 'official-client-conversation' },
    items: [
      {
        type: 'message',
        role: 'user',
        content: 'Summarize the release in one paragraph.',
      },
      {
        type: 'message',
        role: 'assistant',
        content: 'The release is ready after validation.',
      },
    ],
  });
  await expect(client.conversations.retrieve(conversation.id)).resolves.toEqual(conversation);

  const response = await client.responses.create({
    model: 'mock-gpt-4o',
    input: 'Correction: use exactly five words.',
    conversation: conversation.id,
  });
  expect(response.conversation?.id).toBe(conversation.id);
  expect(response.output_text).toBe('Release validated; deployment is ready.');

  const deleted = await client.conversations.delete(conversation.id);
  expect(deleted).toEqual({
    id: conversation.id,
    object: 'conversation.deleted',
    deleted: true,
  });
});
