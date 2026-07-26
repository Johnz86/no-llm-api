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

test('official client manages deterministic conversation items', async () => {
  const conversation = await client.conversations.create({
    metadata: { suite: 'official-client-conversation-items' },
    items: [
      { type: 'message', role: 'user', content: 'First item' },
      { type: 'message', role: 'assistant', content: 'Second item' },
    ],
  });

  const firstPage = await client.conversations.items.list(conversation.id, {
    order: 'asc',
    limit: 1,
  });
  expect(firstPage.data).toHaveLength(1);
  expect(firstPage.has_more).toBe(true);
  expect(firstPage.hasNextPage()).toBe(true);

  const secondPage = await firstPage.getNextPage();
  expect(secondPage.data).toHaveLength(1);
  expect(secondPage.has_more).toBe(false);
  expect(secondPage.hasNextPage()).toBe(false);

  const added = await client.conversations.items.create(conversation.id, {
    items: [{ type: 'message', role: 'user', content: 'Third item' }],
  });
  expect(added.data).toHaveLength(1);
  const addedItem = added.data[0];
  expect(addedItem).toBeDefined();
  if (!addedItem) {
    throw new Error('conversation item creation returned an empty list');
  }

  const retrieved = await client.conversations.items.retrieve(addedItem.id, {
    conversation_id: conversation.id,
  });
  expect(retrieved).toEqual(addedItem);

  const afterDelete = await client.conversations.items.delete(addedItem.id, {
    conversation_id: conversation.id,
  });
  expect(afterDelete).toEqual(conversation);
  await expect(
    client.conversations.items.retrieve(addedItem.id, {
      conversation_id: conversation.id,
    }),
  ).rejects.toMatchObject({ status: 404 });
});

test('official client receives typed schema and malformed-field errors', async () => {
  await expect(
    client.responses.create(
      {
        model: 'mock-gpt-4o',
        input: 'Report release status.',
        text: {
          format: {
            type: 'json_schema',
            name: 'release-status',
            schema: { type: 'object', properties: { status: { const: 'red' } } },
          },
        },
      },
      { headers: { 'X-Simulate-Case': 'structured-output/release-status' } },
    ),
  ).rejects.toMatchObject({ status: 400, code: 'semantic_schema_error', param: 'text.format' });

  await expect(
    client.responses.create({
      model: 'mock-reasoner',
      input: 'Which release should ship?',
      reasoning: { effort: 'extreme' },
    } as never),
  ).rejects.toMatchObject({ status: 400 });
});

test('official client preserves incomplete failed and cancelled terminals', async () => {
  const cases = [
    ['Return a partial deployment summary.', 'incomplete'],
    ['Simulate a failed deployment summary.', 'failed'],
    ['Simulate a cancelled deployment summary.', 'cancelled'],
  ] as const;
  for (const [input, expected] of cases) {
    const response = await client.responses.create({
      model: 'mock-gpt-4o',
      input,
      store: false,
    });
    expect(response.status).toBe(expected);
  }
});

test('official client observes deterministic conversation write conflicts', async () => {
  const conversation = await client.conversations.create({
    metadata: { suite: 'official-client-conversation-concurrency' },
  });
  const writes = await Promise.allSettled(
    Array.from({ length: 16 }, () =>
      client.responses.create({
        model: 'mock-gpt-4o',
        input: 'Introduce the simulator.',
        conversation: conversation.id,
      }, { headers: {
        'X-Simulate-Case': 'basic-text/concise',
        'X-Simulate-Commit-Delay-Ms': '100',
      } }),
    ),
  );
  const fulfilled = writes.filter(result => result.status === 'fulfilled');
  const rejected = writes.filter(result => result.status === 'rejected');
  expect(fulfilled.length).toBeGreaterThan(0);
  expect(rejected.length).toBeGreaterThan(0);
  expect(fulfilled.length + rejected.length).toBe(16);
  for (const result of rejected) {
    expect(result.reason).toMatchObject({ status: 409, code: 'conversation_conflict' });
  }
});
