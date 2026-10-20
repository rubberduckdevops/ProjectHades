import { useCallback, useEffect, useRef } from 'react'
import Box from '@cloudscape-design/components/box'
import Button from '@cloudscape-design/components/button'
import Modal from '@cloudscape-design/components/modal'
import SpaceBetween from '@cloudscape-design/components/space-between'
import { useBlocker, type BlockerFunction } from 'react-router-dom'

/**
 * Blocks leaving the page while `dirty`: the browser's native prompt on
 * reload/close (`beforeunload`), and a confirm Modal for in-app navigation
 * (react-router `useBlocker`, which needs the data router).
 *
 * Call the returned `allowNextNavigation()` right before navigating away on
 * purpose (e.g. after a save or delete), since state updates from the same
 * tick are not yet visible to the blocker.
 */
export function useUnsavedChangesGuard(dirty: boolean) {
  const dirtyRef = useRef(dirty)
  const bypassRef = useRef(false)
  useEffect(() => {
    dirtyRef.current = dirty
  }, [dirty])

  const shouldBlock = useCallback<BlockerFunction>(({ currentLocation, nextLocation }) => {
    if (bypassRef.current) {
      bypassRef.current = false
      return false
    }
    return dirtyRef.current && currentLocation.pathname !== nextLocation.pathname
  }, [])
  const blocker = useBlocker(shouldBlock)

  useEffect(() => {
    if (!dirty) return
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      e.preventDefault()
      // Legacy browsers need returnValue set to show the prompt.
      e.returnValue = ''
    }
    window.addEventListener('beforeunload', onBeforeUnload)
    return () => window.removeEventListener('beforeunload', onBeforeUnload)
  }, [dirty])

  const allowNextNavigation = useCallback(() => {
    bypassRef.current = true
  }, [])

  const modal = (
    <Modal
      visible={blocker.state === 'blocked'}
      onDismiss={() => blocker.reset?.()}
      header="Discard unsaved changes?"
      footer={
        <Box float="right">
          <SpaceBetween direction="horizontal" size="xs">
            <Button variant="link" onClick={() => blocker.reset?.()}>
              Stay on page
            </Button>
            <Button variant="primary" onClick={() => blocker.proceed?.()}>
              Discard changes
            </Button>
          </SpaceBetween>
        </Box>
      }
    >
      You have unsaved changes to this note. If you leave now, they will be lost.
    </Modal>
  )

  return { allowNextNavigation, modal }
}
