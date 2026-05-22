/**
 * Neuron React TUI — Main application component.
 *
 * This is the React/Ink frontend for NeuronCLI. It renders:
 * 1. Option 3 block-border banner with colorized Neuron logo
 * 2. Provider/model/quota status bar
 * 3. Command hints and status
 * 4. Interactive prompt (placeholder — full REPL is Rust-backed)
 *
 * Architecture: React/Ink renders UI → Rust binary handles tools/streaming.
 */

import React, {useMemo, useState} from 'react';
import {Box, Newline, Text, useInput} from 'ink';
import {color, neuronTheme, icons} from './theme.js';
import {Banner} from './Banner.js';

type AppProps = {
  model?: string;
  permissionMode?: string;
  workspace?: string;
};

const COMMANDS = [
  ['/help', 'Show all available commands'],
  ['/status', 'Workspace, git, session, model info'],
  ['/diff', 'Review staged and unstaged changes'],
  ['/compact', 'Summarize conversation to save tokens'],
  ['/model', 'Switch provider or model'],
  ['/permissions', 'Toggle read-only / workspace-write / full-access'],
  ['/doctor', 'Run local environment checks'],
  ['/init', 'Create NEURON.md project file'],
] as const;

/**
 * Resolve provider from environment, matching Rust resolve_provider() logic.
 */
function detectProvider(): {provider: string; quota: string} {
  const azureKey = process.env.AZURE_OPENAI_API_KEY;
  const azureEndpoint = process.env.AZURE_OPENAI_ENDPOINT;
  if (azureKey && azureEndpoint) {
    return {provider: 'Azure', quota: '0K / 44K'};
  }
  if (process.env.OPENAI_API_KEY) {
    return {provider: 'OpenAI', quota: 'unlimited'};
  }
  if (process.env.OPENROUTER_API_KEY) {
    return {provider: 'OpenRouter', quota: 'free tier'};
  }
  return {provider: 'OpenRouter', quota: 'free tier'};
}

export function App({
  model = process.env.NEURON_MODEL ?? process.env.AZURE_OPENAI_MODEL ?? 'Kimi-K2.5',
  permissionMode = process.env.NEURON_PERMISSION_MODE ?? 'workspace-write',
  workspace = process.cwd(),
}: AppProps): React.ReactElement {
  const [showHelp, setShowHelp] = useState(false);
  const [status, setStatus] = useState('ready');
  const {provider, quota} = useMemo(detectProvider, []);

  useInput((input, key) => {
    if (input === 'q' || (key.ctrl && input === 'c')) {
      setStatus('exiting...');
      setTimeout(() => {
        process.exit(0);
      }, 100);
      return;
    }
    if (input === '?' || input === 'h') {
      setShowHelp((v) => !v);
      return;
    }
  });

  const bc = color(neuronTheme.brandBlue);
  const dim = color(neuronTheme.muted);
  const green = color(neuronTheme.success);
  const orange = color(neuronTheme.accent);
  const text = color(neuronTheme.text);

  return (
    <Box flexDirection="column">
      {/* ── Option 3 Block Border Banner ── */}
      <Banner
        model={model}
        provider={provider}
        quota={quota}
        cwd={workspace}
      />

      <Newline />

      {/* ── Status bar ── */}
      <Box paddingLeft={2} gap={2}>
        <Text>
          <Text color={green} bold>{icons.check}</Text>
          <Text color={text}> Connected</Text>
        </Text>
        <Text color={dim}>│</Text>
        <Text>
          <Text color={dim}>Mode: </Text>
          <Text color={orange}>{permissionMode}</Text>
        </Text>
        <Text color={dim}>│</Text>
        <Text>
          <Text color={dim}>Status: </Text>
          <Text color={text}>{status}</Text>
        </Text>
      </Box>

      <Newline />

      {/* ── Hint bar ── */}
      <Box paddingLeft={2}>
        <Text color={dim}>
          Press <Text color={orange}>?</Text> for commands · <Text color={orange}>q</Text> to quit
        </Text>
      </Box>

      {/* ── Command help panel ── */}
      {showHelp ? (
        <Box
          marginTop={1}
          marginLeft={2}
          marginRight={2}
          flexDirection="column"
          borderStyle="single"
          borderColor={bc}
          paddingX={2}
          paddingY={1}
        >
          <Text color={orange} bold>
            {icons.diamond} Available Commands
          </Text>
          <Newline />
          {COMMANDS.map(([name, description]) => (
            <Text key={name}>
              <Text color={bc} bold>
                {name.padEnd(16)}
              </Text>
              <Text color={dim}>{description}</Text>
            </Text>
          ))}
          <Newline />
          <Text color={dim}>
            {icons.arrow} Start the Rust CLI for full interactive REPL with tool execution
          </Text>
        </Box>
      ) : null}

      <Newline />

      {/* ── Prompt area ── */}
      <Box paddingLeft={2}>
        <Text color={bc} bold>
          {icons.arrow}{' '}
        </Text>
        <Text color={dim} italic>
          This is the React TUI preview. Run the Rust CLI for full tool execution.
        </Text>
      </Box>
    </Box>
  );
}
