import http from 'node:http';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';

const UPSTREAM_PORT = 9099;
const GATEWAY_PORT = 8787;
const CLI_PATH = path.resolve('rust/target/release/neuron.exe');

console.log('=== Starting End-to-End External User Flow Verification ===');

// 1. Start Mock Upstream Provider
let upstreamRequests = [];
const upstreamServer = http.createServer((req, res) => {
  let body = '';
  req.on('data', chunk => { body += chunk; });
  req.on('end', () => {
    upstreamRequests.push({ method: req.method, url: req.url, headers: req.headers, body });
    
    if (req.url === '/v1/models' && req.method === 'GET') {
      res.writeHead(200, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify({
        object: 'list',
        data: [{ id: 'llama-3.3-70b-instruct', object: 'model', owned_by: 'omniroute', supported_parameters: ['tools'] }]
      }));
      return;
    }

    if (req.url === '/v1/chat/completions' && req.method === 'POST') {
      const parsed = JSON.parse(body || '{}');
      const messages = parsed.messages || [];
      const hasTools = Array.isArray(parsed.tools) && parsed.tools.length > 0;

      const isToolResult = messages.some(m => m.role === 'tool');
      const userPrompt = messages.find(m => m.role === 'user')?.content || '';
      const wantsFile = typeof userPrompt === 'string' && userPrompt.toLowerCase().includes('sample.txt');

      res.writeHead(200, {
        'Content-Type': 'text/event-stream',
        'Cache-Control': 'no-cache',
        'Connection': 'keep-alive',
      });

      if (hasTools && wantsFile && !isToolResult) {
        // Upstream triggers a tool call: read_file
        const chunk1 = {
          id: 'chatcmpl-mock-tool-1',
          object: 'chat.completion.chunk',
          created: Math.floor(Date.now() / 1000),
          model: 'llama-3.3-70b-instruct',
          choices: [{
            index: 0,
            delta: {
              role: 'assistant',
              content: null,
              tool_calls: [{
                index: 0,
                id: 'call_read_123',
                type: 'function',
                function: {
                  name: 'read_file',
                  arguments: '{"path":"sample.txt"}'
                }
              }]
            },
            finish_reason: null
          }]
        };
        const chunk2 = {
          id: 'chatcmpl-mock-tool-1',
          object: 'chat.completion.chunk',
          created: Math.floor(Date.now() / 1000),
          model: 'llama-3.3-70b-instruct',
          choices: [{
            index: 0,
            delta: {},
            finish_reason: 'tool_calls'
          }],
          usage: { prompt_tokens: 25, completion_tokens: 15, total_tokens: 40 }
        };
        res.write('data: ' + JSON.stringify(chunk1) + '\n\n');
        res.write('data: ' + JSON.stringify(chunk2) + '\n\n');
        res.write('data: [DONE]\n\n');
        res.end();
        return;
      }

      // Normal text streaming completion
      const contentText = isToolResult
        ? 'Verified file content: Hello Neuron User! Gateway tool flow verified.'
        : 'Zero-X multi-provider gateway is operating normally with high performance!';

      const chunk1 = {
        id: 'chatcmpl-mock-text-1',
        object: 'chat.completion.chunk',
        created: Math.floor(Date.now() / 1000),
        model: 'llama-3.3-70b-instruct',
        choices: [{
          index: 0,
          delta: { role: 'assistant', content: contentText },
          finish_reason: null
        }]
      };
      const chunk2 = {
        id: 'chatcmpl-mock-text-1',
        object: 'chat.completion.chunk',
        created: Math.floor(Date.now() / 1000),
        model: 'llama-3.3-70b-instruct',
        choices: [{
          index: 0,
          delta: {},
          finish_reason: 'stop'
        }],
        usage: { prompt_tokens: 20, completion_tokens: 18, total_tokens: 38 }
      };
      res.write('data: ' + JSON.stringify(chunk1) + '\n\n');
      res.write('data: ' + JSON.stringify(chunk2) + '\n\n');
      res.write('data: [DONE]\n\n');
      res.end();
      return;
    }

    res.writeHead(404);
    res.end();
  });
});

await new Promise(resolve => upstreamServer.listen(UPSTREAM_PORT, '127.0.0.1', resolve));
console.log(`[1] Mock upstream provider listening on http://127.0.0.1:${UPSTREAM_PORT}`);

// 2. Start Zero-X Gateway Server
const gatewayEnv = {
  ...process.env,
  AUTH_HOST: '127.0.0.1',
  AUTH_PORT: String(GATEWAY_PORT),
  NEURON_API_ONLY: 'true',
  ALLOW_ANONYMOUS_SESSIONS: 'true',
  OMNIROUTE_BASE_URL: `http://127.0.0.1:${UPSTREAM_PORT}/v1`,
  OMNIROUTE_API_KEY: 'platform-omniroute-secret-key',
  OMNIROUTE_MODELS: 'llama-3.3-70b-instruct',
  OMNIROUTE_TOOL_MODELS: 'llama-3.3-70b-instruct',
};

const gatewayProcess = spawn('node', ['server.js'], {
  cwd: 'C:\\Users\\DELL\\zero-x.live\\neuroncli\\auth-server',
  env: gatewayEnv,
  stdio: ['ignore', 'pipe', 'pipe']
});

let gatewayLogs = '';
gatewayProcess.stdout.on('data', d => { gatewayLogs += d.toString(); });
gatewayProcess.stderr.on('data', d => { gatewayLogs += d.toString(); });

// Wait for gateway to become ready
let gatewayReady = false;
for (let i = 0; i < 30; i++) {
  try {
    const res = await fetch(`http://127.0.0.1:${GATEWAY_PORT}/health`);
    if (res.status === 200) {
      gatewayReady = true;
      break;
    }
  } catch {}
  await new Promise(r => setTimeout(r, 200));
}
assert.ok(gatewayReady, 'Gateway server failed to start within 6 seconds. Logs:\n' + gatewayLogs);
console.log(`[2] Zero-X Gateway server running on http://127.0.0.1:${GATEWAY_PORT}`);

try {
  // Step A: External user requests a session token
  console.log('\n[Step A] External User requests a new session token via POST /auth/session...');
  const sessionRes = await fetch(`http://127.0.0.1:${GATEWAY_PORT}/auth/session`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      machine_fingerprint: 'external-user-pc-uuid-98765',
      version: '6.2.5'
    })
  });
  assert.equal(sessionRes.status, 200, 'Session creation failed');
  const sessionData = await sessionRes.json();
  const sessionToken = sessionData.session_token;
  assert.ok(sessionToken && sessionToken.startsWith('ses_'), `Invalid session token: ${sessionToken}`);
  console.log(`  -> Obtained Session Token: ${sessionToken}`);
  console.log(`  -> Plan: ${sessionData.plan}, Provider: ${sessionData.provider}`);

  // Step B: Verify session token
  console.log('\n[Step B] External User verifies session token via GET /auth/session...');
  const verifyRes = await fetch(`http://127.0.0.1:${GATEWAY_PORT}/auth/session`, {
    headers: { 'Authorization': `Bearer ${sessionToken}` }
  });
  assert.equal(verifyRes.status, 200, 'Session verification failed');
  const verifyData = await verifyRes.json();
  assert.equal(verifyData.plan, 'free');
  console.log(`  -> Session is valid. User plan: ${verifyData.plan}, Quota remaining: ${verifyData.quota?.remaining || 'ok'}`);

  // Step C: Query model catalog
  console.log('\n[Step C] External User queries models via GET /v1/models...');
  const modelsRes = await fetch(`http://127.0.0.1:${GATEWAY_PORT}/v1/models`, {
    headers: { 'Authorization': `Bearer ${sessionToken}` }
  });
  assert.equal(modelsRes.status, 200, 'Model catalog query failed');
  const modelsData = await modelsRes.json();
  console.log(`  -> Models available: ${modelsData.data.map(m => m.id).join(', ')}`);
  assert.ok(modelsData.data.some(m => m.id.includes('llama-3.3-70b-instruct')), 'Expected omniroute model in catalog');

  // Step D: Create a test file in temporary workspace for tool inspection
  const testWorkspace = path.resolve('rust/tests/scratch_user_test');
  fs.mkdirSync(testWorkspace, { recursive: true });
  fs.writeFileSync(path.join(testWorkspace, 'sample.txt'), 'Hello Neuron User!\n');

  // Step E: Run Neuron doctor as external user
  console.log('\n[Step D] Running "neuron doctor" as external user...');
  const doctorOutput = await new Promise((resolve, reject) => {
    const proc = spawn(CLI_PATH, ['doctor'], {
      cwd: testWorkspace,
      env: {
        ...process.env,
        NEURON_TOKEN: sessionToken,
        NEURON_API_BASE: `http://127.0.0.1:${GATEWAY_PORT}/v1`,
        NO_COLOR: '1'
      }
    });
    let out = '';
    proc.stdout.on('data', d => { out += d.toString(); });
    proc.stderr.on('data', d => { out += d.toString(); });
    proc.on('close', code => resolve({ code, out }));
  });
  assert.equal(doctorOutput.code, 0, `neuron doctor exited with code ${doctorOutput.code}`);
  assert.match(doctorOutput.out, /gateway_token=present/, 'Doctor must report gateway_token=present');
  console.log('  -> neuron doctor passed successfully: gateway_token=present confirmed');

  // Step F: Run basic prompt with Neuron CLI
  console.log('\n[Step E] Running "neuron --compact <prompt>" non-interactive inference prompt...');
  const promptOutput = await new Promise((resolve, reject) => {
    const proc = spawn(CLI_PATH, [
      '--compact',
      '--permission-mode', 'read-only',
      'prompt', 'Hello gateway'
    ], {
      cwd: testWorkspace,
      env: {
        ...process.env,
        NEURON_TOKEN: sessionToken,
        NEURON_API_BASE: `http://127.0.0.1:${GATEWAY_PORT}/v1`,
        NO_COLOR: '1'
      }
    });
    let out = '';
    let err = '';
    proc.stdout.on('data', d => { out += d.toString(); });
    proc.stderr.on('data', d => { err += d.toString(); });
    proc.on('close', code => resolve({ code, out, err }));
  });
  console.log(`  -> Exit code: ${promptOutput.code}`);
  console.log(`  -> Output: ${promptOutput.out.trim()}`);
  assert.equal(promptOutput.code, 0, `Prompt command failed: ${promptOutput.err}`);
  assert.match(promptOutput.out, /Zero-X multi-provider gateway is operating normally/);

  // Step G: Run tool-calling flow with Neuron CLI
  console.log('\n[Step F] Running "neuron --allowedTools read_file <prompt>" tool execution flow...');
  const toolOutput = await new Promise((resolve, reject) => {
    const proc = spawn(CLI_PATH, [
      '--compact',
      '--permission-mode', 'read-only',
      '--allowedTools', 'read_file',
      'prompt', 'Read the file sample.txt'
    ], {
      cwd: testWorkspace,
      env: {
        ...process.env,
        NEURON_TOKEN: sessionToken,
        NEURON_API_BASE: `http://127.0.0.1:${GATEWAY_PORT}/v1`,
        NO_COLOR: '1'
      }
    });
    let out = '';
    let err = '';
    proc.stdout.on('data', d => { out += d.toString(); });
    proc.stderr.on('data', d => { err += d.toString(); });
    proc.on('close', code => resolve({ code, out, err }));
  });
  console.log(`  -> Exit code: ${toolOutput.code}`);
  console.log(`  -> Output: ${toolOutput.out.trim()}`);
  assert.equal(toolOutput.code, 0, `Tool command failed: ${toolOutput.err}`);
  assert.match(toolOutput.out, /Verified file content: Hello Neuron User!/);

  // Step H: Verify Security: Upstream provider credentials NEVER leaked to client
  console.log('\n[Step G] Verifying secret isolation and security...');
  assert.ok(!promptOutput.out.includes('platform-omniroute-secret-key'), 'SECRET LEAK: Upstream API key found in prompt output!');
  assert.ok(!toolOutput.out.includes('platform-omniroute-secret-key'), 'SECRET LEAK: Upstream API key found in tool output!');
  assert.ok(!promptOutput.err.includes('platform-omniroute-secret-key'), 'SECRET LEAK: Upstream API key found in prompt stderr!');
  console.log('  -> Upstream API keys remained strictly server-side (zero secret leakage).');

  // Step I: Verify upstream request headers and correlation
  console.log('\n[Step H] Verifying request flow and upstream authentication...');
  assert.ok(upstreamRequests.length >= 2, `Expected at least 2 upstream requests, got ${upstreamRequests.length}`);
  for (const uReq of upstreamRequests) {
    if (uReq.method === 'POST') {
      assert.equal(uReq.headers.authorization, 'Bearer platform-omniroute-secret-key', 'Upstream request must contain server-side key');
    }
  }
  console.log(`  -> Verified ${upstreamRequests.length} upstream interactions successfully handled.`);

  // Cleanup test workspace
  try {
    fs.rmSync(testWorkspace, { recursive: true, force: true, maxRetries: 3 });
  } catch {}
  console.log('\n=== ALL EXTERNAL USER FLOW VERIFICATIONS PASSED SUCCESSFULLY! ===');

} finally {
  gatewayProcess.kill();
  await new Promise(r => upstreamServer.close(r));
}
