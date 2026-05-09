import React, {useMemo, useState} from 'react';
import {Box, Newline, Text, useInput} from 'ink';
import {color, icons, opencodeTheme} from './theme.js';

type AppProps = {
  model?: string;
  permissionMode?: string;
  workspace?: string;
};

const commands = [
  ['/help', 'Show command help'],
  ['/status', 'Workspace, git, session, and model snapshot'],
  ['/diff', 'Review staged and unstaged changes'],
  ['/permissions', 'Switch read-only, workspace-write, or danger-full-access'],
  ['/model', 'Switch provider model'],
  ['/doctor', 'Run local readiness checks']
] as const;

export function App({
  model = process.env.NEURON_MODEL ?? 'claude-opus-4-6',
  permissionMode = process.env.NEURON_PERMISSION_MODE ?? 'workspace-write',
  workspace = process.cwd()
}: AppProps): React.ReactElement {
  const [showHelp, setShowHelp] = useState(false);
  const [status, setStatus] = useState('ready');

  useInput((input) => {
    if (input === 'q') {
      setStatus('quit requested');
      process.exitCode = 0;
      return;
    }
    if (input === '?' || input === 'h') {
      setShowHelp((value) => !value);
      return;
    }
    if (input === 'd') {
      setStatus('open the Rust CLI and run /diff for live changes');
    }
  });

  const shortWorkspace = useMemo(() => {
    if (workspace.length <= 58) {
      return workspace;
    }
    return `…${workspace.slice(-57)}`;
  }, [workspace]);

  return (
    <Box flexDirection="column" paddingX={1}>
      <Box borderStyle="round" borderColor={color(opencodeTheme.border)} paddingX={2} paddingY={1} flexDirection="column">
        <Text color={color(opencodeTheme.primary)} bold>
          {icons.mark} Neuron TUI
        </Text>
        <Text color={color(opencodeTheme.muted)}>
          React/Ink frontend using the local opencode palette.
        </Text>
        <Newline />
        <Text>
          <Text color={color(opencodeTheme.secondary)}>Model</Text> {model}
        </Text>
        <Text>
          <Text color={color(opencodeTheme.secondary)}>Permissions</Text> {permissionMode}
        </Text>
        <Text>
          <Text color={color(opencodeTheme.secondary)}>Workspace</Text> {shortWorkspace}
        </Text>
      </Box>

      <Box marginTop={1} borderStyle="single" borderColor={color(opencodeTheme.panel)} paddingX={2} flexDirection="column">
        <Text color={color(opencodeTheme.success)}>{icons.check} Runtime bridge</Text>
        <Text color={color(opencodeTheme.muted)}>
          This frontend is intentionally thin: it presents state and command hints while the Rust CLI remains the execution engine.
        </Text>
        <Text color={color(opencodeTheme.warning)}>
          Press ? for commands, d for diff hint, q to quit.
        </Text>
      </Box>

      {showHelp ? (
        <Box marginTop={1} flexDirection="column">
          <Text color={color(opencodeTheme.accent)} bold>Command Map</Text>
          {commands.map(([name, description]) => (
            <Text key={name}>
              <Text color={color(opencodeTheme.primary)}>{name.padEnd(14)}</Text>
              {description}
            </Text>
          ))}
        </Box>
      ) : null}

      <Box marginTop={1}>
        <Text color={color(opencodeTheme.info)}>Status:</Text>
        <Text> {status}</Text>
      </Box>
    </Box>
  );
}
