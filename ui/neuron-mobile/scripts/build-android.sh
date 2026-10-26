#!/usr/bin/env bash
# Builds the Android release APK without EAS: a clean `expo prebuild`, then Gradle.
#
#   scripts/build-android.sh [output.apk]
#
# Signing (plugins/with-release-signing.js): with NEURON_ANDROID_KEYSTORE, NEURON_ANDROID_KEYSTORE_PASSWORD,
# NEURON_ANDROID_KEY_ALIAS and NEURON_ANDROID_KEY_PASSWORD set, the APK is signed with that key; without
# them it is debug-signed, which installs for testing but is not for distribution.
# ABIS (default arm64-v8a,x86_64: phones and emulators) picks the native architectures.
set -euo pipefail
cd "$(dirname "$0")/.."
output="${1:-dist/neuron-mobile.apk}"
abis="${ABIS:-arm64-v8a,x86_64}"

npx expo prebuild --platform android --no-install --clean
(
  cd android
  ./gradlew --no-daemon --stacktrace assembleRelease -PreactNativeArchitectures="$abis"
)
mkdir -p "$(dirname "$output")"
cp android/app/build/outputs/apk/release/app-release.apk "$output"
echo "Built $output ($abis)"
