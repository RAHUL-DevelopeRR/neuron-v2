import { logoLetters } from '@neuron/client'

/** 5×5 pixel glyphs for the logo, in the NeuronCLI blocky style. */
const GLYPHS: Record<string, string[]> = {
  N: ['10001', '11001', '10101', '10011', '10001'],
  E: ['11111', '10000', '11110', '10000', '11111'],
  U: ['10001', '10001', '10001', '10001', '11111'],
  R: ['11110', '10001', '11110', '10100', '10011'],
  O: ['01110', '10001', '10001', '10001', '01110'],
}

/** The NEURON wordmark, drawn as pixels in the four brand colours. */
export function PixelLogo({ pixel = 4, title = 'Neuron' }: { pixel?: number; title?: string }) {
  const width = logoLetters.length * 6 - 1
  return (
    <svg
      role="img"
      aria-label={title}
      width={width * pixel}
      height={5 * pixel}
      viewBox={`0 0 ${width} 5`}
      shapeRendering="crispEdges"
      className="pixel-logo"
    >
      {logoLetters.map(([letter, color], index) =>
        GLYPHS[letter].flatMap((row, y) =>
          [...row].map((bit, x) =>
            bit === '1' ? <rect key={`${index}-${x}-${y}`} x={index * 6 + x} y={y} width={1} height={1} fill={color} /> : null,
          ),
        ),
      )}
    </svg>
  )
}
