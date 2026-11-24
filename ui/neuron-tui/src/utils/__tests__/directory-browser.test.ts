import { mkdirSync, mkdtempSync, rmSync } from 'fs'
import os from 'os'
import path from 'path'

import { afterAll, describe, expect, test } from 'bun:test'

import { getDirectories, isWindowsDriveRoot, listOtherDrives } from '../directory-browser'

const temps: string[] = []
afterAll(() => {
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true })
})

describe('the picker folder rows', () => {
  test('a drive root is C:\\ or C:/, not a folder in it', () => {
    expect(isWindowsDriveRoot('C:\\')).toBe(true)
    expect(isWindowsDriveRoot('d:/')).toBe(true)
    expect(isWindowsDriveRoot('C:\\Users')).toBe(false)
    expect(isWindowsDriveRoot('/')).toBe(false)
    expect(isWindowsDriveRoot('\\\\server\\share\\')).toBe(false)
  })

  test('the other drives are the ones that exist, less the current one', () => {
    const present = new Set(['C:\\', 'D:\\', 'Z:\\'])
    expect(listOtherDrives('C:\\', (root) => present.has(root))).toEqual(['D:\\', 'Z:\\'])
    expect(listOtherDrives('d:\\', (root) => present.has(root))).toEqual(['C:\\', 'Z:\\'])
  })

  // A real drive root to read: Windows only.
  test.if(process.platform === 'win32')('at a Windows drive root the other drives come first, with no parent row', () => {
    const entries = getDirectories('C:\\', {
      platform: 'win32',
      driveExists: (root) => root === 'D:\\',
    })
    expect(entries[0]).toEqual({
      name: 'D:\\',
      path: 'D:\\',
      isParent: false,
      isGitRepo: false,
      isDrive: true,
    })
    expect(entries.some((entry) => entry.isParent)).toBe(false)
    expect(entries.filter((entry) => entry.isDrive)).toHaveLength(1)
  })

  test('below a root, or on other systems, there are no drive rows', () => {
    const dir = mkdtempSync(path.join(os.tmpdir(), 'neuron-dirs-'))
    temps.push(dir)
    mkdirSync(path.join(dir, 'alpha'))
    const always = () => true
    const inFolder = getDirectories(dir, { platform: 'win32', driveExists: always })
    expect(inFolder.map((entry) => entry.name)).toEqual(['..', 'alpha'])
    expect(getDirectories('C:\\', { platform: 'linux', driveExists: always }).some((e) => e.isDrive)).toBe(
      false,
    )
  })
})
