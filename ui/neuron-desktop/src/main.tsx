import { StrictMode, useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'

import { App } from './App'
import { PixelLogo } from './components/PixelLogo'
import { detectPlatform, type Platform } from './platform'
import './styles.css'

function Root() {
  const [platform, setPlatform] = useState<Platform | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    detectPlatform().then(setPlatform, (failure: Error) => setError(failure.message))
  }, [])
  if (platform) return <App platform={platform} />
  return (
    <div className="splash">
      <PixelLogo pixel={10} />
      {error && (
        <div className="error-line" role="alert">
          {error}
        </div>
      )}
    </div>
  )
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
)
