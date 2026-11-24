// Neura, Neuron's mascot: a small pixel neuron cell with branching dendrites and a
// firing spark. One grid is the source for every form of it:
//   node assets/mascot/build-mascot.mjs          writes neura.svg and neura-mark.svg
//   node assets/mascot/build-mascot.mjs --check  fails when they differ from the grid
// The terminal UI draws the same grid with half-block characters
// (ui/neuron-tui/src/utils/mascot.ts keeps a copy; its test compares the two).
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// A rounded cell body (the soma) with two small eyes, three dendrites rising to cyan
// tips (the middle one firing), and an axon curling off to the right that ends in a
// cyan terminal. B body, L body highlight, E eye, S spark (the signal), . empty.
export const GRID = [
  '........S.........',
  '..S.....B.....S...',
  '...B....B....B....',
  '....B...B...B.....',
  '.....BBBBBBB......',
  '....BBBBBBBBB.....',
  '...BBLBBBBBBBB....',
  '...BLBEBBBEBBB....',
  '...BBBEBBBEBBB....',
  '...BBBBBBBBBBB....',
  '....BBBBBBBBBBBB..',
  '.....BBBBBBB...B..',
  '..............SSS.',
];

export const COLORS = { B: '#3B82F6', L: '#7AA9F9', E: '#0B1220', S: '#5AC8FA' };

const here = dirname(fileURLToPath(import.meta.url));

function svg(grid, { scale = 1, title }) {
  const width = grid[0].length;
  const height = grid.length;
  const rects = [];
  grid.forEach((row, y) => {
    // Runs of one colour become one rectangle.
    let x = 0;
    while (x < row.length) {
      const cell = row[x];
      let end = x + 1;
      while (end < row.length && row[end] === cell) end += 1;
      if (cell !== '.') {
        rects.push(`  <rect x="${x}" y="${y}" width="${end - x}" height="1" fill="${COLORS[cell]}"/>`);
      }
      x = end;
    }
  });
  return [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width * scale}" height="${height * scale}" shape-rendering="crispEdges" role="img" aria-labelledby="t">`,
    `  <title id="t">${title}</title>`,
    ...rects,
    '</svg>',
    '',
  ].join('\n');
}

const outputs = {
  'neura.svg': svg(GRID, { scale: 12, title: 'Neura, the Neuron mascot' }),
  'neura-mark.svg': svg(GRID, { scale: 2, title: 'Neura' }),
};

const check = process.argv.includes('--check');
const stale = [];
for (const [name, text] of Object.entries(outputs)) {
  const path = join(here, name);
  if (check) {
    let current = '';
    try { current = readFileSync(path, 'utf8').replace(/\r\n/g, '\n'); } catch {}
    if (current !== text) stale.push(name);
  } else {
    writeFileSync(path, text);
  }
}
if (check && stale.length) {
  console.error(`out of date: ${stale.join(', ')}; run node assets/mascot/build-mascot.mjs`);
  process.exit(1);
}
