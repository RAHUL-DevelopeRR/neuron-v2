import path from 'path'

import { describe, test, expect } from 'bun:test'

import { isAtOrAboveHome, shouldShowProjectPicker } from '../../utils/project-picker'

describe('cli/utils/project-picker', () => {
  test('returns true when start cwd is home directory', () => {
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')

    expect(shouldShowProjectPicker(homeDir, homeDir)).toBe(true)
  })

  test('returns true when start cwd is a parent of home directory', () => {
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    const parentDir = path.dirname(homeDir)

    expect(shouldShowProjectPicker(parentDir, homeDir)).toBe(true)
    expect(shouldShowProjectPicker(root, homeDir)).toBe(true)
  })

  test('returns false when start cwd is a child of home directory', () => {
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    const childDir = path.join(homeDir, 'projects')

    expect(shouldShowProjectPicker(childDir, homeDir)).toBe(false)
  })

  test('returns false when start cwd is a sibling of home directory', () => {
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    const siblingDir = path.join(root, 'home', 'other-user')

    expect(shouldShowProjectPicker(siblingDir, homeDir)).toBe(false)
  })

  test('the git-root banner never offers the home folder or a folder above it', () => {
    // A home folder that is itself a git repo (dotfiles) made the banner offer
    // it, and the engine then refused it as too broad.
    const root = path.parse(process.cwd()).root
    const homeDir = path.join(root, 'home', 'test-user')
    expect(isAtOrAboveHome(homeDir, homeDir)).toBe(true)
    expect(isAtOrAboveHome(root, homeDir)).toBe(true)
    expect(isAtOrAboveHome(path.join(homeDir, 'code', 'app'), homeDir)).toBe(false)
  })
})
