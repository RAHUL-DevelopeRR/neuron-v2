/**
 * Banner — Option 3 Block Border banner for the Neuron React TUI.
 *
 * Faithful port of neuron_banner_fixed.py Option 3.
 * Uses █ block characters as borders with colorized "Neuron" block-char logo.
 * Each letter: N(blue), e(red), u(orange), r(orange), o(green), n(green).
 */

import React from 'react';
import {Box, Text} from 'ink';
import {color, neuronTheme} from './theme.js';

// ── Letter definitions (8 cols × 5 rows, Style B Wide Bold) ─────────
const LETTERS: Array<{rows: string[]; color: string}> = [
  {
    // N (blue)
    rows: ['██▄   ██', '████  ██', '██ ██ ██', '██  ████', '██   ▀██'],
    color: color(neuronTheme.brandBlue),
  },
  {
    // e (red)
    rows: ['        ', '  ▄██▄  ', ' █▄▄▄█▀ ', ' █▀▀▀▀  ', '  ▀██▀  '],
    color: color(neuronTheme.brandRed),
  },
  {
    // u (orange)
    rows: ['        ', ' ██  ██ ', ' ██  ██ ', ' ██  ██ ', '  ▀██▀  '],
    color: color(neuronTheme.brandOrange),
  },
  {
    // r (orange)
    rows: ['        ', ' ██▄▄▄  ', ' ███▀▀  ', ' ██     ', ' ██     '],
    color: color(neuronTheme.brandOrange),
  },
  {
    // o (green)
    rows: ['        ', '  ▄██▄  ', ' ██  ██ ', ' ██  ██ ', '  ▀██▀  '],
    color: color(neuronTheme.brandGreen),
  },
  {
    // n (green)
    rows: ['        ', ' ██▄▄█  ', ' ██  ██ ', ' ██  ██ ', ' ██  ██ '],
    color: color(neuronTheme.brandGreen),
  },
];

// Block characters that should be colorized (spaces stay plain)
const BLOCK_CHARS = '█▄▀▓▒░▐▟▙▜▛';

/**
 * Colorize a single row: block chars get the letter color, spaces stay plain.
 */
function ColorizedRow({row, letterColor}: {row: string; letterColor: string}): React.ReactElement {
  const segments: React.ReactElement[] = [];
  let plainBuffer = '';
  let blockBuffer = '';

  const flushPlain = () => {
    if (plainBuffer) {
      segments.push(<Text key={`p${segments.length}`}>{plainBuffer}</Text>);
      plainBuffer = '';
    }
  };

  const flushBlock = () => {
    if (blockBuffer) {
      segments.push(
        <Text key={`b${segments.length}`} color={letterColor} bold>
          {blockBuffer}
        </Text>,
      );
      blockBuffer = '';
    }
  };

  for (const ch of row) {
    if (BLOCK_CHARS.includes(ch)) {
      flushPlain();
      blockBuffer += ch;
    } else {
      flushBlock();
      plainBuffer += ch;
    }
  }
  flushPlain();
  flushBlock();

  return <Text>{segments}</Text>;
}

/**
 * Compose one row of the logo: all letters side by side with 1-char gaps.
 */
function LogoRow({rowIndex}: {rowIndex: number}): React.ReactElement {
  const parts: React.ReactElement[] = [];
  for (let i = 0; i < LETTERS.length; i++) {
    if (i > 0) {
      parts.push(<Text key={`gap${i}`}> </Text>);
    }
    parts.push(
      <ColorizedRow
        key={`letter${i}`}
        row={LETTERS[i].rows[rowIndex]}
        letterColor={LETTERS[i].color}
      />,
    );
  }
  return <Text>{parts}</Text>;
}

// ── Banner props ─────────────────────────────────────────────────────
type BannerProps = {
  model?: string;
  provider?: string;
  quota?: string;
  cwd?: string;
  version?: string;
};

/**
 * Option 3 Block Border Banner.
 *
 * Renders:
 *   ██████████████████████████████████████████████████████████████
 *   █                                                          █
 *   █  ██▄   ██  ▄██▄   ██  ██  ██▄▄▄    ▄██▄   ██▄▄█         █
 *   █  ████  ██ █▄▄▄█▀  ██  ██  ███▀▀   ██  ██  ██  ██        █
 *   █  ██ ██ ██ █▀▀▀▀   ██  ██  ██      ██  ██  ██  ██        █
 *   █  ██  ████  ▀██▀    ▀██▀   ██       ▀██▀   ██  ██        █
 *   █  ██   ▀██                                                █
 *   █                                                          █
 *   █   Kimi-K2.5 · Azure · Quota: 0K/44K                     █
 *   █   ~/projects/neuron-v2                                   █
 *   █                                                          █
 *   ██████████████████████████████████████████████████████████████
 */
export function Banner({
  model = 'claude-opus-4-6',
  provider = 'OpenRouter',
  quota = '0K / 44K',
  cwd = process.cwd(),
  version = '6.2.0',
}: BannerProps): React.ReactElement {
  const bc = color(neuronTheme.brandBlue);
  const dim = color(neuronTheme.muted);
  const green = color(neuronTheme.brandGreen);
  const orange = color(neuronTheme.brandOrange);

  // Shorten model name (strip provider prefix)
  const modelShort = model.includes('/') ? model.split('/').pop()! : model;

  // Shorten CWD
  const shortCwd = cwd.length > 50 ? `~/${cwd.split(/[/\\]/).pop()}` : cwd;

  const W = 62; // border width in █ chars
  const borderBar = '█'.repeat(W);
  const innerSpace = ' '.repeat(W - 2);

  return (
    <Box flexDirection="column" paddingLeft={2}>
      {/* Top border */}
      <Text color={bc} bold>
        {borderBar}
      </Text>

      {/* Empty row */}
      <Text>
        <Text color={bc} bold>█</Text>
        {innerSpace}
        <Text color={bc} bold>█</Text>
      </Text>

      {/* Logo rows */}
      {[0, 1, 2, 3, 4].map((rowIdx) => (
        <Text key={rowIdx}>
          <Text color={bc} bold>█</Text>
          <Text>  </Text>
          <LogoRow rowIndex={rowIdx} />
          <Text>{'  '}</Text>
          <Text color={bc} bold>█</Text>
        </Text>
      ))}

      {/* Empty row */}
      <Text>
        <Text color={bc} bold>█</Text>
        {innerSpace}
        <Text color={bc} bold>█</Text>
      </Text>

      {/* Info line: model · provider · quota */}
      <Text>
        <Text color={bc} bold>█</Text>
        <Text>   </Text>
        <Text color={green} bold>{modelShort}</Text>
        <Text color={dim}> · </Text>
        <Text color={bc}>{provider}</Text>
        <Text color={dim}> · Quota: </Text>
        <Text color={orange}>{quota}</Text>
        <Text>{'                         '}</Text>
        <Text color={bc} bold>█</Text>
      </Text>

      {/* CWD line */}
      <Text>
        <Text color={bc} bold>█</Text>
        <Text>   </Text>
        <Text color={dim}>{shortCwd}</Text>
        <Text>{'                                        '.slice(0, Math.max(0, W - 5 - shortCwd.length))}</Text>
        <Text color={bc} bold>█</Text>
      </Text>

      {/* Version line */}
      <Text>
        <Text color={bc} bold>█</Text>
        <Text>   </Text>
        <Text color={dim}>v{version} · @ zero-x.live</Text>
        <Text>{'                                        '.slice(0, Math.max(0, W - 27 - version.length))}</Text>
        <Text color={bc} bold>█</Text>
      </Text>

      {/* Empty row */}
      <Text>
        <Text color={bc} bold>█</Text>
        {innerSpace}
        <Text color={bc} bold>█</Text>
      </Text>

      {/* Bottom border */}
      <Text color={bc} bold>
        {borderBar}
      </Text>
    </Box>
  );
}
