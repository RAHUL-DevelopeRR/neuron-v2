#!/usr/bin/env node
import React from 'react';
import {render} from 'ink';
import {App} from './App.js';

const args = new Map<string, string>();
for (let index = 2; index < process.argv.length; index += 1) {
  const arg = process.argv[index];
  if (!arg.startsWith('--')) {
    continue;
  }
  const [key, inlineValue] = arg.slice(2).split('=', 2);
  const value = inlineValue ?? process.argv[index + 1];
  if (value && !value.startsWith('--')) {
    args.set(key, value);
    if (inlineValue === undefined) {
      index += 1;
    }
  }
}

render(
  <App
    model={args.get('model')}
    permissionMode={args.get('permission-mode')}
    workspace={args.get('workspace')}
  />
);
