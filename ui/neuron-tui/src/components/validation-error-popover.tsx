// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
import { pluralize } from '@neuron-tui/common/util/string'
import React, { useState } from 'react'

import { Button } from './button'
import { TerminalLink } from './terminal-link'
import { useTheme } from '../hooks/use-theme'
import { getLoadedAgentsData } from '../utils/local-agent-registry'
import { openFileAtPath } from '../utils/open-file'
import { formatValidationError } from '../utils/validation-error-formatting'
import { NETWORK_ERROR_ID } from '../utils/validation-error-helpers'

import type { LocalAgentInfo } from '../utils/local-agent-registry'

interface ValidationErrorPopoverProps {
  errors: Array<{ id: string; message: string }>
  onClose?: () => void
}

export const ValidationErrorPopover: React.FC<ValidationErrorPopoverProps> = ({
  errors,
  onClose,
}) => {
  const theme = useTheme()
  const [isCloseHovered, setIsCloseHovered] = useState(false)
  const loadedAgentsData = getLoadedAgentsData()

  const errorCount = errors.length

  return (
    <box
      style={{
        flexDirection: 'column',
        padding: 1,
        border: true,
        borderStyle: 'single',
        borderColor: theme.warning,
        backgroundColor: theme.surface,
        width: '100%',
      }}
    >
      <box style={{ flexDirection: 'column', gap: 0 }}>
        <box
          style={{
            flexDirection: 'row',
            justifyContent: 'space-between',
            alignItems: 'center',
          }}
        >
          <text style={{ fg: theme.warning, wrapMode: 'word' }}>
            {pluralize(errorCount, 'Error')}
          </text>
          {onClose && (
            <Button
              style={{ paddingRight: 1 }}
              onClick={onClose}
              onMouseOver={() => setIsCloseHovered(true)}
              onMouseOut={() => setIsCloseHovered(false)}
            >
              <text
                style={{
                  fg: isCloseHovered ? theme.foreground : theme.secondary,
                  wrapMode: 'none',
                }}
              >
                [x]
              </text>
            </Button>
          )}
        </box>

        <box style={{ flexDirection: 'column', paddingTop: 1, gap: 0 }}>
          {errors.slice(0, 3).map((error, index) => {
            const errorId = error.id ?? ''
            const agentId = errorId.replace(/_\d+$/, '')
            const isNetworkError = errorId === NETWORK_ERROR_ID
            const agentInfo = loadedAgentsData?.agents.find(
              (a) => a.id === agentId,
            ) as LocalAgentInfo | undefined

            const { fieldName, message } = formatValidationError(error.message)
            const errorMsg = fieldName ? `${fieldName}: ${message}` : message

            // Special handling for network errors - show message only without ID
            if (isNetworkError) {
              return (
                <box
                  key={errorId || `error-${index}`}
                  style={{ flexDirection: 'column', paddingTop: 0.5 }}
                >
                  <text style={{ fg: theme.muted, wrapMode: 'word' }}>
                    {`• ${errorMsg}`}
                  </text>
                </box>
              )
            }

            if (agentInfo?.filePath) {
              return (
                <box
                  key={errorId || `error-${index}`}
                  style={{ flexDirection: 'column', paddingTop: 0.5 }}
                >
                  <text style={{ fg: theme.muted, wrapMode: 'word' }}>
                    {'• '}
                    <TerminalLink
                      text={agentId}
                      color={theme.info}
                      inline={true}
                      onActivate={() => openFileAtPath(agentInfo.filePath)}
                    />
                  </text>
                  <text
                    style={{
                      fg: theme.muted,
                      wrapMode: 'word',
                      paddingLeft: 2,
                    }}
                  >
                    {errorMsg}
                  </text>
                </box>
              )
            }

            return (
              <box
                key={errorId || `error-${index}`}
                style={{ flexDirection: 'column', paddingTop: 0.5 }}
              >
                <text style={{ fg: theme.muted, wrapMode: 'word' }}>
                  {`• ${agentId || 'Unknown'}`}
                </text>
                <text
                  style={{
                    fg: theme.muted,
                    wrapMode: 'word',
                    paddingLeft: 2,
                  }}
                >
                  {errorMsg}
                </text>
              </box>
            )
          })}

          {errorCount > 3 && (
            <text
              style={{ fg: theme.muted, wrapMode: 'word', paddingTop: 0.5 }}
            >
              {`+ ${errorCount - 3} more`}
            </text>
          )}
        </box>

      </box>
    </box>
  )
}
