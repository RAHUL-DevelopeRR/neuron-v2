/**
 * Folder-trust wording (plan §2.5). The terminal UI shows the same screen, so
 * this moves to the shared `@neuron/client` trust copy once the engine-protocol
 * work publishes it; until then the desktop keeps its own copy of the text.
 */
export const TRUST_COPY = {
  title: 'Before Neuron works here',
  access: 'Neuron will be able to open, change and run files in this folder.',
  alsoTurnsOn: 'This project also wants to turn on:',
  caution: 'Continue only if you created this project or trust where it came from.',
  why: 'Why this matters',
  allow: 'Allow Neuron in this folder',
  elsewhere: 'Open a different folder',
} as const
