/**
 * Android release builds signed with Neuron's upload key when the build has one, without EAS:
 * `expo prebuild` writes android/app/build.gradle, whose release build uses the debug key, and
 * this plugin adds a `release` signing config read from the environment.
 *
 *   NEURON_ANDROID_KEYSTORE           absolute path of the keystore (.jks)
 *   NEURON_ANDROID_KEYSTORE_PASSWORD  its password
 *   NEURON_ANDROID_KEY_ALIAS          the key's alias
 *   NEURON_ANDROID_KEY_PASSWORD       the key's password
 *
 * Without NEURON_ANDROID_KEYSTORE the release build stays debug-signed: fine to sideload for
 * testing, refused by Google Play. CI names such builds `-debug-signed`
 * (.github/workflows/mobile-native.yml). No secret is ever written into the project.
 */
const { withAppBuildGradle } = require('expo/config-plugins')

const RELEASE_SIGNING = `
        release {
            if (System.getenv('NEURON_ANDROID_KEYSTORE')) {
                storeFile file(System.getenv('NEURON_ANDROID_KEYSTORE'))
                storePassword System.getenv('NEURON_ANDROID_KEYSTORE_PASSWORD')
                keyAlias System.getenv('NEURON_ANDROID_KEY_ALIAS')
                keyPassword System.getenv('NEURON_ANDROID_KEY_PASSWORD')
            }
        }`
const RELEASE_CHOICE = "signingConfig System.getenv('NEURON_ANDROID_KEYSTORE') ? signingConfigs.release : signingConfigs.debug"

function addReleaseSigning(gradle) {
  if (gradle.includes('NEURON_ANDROID_KEYSTORE')) return gradle
  const withConfig = gradle.replace(/signingConfigs\s*\{/, (match) => `${match}${RELEASE_SIGNING}`)
  // The release build type's `signingConfig signingConfigs.debug`, not the debug build type's.
  const withChoice = withConfig.replace(/(buildTypes\s*\{[\s\S]*?\brelease\s*\{[\s\S]*?)signingConfig signingConfigs\.debug/, `$1${RELEASE_CHOICE}`)
  if (withChoice === withConfig || withConfig === gradle) {
    throw new Error('with-release-signing: android/app/build.gradle no longer has the shape this plugin expects; update plugins/with-release-signing.js.')
  }
  return withChoice
}

module.exports = function withReleaseSigning(config) {
  return withAppBuildGradle(config, (gradleConfig) => {
    if (gradleConfig.modResults.language !== 'groovy') {
      throw new Error('with-release-signing: expected a Groovy android/app/build.gradle.')
    }
    gradleConfig.modResults.contents = addReleaseSigning(gradleConfig.modResults.contents)
    return gradleConfig
  })
}

module.exports.addReleaseSigning = addReleaseSigning
