/*
 * Ported from Freebuff, common/src/desktop-cloud/image-files.ts and media-files.ts,
 * commit 0dc95cb (https://github.com/CodebuffAI/freebuff). Copyright Codebuff,
 * Apache License 2.0. Changed for Neuron: plain types instead of zod schemas; the
 * base64 image shape, the chunked media constants and `parseByteRange` are left
 * out (the Files tab reads a whole file, size-capped, through `fs_read_media` in
 * src-tauri/src/workspace.rs, which keeps the same list); `previewOf` added.
 * See ../NOTICE.
 */

/** Browser-decodable project images. SVGs are displayed only through an <img>, never inline. */
const IMAGE_MIME_TYPES = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  avif: 'image/avif',
  svg: 'image/svg+xml',
  ico: 'image/x-icon',
  bmp: 'image/bmp',
} as const

/** Containers the webview plays natively; one it cannot decode shows a note instead. */
const MEDIA_TYPES = {
  mp4: { kind: 'video', mimeType: 'video/mp4' },
  m4v: { kind: 'video', mimeType: 'video/mp4' },
  mov: { kind: 'video', mimeType: 'video/quicktime' },
  webm: { kind: 'video', mimeType: 'video/webm' },
  ogv: { kind: 'video', mimeType: 'video/ogg' },
  mp3: { kind: 'audio', mimeType: 'audio/mpeg' },
  m4a: { kind: 'audio', mimeType: 'audio/mp4' },
  wav: { kind: 'audio', mimeType: 'audio/wav' },
  ogg: { kind: 'audio', mimeType: 'audio/ogg' },
  oga: { kind: 'audio', mimeType: 'audio/ogg' },
  flac: { kind: 'audio', mimeType: 'audio/flac' },
} as const

/** The most an image may weigh to be shown (the host refuses more). */
export const IMAGE_FILE_MAX_BYTES = 10 * 1024 * 1024
/** The most a video or sound may weigh to be played: it is read whole. */
export const MEDIA_FILE_MAX_BYTES = 64 * 1024 * 1024

export type ImageMimeType = (typeof IMAGE_MIME_TYPES)[keyof typeof IMAGE_MIME_TYPES]
export type FileMediaType = (typeof MEDIA_TYPES)[keyof typeof MEDIA_TYPES]

const extensionOf = (path: string) => /\.([^./\\]+)$/.exec(path)?.[1]?.toLowerCase()

export function imageMimeType(path: string): ImageMimeType | undefined {
  const extension = extensionOf(path)
  return extension && Object.hasOwn(IMAGE_MIME_TYPES, extension) ? IMAGE_MIME_TYPES[extension as keyof typeof IMAGE_MIME_TYPES] : undefined
}

export function mediaType(path: string): FileMediaType | undefined {
  const extension = extensionOf(path)
  return extension && Object.hasOwn(MEDIA_TYPES, extension) ? MEDIA_TYPES[extension as keyof typeof MEDIA_TYPES] : undefined
}

/** How the Files tab shows a file: as an image, a video or a sound; `null` for text and everything else. */
export function previewOf(path: string): { kind: 'image' | 'video' | 'audio'; mimeType: string } | null {
  const image = imageMimeType(path)
  if (image) return { kind: 'image', mimeType: image }
  const media = mediaType(path)
  return media ? { kind: media.kind, mimeType: media.mimeType } : null
}
