import { existsSync } from 'fs'

import { attachFiles, attachWorkspaceRoot, startAttach } from '../utils/attach-files'
import { parseAttachArgs } from '../utils/attach-routing'
import { resolveFilePath } from '../utils/image-handler'
import { logger } from '../utils/logger'

/**
 * `/attach`: the Attach button's file dialog (or the in-UI browser), or with
 * paths, those files at once. Quote a path with spaces, or type it bare when
 * it is the only one.
 */
export function handleAttachCommand(args: string): void {
  const root = attachWorkspaceRoot()
  const paths = parseAttachArgs(args, (candidate) => existsSync(resolveFilePath(candidate, root)))
  if (paths.length > 0) {
    attachFiles(paths, { root })
    return
  }
  startAttach({ root }).catch((error) => {
    logger.error({ error }, '[attach] failed to start')
  })
}
