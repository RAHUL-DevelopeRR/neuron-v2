// Derived from Codebuff/Freebuff (Apache-2.0); modified for Neuron.
// See ui/neuron-tui/PORTING.md.
//
// Freebuff loaded skills from ~/.agents/skills and the project through the
// Codebuff SDK. Neuron's engine owns skills, so the cache starts empty and is
// filled only by tests until the engine reports its skills.

import type { SkillDefinition, SkillsMap } from '@codebuff/common/types/skill'

let skillsCache: SkillsMap = {}

/**
 * Get all loaded skills.
 */
export function getLoadedSkills(): SkillsMap {
  return skillsCache
}

/**
 * Get a skill by name.
 */
export function getSkillByName(name: string): SkillDefinition | undefined {
  return skillsCache[name]
}

/**
 * Get the number of loaded skills.
 */
export function getSkillCount(): number {
  return Object.keys(skillsCache).length
}

// ============================================================================
// Testing utilities
// ============================================================================

/**
 * Clear cached skills. Intended for test scenarios.
 */
export function __resetSkillRegistryForTests(): void {
  skillsCache = {}
}

/**
 * Seed the cache without touching the filesystem. Intended for test scenarios.
 */
export function __setSkillsForTests(skills: SkillsMap): void {
  skillsCache = skills
}
